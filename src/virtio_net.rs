//! Legacy + modern virtio-mmio network driver (device id 1).
//!
//! QEMU `virt` exposes virtio-mmio slots at `0x0a00_0000`. We scan for a net
//! device, negotiate only MAC + VERSION_1 (no checksum/mergeable-rx offload, so
//! the header is a fixed 10 bytes and smoltcp computes checksums itself), and
//! run two queues: receive (0) and transmit (1).
//!
//! Receive is polled: a ring of pre-posted 2 KiB buffers is drained by
//! `rx_pop`. Transmit is synchronous: build a header+frame descriptor chain,
//! notify, and poll the used ring. No interrupt is enabled; the network stack
//! is driven by `crate::net::poll` from the timer tick and socket calls.

use core::ptr::{addr_of_mut, read_volatile, write_volatile};
use spin::Mutex;

const MMIO_BASE: u64 = 0x0a00_0000;
const MMIO_STRIDE: u64 = 0x200;
const SLOTS: u64 = 32;

const MAGIC: u32 = 0x7472_6976;
const DEVICE_NET: u32 = 1;

const REG_MAGIC: u64 = 0x000;
const REG_VERSION: u64 = 0x004;
const REG_DEVICE_ID: u64 = 0x008;
const REG_DEVICE_FEATURES: u64 = 0x010;
const REG_DEVICE_FEATURES_SEL: u64 = 0x014;
const REG_DRIVER_FEATURES: u64 = 0x020;
const REG_DRIVER_FEATURES_SEL: u64 = 0x024;
const REG_GUEST_PAGE_SIZE: u64 = 0x028;
const REG_QUEUE_SEL: u64 = 0x030;
const REG_QUEUE_NUM_MAX: u64 = 0x034;
const REG_QUEUE_NUM: u64 = 0x038;
const REG_QUEUE_ALIGN: u64 = 0x03c;
const REG_QUEUE_PFN: u64 = 0x040;
const REG_QUEUE_READY: u64 = 0x044;
const REG_QUEUE_NOTIFY: u64 = 0x050;
const REG_STATUS: u64 = 0x070;
const REG_QUEUE_DESC_LOW: u64 = 0x080;
const REG_QUEUE_AVAIL_LOW: u64 = 0x090;
const REG_QUEUE_USED_LOW: u64 = 0x0a0;
const REG_CONFIG: u64 = 0x100;

const STATUS_ACK: u32 = 1;
const STATUS_DRIVER: u32 = 2;
const STATUS_FEATURES_OK: u32 = 8;
const STATUS_DRIVER_OK: u32 = 4;

const FEATURE_MAC: u32 = 1 << 5;
const FEATURE_VERSION_1: u32 = 1;

const QUEUE_NUM: u16 = 16;
const RX_BUFFERS: usize = 8;
const RX_BUF_SIZE: usize = 2048;
const NET_HDR: usize = 10;
const REGION_SIZE: usize = 0x3000;
const QUEUE_ALIGN: u64 = 4096;

const DESC_FLAG_NEXT: u16 = 1;
const DESC_FLAG_WRITE: u16 = 2;

#[repr(C)]
#[derive(Clone, Copy)]
struct Descriptor {
    address: u64,
    length: u32,
    flags: u16,
    next: u16,
}

#[repr(C)]
struct Avail {
    flags: u16,
    index: u16,
    ring: [u16; QUEUE_NUM as usize],
    used_event: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct UsedElement {
    id: u32,
    length: u32,
}

#[repr(C)]
struct Used {
    flags: u16,
    index: u16,
    ring: [UsedElement; QUEUE_NUM as usize],
    avail_event: u16,
}

#[repr(C, align(4096))]
struct Region {
    bytes: [u8; REGION_SIZE],
}

static mut RX_REGION: Region = Region { bytes: [0; REGION_SIZE] };
static mut TX_REGION: Region = Region { bytes: [0; REGION_SIZE] };
static mut RX_BUFS: [[u8; RX_BUF_SIZE]; RX_BUFFERS] = [[0; RX_BUF_SIZE]; RX_BUFFERS];
static mut TX_HDR: [u8; NET_HDR] = [0; NET_HDR];

struct State {
    base: u64,
    version: u32,
    ready: bool,
    mac: [u8; 6],
    rx_avail: u16,
    rx_last_used: u16,
    tx_avail: u16,
    tx_last_used: u16,
}

static STATE: Mutex<State> = Mutex::new(State {
    base: 0,
    version: 0,
    ready: false,
    mac: [0; 6],
    rx_avail: 0,
    rx_last_used: 0,
    tx_avail: 0,
    tx_last_used: 0,
});

fn read32(base: u64, offset: u64) -> u32 {
    unsafe { read_volatile((base + offset) as *const u32) }
}

fn write32(base: u64, offset: u64, value: u32) {
    unsafe { write_volatile((base + offset) as *mut u32, value) }
}

fn read8(base: u64, offset: u64) -> u8 {
    unsafe { read_volatile((base + offset) as *const u8) }
}

fn write64(base: u64, offset: u64, value: u64) {
    write32(base, offset, value as u32);
    write32(base, offset + 4, (value >> 32) as u32);
}

fn barrier() {
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
}

fn align_up(value: u64, alignment: u64) -> u64 {
    (value + alignment - 1) & !(alignment - 1)
}

fn avail_offset(num: u16) -> u64 {
    (num as u64) * 16
}

fn used_offset(num: u16) -> u64 {
    align_up(avail_offset(num) + 4 + 2 * num as u64 + 2, QUEUE_ALIGN)
}

fn rx_region() -> u64 {
    unsafe { addr_of_mut!(RX_REGION.bytes) as u64 }
}

fn tx_region() -> u64 {
    unsafe { addr_of_mut!(TX_REGION.bytes) as u64 }
}

fn setup_queue(base: u64, version: u32, selection: u16, region: u64, num: u16) -> bool {
    write32(base, REG_QUEUE_SEL, selection as u32);
    if read32(base, REG_QUEUE_NUM_MAX) < num as u32 {
        return false;
    }
    if version == 1 {
        write32(base, REG_QUEUE_NUM, num as u32);
        write32(base, REG_QUEUE_ALIGN, QUEUE_ALIGN as u32);
        write32(base, REG_QUEUE_PFN, (region >> 12) as u32);
    } else {
        write64(base, REG_QUEUE_DESC_LOW, region);
        write64(base, REG_QUEUE_AVAIL_LOW, region + avail_offset(num));
        write64(base, REG_QUEUE_USED_LOW, region + used_offset(num));
        write32(base, REG_QUEUE_NUM, num as u32);
        write32(base, REG_QUEUE_READY, 1);
    }
    true
}

fn reset_region(region: u64) {
    unsafe { core::ptr::write_bytes(region as *mut u8, 0, REGION_SIZE) };
}

/// Scan for a virtio-net device and initialise both queues. Returns the version.
pub fn init() -> Option<u32> {
    let mut found = None;
    for slot in 0..SLOTS {
        let base = MMIO_BASE + slot * MMIO_STRIDE;
        if read32(base, REG_MAGIC) != MAGIC {
            continue;
        }
        let version = read32(base, REG_VERSION);
        if read32(base, REG_DEVICE_ID) == DEVICE_NET && (version == 1 || version == 2) {
            found = Some((base, version));
            break;
        }
    }
    let (base, version) = found?;

    reset_region(rx_region());
    reset_region(tx_region());

    write32(base, REG_STATUS, 0);
    barrier();
    write32(base, REG_STATUS, STATUS_ACK);
    write32(base, REG_STATUS, STATUS_ACK | STATUS_DRIVER);

    if version == 1 {
        write32(base, REG_GUEST_PAGE_SIZE, 4096);
        write32(base, REG_DRIVER_FEATURES, FEATURE_MAC);
    } else {
        write32(base, REG_DEVICE_FEATURES_SEL, 1);
        let has_version_1 = read32(base, REG_DEVICE_FEATURES) & FEATURE_VERSION_1 != 0;
        write32(base, REG_DEVICE_FEATURES_SEL, 0);
        if !has_version_1 {
            return None;
        }
        write32(base, REG_DRIVER_FEATURES_SEL, 0);
        write32(base, REG_DRIVER_FEATURES, FEATURE_MAC);
        write32(base, REG_DRIVER_FEATURES_SEL, 1);
        write32(base, REG_DRIVER_FEATURES, FEATURE_VERSION_1);
        write32(base, REG_STATUS, STATUS_ACK | STATUS_DRIVER | STATUS_FEATURES_OK);
        if read32(base, REG_STATUS) & STATUS_FEATURES_OK == 0 {
            return None;
        }
    }

    if !setup_queue(base, version, 0, rx_region(), QUEUE_NUM) {
        return None;
    }
    if !setup_queue(base, version, 1, tx_region(), QUEUE_NUM) {
        return None;
    }

    let mut mac = [0u8; 6];
    for (i, byte) in mac.iter_mut().enumerate() {
        *byte = read8(base, REG_CONFIG + i as u64);
    }

    let mut final_status = STATUS_ACK | STATUS_DRIVER | STATUS_DRIVER_OK;
    if version == 2 {
        final_status |= STATUS_FEATURES_OK;
    }
    barrier();
    write32(base, REG_STATUS, final_status);

    let mut state = STATE.lock();
    state.base = base;
    state.version = version;
    state.mac = mac;
    state.rx_avail = 0;
    state.rx_last_used = 0;
    state.tx_avail = 0;
    state.tx_last_used = 0;
    state.ready = true;

    // Pre-post every receive buffer.
    let desc = rx_region() as *mut Descriptor;
    for index in 0..RX_BUFFERS {
        unsafe {
            core::ptr::write(
                desc.add(index),
                Descriptor {
                    address: addr_of_mut!(RX_BUFS[index]) as u64,
                    length: RX_BUF_SIZE as u32,
                    flags: DESC_FLAG_WRITE,
                    next: 0,
                },
            );
        }
        submit_rx(&mut state, index as u16);
    }
    barrier();
    write32(base, REG_QUEUE_NOTIFY, 0);

    Some(version)
}

pub fn ready() -> bool {
    STATE.lock().ready
}

pub fn mac() -> [u8; 6] {
    STATE.lock().mac
}

fn submit_rx(state: &mut State, index: u16) {
    let avail = (rx_region() + avail_offset(QUEUE_NUM)) as *mut Avail;
    let position = state.rx_avail;
    unsafe {
        (*avail).ring[(position % QUEUE_NUM) as usize] = index;
        barrier();
        (*avail).index = position.wrapping_add(1);
    }
    state.rx_avail = position.wrapping_add(1);
}

/// Drain one received Ethernet frame into `out`, returning its length.
pub fn rx_pop(out: &mut [u8]) -> Option<usize> {
    let mut state = STATE.lock();
    if !state.ready {
        return None;
    }
    let used = (rx_region() + used_offset(QUEUE_NUM)) as *const Used;
    let index = unsafe { read_volatile(core::ptr::addr_of!((*used).index)) };
    if state.rx_last_used == index {
        return None;
    }
    let element = unsafe {
        read_volatile(core::ptr::addr_of!(
            (*used).ring[(state.rx_last_used % QUEUE_NUM) as usize]
        ))
    };
    state.rx_last_used = state.rx_last_used.wrapping_add(1);

    let buffer_index = element.id as usize;
    let total = element.length as usize;
    let mut copied = None;
    if buffer_index < RX_BUFFERS && total > NET_HDR {
        let payload = &unsafe { &RX_BUFS[buffer_index] }[NET_HDR..total.min(RX_BUF_SIZE)];
        let length = payload.len().min(out.len());
        out[..length].copy_from_slice(&payload[..length]);
        copied = Some(length);
    }

    // Re-post the buffer for the next packet.
    if buffer_index < RX_BUFFERS {
        let desc = rx_region() as *mut Descriptor;
        unsafe {
            core::ptr::write(
                desc.add(buffer_index),
                Descriptor {
                    address: addr_of_mut!(RX_BUFS[buffer_index]) as u64,
                    length: RX_BUF_SIZE as u32,
                    flags: DESC_FLAG_WRITE,
                    next: 0,
                },
            );
        }
        submit_rx(&mut state, buffer_index as u16);
        barrier();
        write32(state.base, REG_QUEUE_NOTIFY, 0);
    }
    copied
}

/// Transmit one Ethernet frame synchronously. Returns true on success.
pub fn tx_send(frame: &[u8]) -> bool {
    let mut state = STATE.lock();
    if !state.ready || frame.len() > RX_BUF_SIZE {
        return false;
    }
    let base = state.base;
    let desc = tx_region() as *mut Descriptor;
    let avail = (tx_region() + avail_offset(QUEUE_NUM)) as *mut Avail;
    let used = (tx_region() + used_offset(QUEUE_NUM)) as *const Used;

    unsafe {
        core::ptr::write(
            desc,
            Descriptor {
                address: addr_of_mut!(TX_HDR) as u64,
                length: NET_HDR as u32,
                flags: DESC_FLAG_NEXT,
                next: 1,
            },
        );
        core::ptr::write(
            desc.add(1),
            Descriptor {
                address: frame.as_ptr() as u64,
                length: frame.len() as u32,
                flags: 0,
                next: 0,
            },
        );
        let position = state.tx_avail;
        (*avail).ring[(position % QUEUE_NUM) as usize] = 0;
        barrier();
        (*avail).index = position.wrapping_add(1);
        state.tx_avail = position.wrapping_add(1);
    }
    barrier();
    write32(base, REG_QUEUE_NOTIFY, 1);

    let expected = state.tx_last_used.wrapping_add(1);
    let mut spins = 0u64;
    loop {
        barrier();
        if unsafe { read_volatile(core::ptr::addr_of!((*used).index)) } == expected {
            break;
        }
        spins += 1;
        if spins > 100_000_000 {
            return false;
        }
        core::hint::spin_loop();
    }
    state.tx_last_used = expected;
    true
}
