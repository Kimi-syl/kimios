//! virtio-mmio input devices (id 18): keyboard and pointer/tablet.
//!
//! Up to two devices are supported. Each event queue is filled with eight
//! 8-byte buffers; EV_KEY events from keyboards are translated to ASCII and
//! fed through the UART line editor, while every key/absolute/relative event
//! is also queued in `crate::input` for user-space event loops.

use core::ptr::{addr_of, read_volatile, write_volatile};
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::input;
use crate::println;

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
const REG_CONFIG: u64 = 0x100;

const CONFIG_EV_BITS: u8 = 0x11;

const STATUS_ACK: u32 = 1;
const STATUS_DRIVER: u32 = 2;
const STATUS_DRIVER_OK: u32 = 4;
const STATUS_FEATURES_OK: u32 = 8;

const FEATURE_VERSION_1: u32 = 1;

const MAX_QUEUE_NUM: u16 = 256;
const DESC_FLAG_WRITE: u16 = 2;
const EVENT_BUFFERS: usize = 8;
const EVENT_QUEUE: u16 = 0;

const REGION_SIZE: usize = 0x3000;
const QUEUE_ALIGN: u64 = 4096;
const MAX_DEVICES: usize = 2;

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
#[derive(Clone, Copy)]
struct Region {
    bytes: [u8; REGION_SIZE],
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Keyboard,
    Pointer,
}

#[derive(Clone, Copy)]
struct Device {
    base: u64,
    version: u32,
    irq: u32,
    kind: Kind,
    queue_count: u16,
    avail_index: u16,
    last_used: u16,
    event_region: Region,
    status_region: Region,
    buffers: [[u8; 8]; EVENT_BUFFERS],
}

const EMPTY_DEVICE: Device = Device {
    base: 0,
    version: 0,
    irq: u32::MAX,
    kind: Kind::Keyboard,
    queue_count: 0,
    avail_index: 0,
    last_used: 0,
    event_region: Region {
        bytes: [0; REGION_SIZE],
    },
    status_region: Region {
        bytes: [0; REGION_SIZE],
    },
    buffers: [[0; 8]; EVENT_BUFFERS],
};

static mut DEVICES: [Device; MAX_DEVICES] = [EMPTY_DEVICE; MAX_DEVICES];
static DEVICE_COUNT: AtomicU32 = AtomicU32::new(0);
static SHIFT: AtomicBool = AtomicBool::new(false);

const KEY_LEFTSHIFT: u16 = 42;
const KEY_RIGHTSHIFT: u16 = 54;

fn read32(base: u64, offset: u64) -> u32 {
    unsafe { read_volatile((base + offset) as *const u32) }
}

fn write32(base: u64, offset: u64, value: u32) {
    unsafe { write_volatile((base + offset) as *mut u32, value) }
}

fn read8(base: u64, offset: u64) -> u8 {
    unsafe { read_volatile((base + offset) as *const u8) }
}

fn write8(base: u64, offset: u64, value: u8) {
    unsafe { write_volatile((base + offset) as *mut u8, value) }
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

fn event_region(device: &Device) -> u64 {
    device.event_region.bytes.as_ptr() as u64
}

fn status_region(device: &Device) -> u64 {
    device.status_region.bytes.as_ptr() as u64
}

fn buffer_address(device: &Device, index: usize) -> u64 {
    device.buffers[index].as_ptr() as u64
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

fn submit_event(device: &mut Device, index: u16) {
    let avail = (event_region(device) + avail_offset(device.queue_count)) as *mut Avail;
    let position = device.avail_index;
    unsafe {
        (*avail).ring[(position % device.queue_count) as usize] = index;
        barrier();
        (*avail).index = position.wrapping_add(1);
    }
    device.avail_index = position.wrapping_add(1);
}

fn fill_events(device: &mut Device) {
    {
        let desc = event_region(device) as *mut Descriptor;
        for index in 0..EVENT_BUFFERS {
            unsafe {
                core::ptr::write(
                    desc.add(index),
                    Descriptor {
                        address: buffer_address(device, index),
                        length: 8,
                        flags: DESC_FLAG_WRITE,
                        next: 0,
                    },
                );
            }
            submit_event(device, index as u16);
        }
    }
    barrier();
    write32(device.base, REG_QUEUE_NOTIFY, EVENT_QUEUE as u32);
}

/// Ask the device which event types it supports.
fn detect_kind(base: u64) -> Kind {
    write8(base, REG_CONFIG, CONFIG_EV_BITS);
    write8(base, REG_CONFIG + 1, input::EV_ABS as u8);
    let absolute = read8(base, REG_CONFIG + 2) > 0;
    write8(base, REG_CONFIG + 1, input::EV_REL as u8);
    let relative = read8(base, REG_CONFIG + 2) > 0;
    write8(base, REG_CONFIG, 0);
    if absolute || relative {
        Kind::Pointer
    } else {
        Kind::Keyboard
    }
}

unsafe fn initialize_device(
    device: &mut Device,
    base: u64,
    version: u32,
    slot: u32,
    kind: Kind,
) -> bool {
    reset_region(event_region(device));
    reset_region(status_region(device));

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
            return false;
        }
        write32(base, REG_DRIVER_FEATURES_SEL, 0);
        write32(base, REG_DRIVER_FEATURES, 0);
        write32(base, REG_DRIVER_FEATURES_SEL, 1);
        write32(base, REG_DRIVER_FEATURES, FEATURE_VERSION_1);
        write32(base, REG_STATUS, STATUS_ACK | STATUS_DRIVER | STATUS_FEATURES_OK);
        if read32(base, REG_STATUS) & STATUS_FEATURES_OK == 0 {
            return false;
        }
    }

    write32(base, REG_QUEUE_SEL, 0);
    let queue_max = read32(base, REG_QUEUE_NUM_MAX) as u16;
    if queue_max < EVENT_BUFFERS as u16 {
        return false;
    }
    let num = queue_max.min(MAX_QUEUE_NUM);

    let events = event_region(device);
    let status = status_region(device);
    if !setup_queue(base, version, 0, events, num) {
        return false;
    }
    if !setup_queue(base, version, 1, status, num) {
        return false;
    }

    let mut final_status = STATUS_ACK | STATUS_DRIVER | STATUS_DRIVER_OK;
    if version == 2 {
        final_status |= STATUS_FEATURES_OK;
    }
    barrier();
    write32(base, REG_STATUS, final_status);

    device.base = base;
    device.version = version;
    device.irq = 48 + slot;
    device.kind = kind;
    device.queue_count = num;
    device.avail_index = 0;
    device.last_used = 0;
    fill_events(device);

    crate::gic::enable_spi(device.irq, 0xa0);
    true
}

/// Scan and initialise every virtio-input device (up to `MAX_DEVICES`).
pub fn init() -> usize {
    let mut count = 0;
    for slot in 0..SLOTS {
        if count == MAX_DEVICES {
            break;
        }
        let base = MMIO_BASE + slot * MMIO_STRIDE;
        if read32(base, REG_MAGIC) != MAGIC {
            continue;
        }
        let version = read32(base, REG_VERSION);
        if read32(base, REG_DEVICE_ID) != DEVICE_INPUT || (version != 1 && version != 2) {
            continue;
        }

        let kind = detect_kind(base);
        unsafe {
            let device = &mut DEVICES[count];
            if !initialize_device(device, base, version, slot as u32, kind) {
                continue;
            }
            println!(
                "virtio-input: {} at {:#x} irq {} mmio v{}",
                match kind {
                    Kind::Keyboard => "keyboard",
                    Kind::Pointer => "pointer",
                },
                base,
                device.irq,
                version
            );
        }
        count += 1;
    }
    DEVICE_COUNT.store(count as u32, Ordering::Release);
    count
}

/// Whether `intid` belongs to an initialised input device.
pub fn handles(intid: u32) -> bool {
    let count = DEVICE_COUNT.load(Ordering::Acquire) as usize;
    (0..count).any(|index| unsafe { DEVICES[index].irq == intid })
}

/// Handle a virtio-input interrupt for `intid`.
///
/// Returns `(line_ready, queued_events)`: a completed shell line and whether
/// any raw event was pushed for user space.
pub fn interrupt(intid: u32) -> (bool, bool) {
    let count = DEVICE_COUNT.load(Ordering::Acquire) as usize;
    let mut line = false;
    let mut queued = false;

    for index in 0..count {
        unsafe {
            let device = &mut DEVICES[index];
            if device.irq != intid {
                continue;
            }
            let base = device.base;
            let status = read32(base, REG_INTERRUPT_STATUS);
            if status != 0 {
                write32(base, REG_INTERRUPT_ACK, status);
            }
            let (device_line, device_queued) = process_used(device);
            line |= device_line;
            queued |= device_queued;
        }
    }
    (line, queued)
}

fn process_used(device: &mut Device) -> (bool, bool) {
    let num = device.queue_count;
    let used = (event_region(device) + used_offset(num)) as *const Used;
    let mut line = false;
    let mut queued = false;
    let mut requeued = false;

    loop {
        let device_index = unsafe { read_volatile(addr_of!((*used).index)) };
        if device.last_used == device_index {
            break;
        }
        let element = unsafe {
            read_volatile(addr_of!(
                (*used).ring[(device.last_used % num) as usize]
            ))
        };
        device.last_used = device.last_used.wrapping_add(1);

        let index = element.id as usize;
        if index < EVENT_BUFFERS && element.length >= 8 {
            let event = unsafe {
                &*(addr_of!(device.buffers[index]) as *const [u8; 8])
            };
            let (event_line, event_queued) = handle_event(device.kind, event);
            line |= event_line;
            queued |= event_queued;
        }
        submit_event(device, index as u16);
        requeued = true;
    }

    if requeued {
        barrier();
        write32(device.base, REG_QUEUE_NOTIFY, EVENT_QUEUE as u32);
    }
    (line, queued)
}

fn handle_event(kind: Kind, event: &[u8; 8]) -> (bool, bool) {
    let event_type = u16::from_le_bytes([event[0], event[1]]);
    let code = u16::from_le_bytes([event[2], event[3]]);
    let value = u32::from_le_bytes([event[4], event[5], event[6], event[7]]);

    if event_type == input::EV_SYN {
        return (false, false);
    }

    match kind {
        Kind::Pointer => {
            if event_type == input::EV_ABS
                || event_type == input::EV_REL
                || event_type == input::EV_KEY
            {
                input::push(event_type, code, value);
                return (false, true);
            }
            (false, false)
        }
        Kind::Keyboard => {
            if event_type != input::EV_KEY {
                return (false, false);
            }
            input::push(event_type, code, value);
            if code == KEY_LEFTSHIFT || code == KEY_RIGHTSHIFT {
                SHIFT.store(value != 0, Ordering::Relaxed);
                return (false, true);
            }
            if value == 0 || !crate::fb::console_enabled() {
                return (false, true);
            }
            match key_to_byte(code, SHIFT.load(Ordering::Relaxed)) {
                Some(byte) => (crate::uart::inject(byte), true),
                None => (false, true),
            }
        }
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