//! Legacy + modern virtio-mmio block driver.
//!
//! QEMU `virt` exposes up to 32 virtio-mmio slots at `0x0a00_0000`. We scan
//! them for a block device (device id 2), set up one request queue (256
//! descriptors) and run synchronous requests by polling the used ring. No
//! interrupts are needed: a filesystem call owns the queue until it completes.
//!
//! Both transports are supported:
//! - version 1 (legacy): queue address published through `QueuePFN`.
//! - version 2 (modern): separate descriptor/driver/device addresses and
//!   `VIRTIO_F_VERSION_1` negotiation.

use core::ptr::{addr_of_mut, read_volatile, write_volatile};
use spin::Mutex;

use crate::block::{self, BlockDevice};

const MMIO_BASE: u64 = 0x0a00_0000;
const MMIO_STRIDE: u64 = 0x200;
const SLOTS: u64 = 32;

const MAGIC: u32 = 0x7472_6976;
const DEVICE_BLOCK: u32 = 2;

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

const FEATURE_FLUSH: u32 = 1 << 9;
const FEATURE_VERSION_1: u32 = 1;

const REQUEST_IN: u32 = 0;
const REQUEST_OUT: u32 = 1;
const REQUEST_FLUSH: u32 = 4;

const QUEUE_NUM: u16 = 256;
const DESC_FLAG_NEXT: u16 = 1;
const DESC_FLAG_WRITE: u16 = 2;

const OK: u8 = 0;
const SECTOR_SIZE: usize = 512;

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

const QUEUE_REGION_SIZE: usize = 0x3000;
const AVAIL_OFFSET: u64 = 0x1000;
const USED_OFFSET: u64 = 0x2000;

#[repr(C, align(4096))]
struct QueueRegion {
    bytes: [u8; QUEUE_REGION_SIZE],
}

static mut QUEUE_REGION: QueueRegion = QueueRegion {
    bytes: [0; QUEUE_REGION_SIZE],
};

struct State {
    base: u64,
    queue_num: u16,
    ready: bool,
    has_flush: bool,
    last_used: u16,
    header: [u8; 16],
    status: u8,
}

const EMPTY_STATE: State = State {
    base: 0,
    queue_num: 0,
    ready: false,
    has_flush: false,
    last_used: 0,
    header: [0; 16],
    status: 0xff,
};

static STATE: Mutex<State> = Mutex::new(EMPTY_STATE);

pub struct VirtioBlk;

static DEVICE: VirtioBlk = VirtioBlk;

impl BlockDevice for VirtioBlk {
    fn block_size(&self) -> usize {
        SECTOR_SIZE
    }

    fn block_count(&self) -> u64 {
        let state = STATE.lock();
        if !state.ready {
            return 0;
        }
        read64(state.base, REG_CONFIG) & 0xffff_ffff
    }

    fn read_block(&self, lba: u64, buffer: &mut [u8]) -> Result<(), &'static str> {
        if buffer.len() != SECTOR_SIZE {
            return Err("bad read size");
        }
        if lba >= self.block_count() {
            return Err("read past end");
        }
        let mut state = STATE.lock();
        request(&mut state, REQUEST_IN, lba, Some(buffer)).map(|_| ())
    }

    fn write_block(&self, lba: u64, buffer: &[u8]) -> Result<(), &'static str> {
        if buffer.len() != SECTOR_SIZE {
            return Err("bad write size");
        }
        if lba >= self.block_count() {
            return Err("write past end");
        }
        let mut copy = [0u8; SECTOR_SIZE];
        copy.copy_from_slice(buffer);
        let mut state = STATE.lock();
        request(&mut state, REQUEST_OUT, lba, Some(&mut copy)).map(|_| ())
    }

    fn flush(&self) -> Result<(), &'static str> {
        let mut state = STATE.lock();
        if !state.ready {
            return Err("device not ready");
        }
        if !state.has_flush {
            return Ok(());
        }
        request(&mut state, REQUEST_FLUSH, 0, None).map(|_| ())
    }
}

fn read32(base: u64, offset: u64) -> u32 {
    unsafe { read_volatile((base + offset) as *const u32) }
}

fn write32(base: u64, offset: u64, value: u32) {
    unsafe { write_volatile((base + offset) as *mut u32, value) }
}

fn read64(base: u64, offset: u64) -> u64 {
    (read32(base, offset) as u64) | ((read32(base, offset + 4) as u64) << 32)
}

fn write64(base: u64, offset: u64, value: u64) {
    write32(base, offset, value as u32);
    write32(base, offset + 4, (value >> 32) as u32);
}

fn barrier() {
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
}

fn queue_base() -> u64 {
    unsafe { addr_of_mut!(QUEUE_REGION.bytes) as u64 }
}

/// Scan for a virtio block device, initialise it and register it.
/// Returns `(capacity_in_sectors, mmio_version)`.
pub fn init() -> Option<(u64, u32)> {
    let mut found = None;
    for slot in 0..SLOTS {
        let base = MMIO_BASE + slot * MMIO_STRIDE;
        if read32(base, REG_MAGIC) != MAGIC {
            continue;
        }
        let version = read32(base, REG_VERSION);
        let device = read32(base, REG_DEVICE_ID);
        if device == DEVICE_BLOCK && (version == 1 || version == 2) {
            found = Some((base, version));
            break;
        }
    }

    let (base, version) = found?;
    let qbase = queue_base();
    unsafe {
        core::ptr::write_bytes(qbase as *mut u8, 0, QUEUE_REGION_SIZE);
    }

    write32(base, REG_STATUS, 0);
    barrier();
    write32(base, REG_STATUS, STATUS_ACK);
    write32(base, REG_STATUS, STATUS_ACK | STATUS_DRIVER);

    let features = read32(base, REG_DEVICE_FEATURES);
    let has_flush = features & FEATURE_FLUSH != 0;

    let mut state = State {
        base,
        queue_num: 0,
        ready: false,
        has_flush,
        last_used: 0,
        header: [0; 16],
        status: 0xff,
    };

    if version == 1 {
        write32(base, REG_GUEST_PAGE_SIZE, 4096);
        write32(base, REG_DRIVER_FEATURES, if has_flush { FEATURE_FLUSH } else { 0 });
        write32(base, REG_QUEUE_SEL, 0);
        if read32(base, REG_QUEUE_NUM_MAX) < QUEUE_NUM as u32 {
            return None;
        }
        write32(base, REG_QUEUE_NUM, QUEUE_NUM as u32);
        write32(base, REG_QUEUE_ALIGN, 4096);
        write32(base, REG_QUEUE_PFN, (qbase >> 12) as u32);
        state.queue_num = QUEUE_NUM;

        let desc = qbase as *mut Descriptor;
        for index in 0..QUEUE_NUM as usize {
            unsafe {
                core::ptr::write(desc.add(index), Descriptor { address: 0, length: 0, flags: 0, next: 0 });
            }
        }
        let avail = (qbase + AVAIL_OFFSET) as *mut Avail;
        let used = (qbase + USED_OFFSET) as *mut Used;
        unsafe {
            (*avail).flags = 0;
            (*avail).index = 0;
            (*used).flags = 0;
            (*used).index = 0;
        }
    } else {
        write32(base, REG_DEVICE_FEATURES_SEL, 1);
        let version_1 = read32(base, REG_DEVICE_FEATURES) & FEATURE_VERSION_1 != 0;
        write32(base, REG_DEVICE_FEATURES_SEL, 0);
        if !version_1 {
            return None;
        }

        write32(base, REG_DRIVER_FEATURES_SEL, 0);
        write32(base, REG_DRIVER_FEATURES, if has_flush { FEATURE_FLUSH } else { 0 });
        write32(base, REG_DRIVER_FEATURES_SEL, 1);
        write32(base, REG_DRIVER_FEATURES, FEATURE_VERSION_1);

        write32(base, REG_STATUS, STATUS_ACK | STATUS_DRIVER | STATUS_FEATURES_OK);
        if read32(base, REG_STATUS) & STATUS_FEATURES_OK == 0 {
            return None;
        }

        write32(base, REG_QUEUE_SEL, 0);
        if read32(base, REG_QUEUE_NUM_MAX) < QUEUE_NUM as u32 {
            return None;
        }
        write64(base, REG_QUEUE_DESC_LOW, qbase);
        write64(base, REG_QUEUE_AVAIL_LOW, qbase + AVAIL_OFFSET);
        write64(base, REG_QUEUE_USED_LOW, qbase + USED_OFFSET);
        write32(base, REG_QUEUE_NUM, 8);
        state.queue_num = 8;
        write32(base, REG_QUEUE_READY, 1);

        let desc = qbase as *mut Descriptor;
        for index in 0..QUEUE_NUM as usize {
            unsafe {
                core::ptr::write(desc.add(index), Descriptor { address: 0, length: 0, flags: 0, next: 0 });
            }
        }
        let avail = (qbase + AVAIL_OFFSET) as *mut Avail;
        let used = (qbase + USED_OFFSET) as *mut Used;
        unsafe {
            (*avail).flags = 0;
            (*avail).index = 0;
            (*used).flags = 0;
            (*used).index = 0;
        }
    }

    barrier();
    let mut final_status = STATUS_ACK | STATUS_DRIVER | STATUS_DRIVER_OK;
    if version == 2 {
        final_status |= STATUS_FEATURES_OK;
    }
    write32(base, REG_STATUS, final_status);

    let capacity = read64(base, REG_CONFIG);
    if capacity == 0 {
        return None;
    }

    state.ready = true;
    *STATE.lock() = state;
    block::register(&DEVICE);
    Some((capacity, version))
}

fn request(
    state: &mut State,
    request_type: u32,
    sector: u64,
    mut data: Option<&mut [u8]>,
) -> Result<usize, &'static str> {
    if !state.ready {
        return Err("device not ready");
    }
    let qbase = queue_base();
    let desc = qbase as *mut Descriptor;
    let avail = (qbase + AVAIL_OFFSET) as *mut Avail;
    let used = (qbase + USED_OFFSET) as *mut Used;

    let header = state.header.as_mut_ptr();
    unsafe {
        core::ptr::write(header as *mut u32, request_type);
        core::ptr::write((header as *mut u32).add(1), 0);
        core::ptr::write(header.add(8) as *mut u64, sector);
    }
    state.status = 0xff;

    let data_length = match data.as_ref() {
        Some(buffer) => buffer.len(),
        None => 0,
    };

    unsafe {
        core::ptr::write(
            desc,
            Descriptor {
                address: header as u64,
                length: 16,
                flags: DESC_FLAG_NEXT,
                next: 1,
            },
        );

        let status_index;
        if let Some(buffer) = data.as_mut() {
            let flags = DESC_FLAG_NEXT
                | if request_type == REQUEST_IN { DESC_FLAG_WRITE } else { 0 };
            core::ptr::write(
                desc.add(1),
                Descriptor {
                    address: buffer.as_mut_ptr() as u64,
                    length: data_length as u32,
                    flags,
                    next: 2,
                },
            );
            status_index = 2;
        } else {
            status_index = 1;
        }

        core::ptr::write(
            desc.add(status_index),
            Descriptor {
                address: &state.status as *const u8 as u64,
                length: 1,
                flags: DESC_FLAG_WRITE,
                next: 0,
            },
        );

        let index = (*avail).index;
        let slot = (index as usize) & (state.queue_num as usize - 1);
        (*avail).ring[slot] = 0;
        barrier();
        (*avail).index = index.wrapping_add(1);
        barrier();
    }

    write32(state.base, REG_QUEUE_NOTIFY, 0);

    let expected = state.last_used.wrapping_add(1);
    let mut spins = 0u64;
    loop {
        barrier();
        if unsafe { (*used).index } == expected {
            break;
        }
        spins += 1;
        if spins > 200_000_000 {
            return Err("virtio request timed out");
        }
        core::hint::spin_loop();
    }

    state.last_used = expected;
    if state.status != OK {
        return Err("virtio request failed");
    }

    let element = unsafe {
        (*used).ring[(expected as usize - 1) & (state.queue_num as usize - 1)]
    };
    Ok(element.length as usize)
}
