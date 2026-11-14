//! virtio-mmio keyboard (device id 18) with legacy v1 and modern v2 support.
//!
//! The event queue is filled with eight 8-byte buffers. Each input event is
//! `{ type: u16, code: u16, value: u32 }` in Linux `input_event` layout; EV_KEY
//! press events are translated to ASCII and fed through the UART line editor,
//! so the shell reads keyboard and serial input through the same path.

use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};
use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU64, Ordering};

const MMIO_BASE: u64 = 0x0a00_0000;
const MMIO_STRIDE: u64 = 0x200;
const SLOTS: u64 = 32;

const MAGIC: u32 = 0x7472_6976;
const DEVICE_INPUT: u32 = 18;

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
const REG_INTERRUPT_STATUS: u64 = 0x060;
const REG_INTERRUPT_ACK: u64 = 0x064;
const REG_STATUS: u64 = 0x070;
const REG_QUEUE_DESC_LOW: u64 = 0x080;
const REG_QUEUE_AVAIL_LOW: u64 = 0x090;
const REG_QUEUE_USED_LOW: u64 = 0x0a0;

const STATUS_ACK: u32 = 1;
const STATUS_DRIVER: u32 = 2;
const STATUS_FEATURES_OK: u32 = 8;
const STATUS_DRIVER_OK: u32 = 4;

const FEATURE_VERSION_1: u32 = 1;

const MAX_QUEUE_NUM: u16 = 256;
const DESC_FLAG_WRITE: u16 = 2;
const EVENT_BUFFERS: usize = 8;
const EVENT_QUEUE: u16 = 0;

const REGION_SIZE: usize = 0x3000;
const QUEUE_ALIGN: u64 = 4096;

const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;

const KEY_LEFTSHIFT: u16 = 42;
const KEY_RIGHTSHIFT: u16 = 54;

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
    ring: [u16; MAX_QUEUE_NUM as usize],
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
    ring: [UsedElement; MAX_QUEUE_NUM as usize],
    avail_event: u16,
}

#[repr(C, align(4096))]
struct Region {
    bytes: [u8; REGION_SIZE],
}

static mut EVENT_REGION: Region = Region {
    bytes: [0; REGION_SIZE],
};
static mut STATUS_REGION: Region = Region {
    bytes: [0; REGION_SIZE],
};
static mut BUFFERS: [[u8; 8]; EVENT_BUFFERS] = [[0; 8]; EVENT_BUFFERS];

static BASE: AtomicU64 = AtomicU64::new(0);
static VERSION: AtomicU32 = AtomicU32::new(0);
static IRQ: AtomicU32 = AtomicU32::new(u32::MAX);
static READY: AtomicBool = AtomicBool::new(false);
static AVAIL_INDEX: AtomicU16 = AtomicU16::new(0);
static LAST_USED: AtomicU16 = AtomicU16::new(0);
static SHIFT: AtomicBool = AtomicBool::new(false);
static QUEUE_COUNT: AtomicU16 = AtomicU16::new(0);

fn read32(base: u64, offset: u64) -> u32 {
    unsafe { read_volatile((base + offset) as *const u32) }
}

fn write32(base: u64, offset: u64, value: u32) {
    unsafe { write_volatile((base + offset) as *mut u32, value) }
}

fn write64(base: u64, offset: u64, value: u64) {
    write32(base, offset, value as u32);
    write32(base, offset + 4, (value >> 32) as u32);
}

fn barrier() {
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
}

fn event_region() -> u64 {
    unsafe { addr_of_mut!(EVENT_REGION.bytes) as u64 }
}

fn status_region() -> u64 {
    unsafe { addr_of_mut!(STATUS_REGION.bytes) as u64 }
}

fn buffer_address(index: usize) -> u64 {
    unsafe { addr_of_mut!(BUFFERS[index]) as u64 }
}

fn align_up(value: u64, alignment: u64) -> u64 {
    (value + alignment - 1) & !(alignment - 1)
}

fn avail_offset(num: u16) -> u64 {
    (num as u64) * 16
}

fn used_offset(num: u16) -> u64 {
    align_up(
        avail_offset(num) + 4 + 2 * num as u64 + 2,
        QUEUE_ALIGN,
    )
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
    unsafe {
        core::ptr::write_bytes(region as *mut u8, 0, REGION_SIZE);
    }
}

fn submit_event(index: u16) {
    let num = QUEUE_COUNT.load(Ordering::Relaxed);
    let avail = (event_region() + avail_offset(num)) as *mut Avail;
    let position = AVAIL_INDEX.load(Ordering::Relaxed);
    unsafe {
        (*avail).ring[(position % num) as usize] = index;
        barrier();
        (*avail).index = position.wrapping_add(1);
    }
    AVAIL_INDEX.store(position.wrapping_add(1), Ordering::Relaxed);
}

fn fill_events() {
    let desc = event_region() as *mut Descriptor;
    for index in 0..EVENT_BUFFERS {
        unsafe {
            core::ptr::write(
                desc.add(index),
                Descriptor {
                    address: buffer_address(index),
                    length: 8,
                    flags: DESC_FLAG_WRITE,
                    next: 0,
                },
            );
        }
        submit_event(index as u16);
    }
    barrier();
    let base = BASE.load(Ordering::Relaxed);
    write32(base, REG_QUEUE_NOTIFY, EVENT_QUEUE as u32);
}

/// Scan for a virtio keyboard, initialise it and route its SPI.
pub fn init() -> Result<(u32, u32), &'static str> {
    let mut found = None;
    for slot in 0..SLOTS {
        let base = MMIO_BASE + slot * MMIO_STRIDE;
        if read32(base, REG_MAGIC) != MAGIC {
            continue;
        }
        let version = read32(base, REG_VERSION);
        if read32(base, REG_DEVICE_ID) == DEVICE_INPUT && (version == 1 || version == 2) {
            found = Some((base, version, slot));
            break;
        }
    }

    let (base, version, slot) = found.ok_or("not present")?;
    reset_region(event_region());
    reset_region(status_region());

    write32(base, REG_STATUS, 0);
    barrier();
    write32(base, REG_STATUS, STATUS_ACK);
    write32(base, REG_STATUS, STATUS_ACK | STATUS_DRIVER);

    if version == 1 {
        write32(base, REG_GUEST_PAGE_SIZE, 4096);
        write32(base, REG_DRIVER_FEATURES, 0);
    } else {
        write32(base, REG_DEVICE_FEATURES_SEL, 1);
        let has_version_1 = read32(base, REG_DEVICE_FEATURES) & FEATURE_VERSION_1 != 0;
        write32(base, REG_DEVICE_FEATURES_SEL, 0);
        if !has_version_1 {
            return Err("modern transport lacks VERSION_1");
        }
        write32(base, REG_DRIVER_FEATURES_SEL, 0);
        write32(base, REG_DRIVER_FEATURES, 0);
        write32(base, REG_DRIVER_FEATURES_SEL, 1);
        write32(base, REG_DRIVER_FEATURES, FEATURE_VERSION_1);
        write32(base, REG_STATUS, STATUS_ACK | STATUS_DRIVER | STATUS_FEATURES_OK);
        if read32(base, REG_STATUS) & STATUS_FEATURES_OK == 0 {
            return Err("features rejected");
        }
    }

    write32(base, REG_QUEUE_SEL, 0);
    let queue_max = read32(base, REG_QUEUE_NUM_MAX) as u16;
    if queue_max < EVENT_BUFFERS as u16 {
        return Err("event queue too small");
    }
    let num = queue_max.min(MAX_QUEUE_NUM);

    if !setup_queue(base, version, 0, event_region(), num) {
        return Err("event queue setup failed");
    }
    if !setup_queue(base, version, 1, status_region(), num) {
        return Err("status queue setup failed");
    }
    QUEUE_COUNT.store(num, Ordering::Release);

    barrier();
    let mut status = STATUS_ACK | STATUS_DRIVER | STATUS_DRIVER_OK;
    if version == 2 {
        status |= STATUS_FEATURES_OK;
    }
    write32(base, REG_STATUS, status);

    BASE.store(base, Ordering::Release);
    VERSION.store(version, Ordering::Release);
    AVAIL_INDEX.store(0, Ordering::Release);
    LAST_USED.store(0, Ordering::Release);
    fill_events();
    READY.store(true, Ordering::Release);

    // The DTB lists virtio-mmio as SPI (16 + slot); GIC INTIDs add 32.
    let irq = 48 + slot as u32;
    IRQ.store(irq, Ordering::Release);
    crate::gic::enable_spi(irq, 0xa0);
    Ok((irq, version))
}

/// INTID of the keyboard's SPI, or `u32::MAX` when absent.
pub fn interrupt_id() -> u32 {
    if READY.load(Ordering::Acquire) {
        IRQ.load(Ordering::Acquire)
    } else {
        u32::MAX
    }
}

/// Handle keyboard interrupts. Returns `true` when a complete line arrived.
pub fn interrupt() -> bool {
    if !READY.load(Ordering::Acquire) {
        return false;
    }
    let base = BASE.load(Ordering::Relaxed);
    let status = read32(base, REG_INTERRUPT_STATUS);
    if status != 0 {
        write32(base, REG_INTERRUPT_ACK, status);
    }
    process_used()
}

fn process_used() -> bool {
    let base = BASE.load(Ordering::Relaxed);
    let num = QUEUE_COUNT.load(Ordering::Relaxed);
    let used = (event_region() + used_offset(num)) as *const Used;
    let mut completed = false;
    let mut requeued = false;
    loop {
        let device_index = unsafe { read_volatile(addr_of!((*used).index)) };
        let last = LAST_USED.load(Ordering::Relaxed);
        if last == device_index {
            break;
        }
        let element = unsafe {
            read_volatile(addr_of!((*used).ring[(last % num) as usize]))
        };
        LAST_USED.store(last.wrapping_add(1), Ordering::Relaxed);

        let index = element.id as usize;
        if index < EVENT_BUFFERS && element.length >= 8 {
            unsafe {
                let event = &*(addr_of!(BUFFERS[index]) as *const [u8; 8]);
                completed |= handle_event(event);
            }
            submit_event(index as u16);
            requeued = true;
        }
    }

    if requeued {
        barrier();
        write32(base, REG_QUEUE_NOTIFY, EVENT_QUEUE as u32);
    }
    completed
}

fn handle_event(event: &[u8; 8]) -> bool {
    let kind = u16::from_le_bytes([event[0], event[1]]);
    let code = u16::from_le_bytes([event[2], event[3]]);
    let value = u32::from_le_bytes([event[4], event[5], event[6], event[7]]);

    if kind == EV_SYN {
        return false;
    }
    if kind != EV_KEY {
        return false;
    }
    if code == KEY_LEFTSHIFT || code == KEY_RIGHTSHIFT {
        SHIFT.store(value != 0, Ordering::Relaxed);
        return false;
    }
    if value == 0 {
        return false;
    }

    match key_to_byte(code, SHIFT.load(Ordering::Relaxed)) {
        Some(byte) => crate::uart::inject(byte),
        None => false,
    }
}

/// Translate a Linux input keycode to ASCII; `None` for unmapped keys.
fn key_to_byte(code: u16, shift: bool) -> Option<u8> {
    let byte = match code {
        1 => return Some(0x1b),
        2..=11 => b"1234567890"[(code - 2) as usize],
        12 => b'-',
        13 => b'=',
        14 => return Some(0x7f),
        15 => b'\t',
        16..=25 => b"qwertyuiop"[(code - 16) as usize],
        26 => b'[',
        27 => b']',
        28 => return Some(b'\n'),
        29 | 56 | 58 | 125 => return None,
        30..=38 => b"asdfghjkl"[(code - 30) as usize],
        39 => b';',
        40 => b'\'',
        41 => b'`',
        42 | 54 => return None,
        43 => b'\\',
        44..=50 => b"zxcvbnm"[(code - 44) as usize],
        51 => b',',
        52 => b'.',
        53 => b'/',
        55 => b'*',
        57 => b' ',
        _ => return None,
    };

    if !shift {
        return Some(byte);
    }
    Some(match byte {
        b'a'..=b'z' => byte - 32,
        b'1' => b'!',
        b'2' => b'@',
        b'3' => b'#',
        b'4' => b'$',
        b'5' => b'%',
        b'6' => b'^',
        b'7' => b'&',
        b'8' => b'*',
        b'9' => b'(',
        b'0' => b')',
        b'-' => b'_',
        b'=' => b'+',
        b'[' => b'{',
        b']' => b'}',
        b';' => b':',
        b'\'' => b'"',
        b'`' => b'~',
        b'\\' => b'|',
        b',' => b'<',
        b'.' => b'>',
        b'/' => b'?',
        other => other,
    })
}