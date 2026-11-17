//! VirtIO-GPU 2D backend (device id 16), hardware-independent display path.
//!
//! Implements the 2D (non-VirGL) virtio-gpu protocol: create a 2D resource,
//! attach guest backing storage, set it as scanout, then transfer dirty rects
//! to the host and flush. QEMU presents the scanout through its display backend
//! (e.g. VNC). This is an alternative to ramfb; the GUI renders identically to
//! a back buffer and calls `present`, which here issues TRANSFER_TO_HOST_2D +
//! RESOURCE_FLUSH instead of a memcpy.
//!
//! Only the control queue is used (cursor queue is optional and omitted). All
//! commands are synchronous: build a descriptor chain (request readable,
//! response writable), notify, and poll the used ring. IRQs are saved/restored
//! around the shared command buffers so the console (IRQ context) and the GUI
//! (syscall context) cannot race.

use crate::println;
use core::ptr::{addr_of_mut, read_volatile, write_volatile};
use spin::Mutex;

const MMIO_BASE: u64 = 0x0a00_0000;
const MMIO_STRIDE: u64 = 0x200;
const SLOTS: u64 = 32;

const MAGIC: u32 = 0x7472_6976;
const DEVICE_GPU: u32 = 16;

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

const STATUS_ACK: u32 = 1;
const STATUS_DRIVER: u32 = 2;
const STATUS_FEATURES_OK: u32 = 8;
const STATUS_DRIVER_OK: u32 = 4;
const FEATURE_VERSION_1: u32 = 1;

const QUEUE_NUM: u16 = 16;
const REGION_SIZE: usize = 0x3000;
const QUEUE_ALIGN: u64 = 4096;
const DESC_FLAG_NEXT: u16 = 1;
const DESC_FLAG_WRITE: u16 = 2;

// virtio-gpu command types (2D subset).
const CMD_GET_DISPLAY_INFO: u32 = 0x0100;
const CMD_RESOURCE_CREATE_2D: u32 = 0x0101;
const CMD_SET_SCANOUT: u32 = 0x0103;
const CMD_RESOURCE_FLUSH: u32 = 0x0104;
const CMD_TRANSFER_TO_HOST_2D: u32 = 0x0105;
const CMD_RESOURCE_ATTACH_BACKING: u32 = 0x0106;

const RESP_OK_NODATA: u32 = 0x1100;
const RESP_OK_DISPLAY_INFO: u32 = 0x1101;

// VIRTIO_GPU_FORMAT_B8G8R8X8_UNORM — matches the guest XRGB8888 byte order.
const FORMAT_B8G8R8X8: u32 = 2;

const CTRL_HDR: usize = 24;
const REQ_BUF: usize = 65536;
const RESP_BUF: usize = 512;

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

static mut CTRL_REGION: Region = Region { bytes: [0; REGION_SIZE] };
static mut CURSOR_REGION: Region = Region { bytes: [0; REGION_SIZE] };
static mut REQ: [u8; REQ_BUF] = [0; REQ_BUF];
static mut RESP: [u8; RESP_BUF] = [0; RESP_BUF];

struct State {
    base: u64,
    version: u32,
    ready: bool,
    resource_id: u32,
    width: u32,
    height: u32,
    avail: u16,
    last_used: u16,
}

static STATE: Mutex<State> = Mutex::new(State {
    base: 0,
    version: 0,
    ready: false,
    resource_id: 0,
    width: 0,
    height: 0,
    avail: 0,
    last_used: 0,
});

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
fn align_up(value: u64, alignment: u64) -> u64 {
    (value + alignment - 1) & !(alignment - 1)
}
fn avail_offset(num: u16) -> u64 {
    (num as u64) * 16
}
fn used_offset(num: u16) -> u64 {
    align_up(avail_offset(num) + 4 + 2 * num as u64 + 2, QUEUE_ALIGN)
}
fn ctrl_region() -> u64 {
    unsafe { addr_of_mut!(CTRL_REGION.bytes) as u64 }
}
fn cursor_region() -> u64 {
    unsafe { addr_of_mut!(CURSOR_REGION.bytes) as u64 }
}

fn put32(buf: &mut [u8], off: usize, v: u32) {
    buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
}
fn put64(buf: &mut [u8], off: usize, v: u64) {
    buf[off..off + 8].copy_from_slice(&v.to_le_bytes());
}
fn get32(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(buf[off..off + 4].try_into().unwrap())
}

/// Fill the 24-byte control header at the start of the request buffer.
fn header(req: &mut [u8], cmd: u32) {
    put32(req, 0, cmd);
    put32(req, 4, 0); // flags
    put64(req, 8, 0); // fence_id
    put32(req, 16, 0); // ctx_id
    req[20] = 0; // ring_idx
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

/// Submit a synchronous control command: `req_len` request bytes (readable) and
/// a writable response buffer. Returns the response type, or None on timeout.
fn command(state: &mut State, req_len: usize) -> Option<u32> {
    let base = state.base;
    let desc = ctrl_region() as *mut Descriptor;
    let avail = (ctrl_region() + avail_offset(QUEUE_NUM)) as *mut Avail;
    let used = (ctrl_region() + used_offset(QUEUE_NUM)) as *const Used;

    unsafe {
        core::ptr::write(
            desc,
            Descriptor {
                address: addr_of_mut!(REQ) as u64,
                length: req_len as u32,
                flags: DESC_FLAG_NEXT,
                next: 1,
            },
        );
        core::ptr::write(
            desc.add(1),
            Descriptor {
                address: addr_of_mut!(RESP) as u64,
                length: RESP_BUF as u32,
                flags: DESC_FLAG_WRITE,
                next: 0,
            },
        );
        let position = state.avail;
        (*avail).ring[(position % QUEUE_NUM) as usize] = 0;
        barrier();
        (*avail).index = position.wrapping_add(1);
        state.avail = position.wrapping_add(1);
    }
    barrier();
    write32(base, REG_QUEUE_NOTIFY, 0);

    let expected = state.last_used.wrapping_add(1);
    let mut spins = 0u64;
    loop {
        barrier();
        if unsafe { read_volatile(core::ptr::addr_of!((*used).index)) } == expected {
            break;
        }
        spins += 1;
        if spins > 200_000_000 {
            return None;
        }
        core::hint::spin_loop();
    }
    state.last_used = expected;
    Some(get32(unsafe { &RESP }, 0))
}

/// Save DAIF, mask IRQs, run `f`, restore DAIF. Keeps the shared command
/// buffers safe across console (IRQ) and GUI (syscall) callers.
fn with_irqs_masked<R>(f: impl FnOnce() -> R) -> R {
    let daif: u64;
    unsafe {
        core::arch::asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack));
        core::arch::asm!("msr daifset, #0xf", options(nomem, nostack));
    }
    let result = f();
    unsafe {
        core::arch::asm!("msr daif, {}", in(reg) daif, options(nomem, nostack));
    }
    result
}

/// Scan for a virtio-gpu device and bring up the 2D scanout. `backing` is the
/// guest-physical address of the render target; `stride`/`height` describe it.
pub fn init(width: u32, height: u32, backing: u64, stride: u32) -> bool {
    let mut found = None;
    for slot in 0..SLOTS {
        let base = MMIO_BASE + slot * MMIO_STRIDE;
        if read32(base, REG_MAGIC) != MAGIC {
            continue;
        }
        let version = read32(base, REG_VERSION);
        let id = read32(base, REG_DEVICE_ID);
        if id == DEVICE_GPU && (version == 1 || version == 2) {
            found = Some((base, version));
            break;
        }
    }
    let (base, version) = match found {
        Some(v) => v,
        None => return false,
    };

    unsafe { core::ptr::write_bytes(ctrl_region() as *mut u8, 0, REGION_SIZE) };
    unsafe { core::ptr::write_bytes(cursor_region() as *mut u8, 0, REGION_SIZE) };

    write32(base, REG_STATUS, 0);
    barrier();
    write32(base, REG_STATUS, STATUS_ACK);
    write32(base, REG_STATUS, STATUS_ACK | STATUS_DRIVER);

    if version == 1 {
        write32(base, REG_GUEST_PAGE_SIZE, 4096);
        write32(base, REG_DRIVER_FEATURES, 0); // 2D only, no VirGL
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

    if !setup_queue(base, version, 0, ctrl_region(), QUEUE_NUM) {
        return false;
    }
    // Cursor queue (1): virtio-gpu expects it ready even if unused.
    let _ = setup_queue(base, version, 1, cursor_region(), QUEUE_NUM);

    let mut final_status = STATUS_ACK | STATUS_DRIVER | STATUS_DRIVER_OK;
    if version == 2 {
        final_status |= STATUS_FEATURES_OK;
    }
    barrier();
    write32(base, REG_STATUS, final_status);

    let ok = with_irqs_masked(|| {
        let mut state = STATE.lock();
        state.base = base;
        state.version = version;
        state.avail = 0;
        state.last_used = 0;
        state.width = width;
        state.height = height;
        state.resource_id = 1;

        // CREATE_2D resource.
        let req = unsafe { &mut REQ };
        header(req, CMD_RESOURCE_CREATE_2D);
        put32(req, CTRL_HDR, 1); // resource_id
        put32(req, CTRL_HDR + 4, FORMAT_B8G8R8X8);
        put32(req, CTRL_HDR + 8, width);
        put32(req, CTRL_HDR + 12, height);
        let r = command(&mut state, CTRL_HDR + 16);
        if r != Some(RESP_OK_NODATA) {
            println!("virtio-gpu  : CREATE_2D rejected (resp {:?})", r);
            return false;
        }

        // ATTACH_BACKING: page-sized entries (spec-compliant DMA mapping).
        let total = stride * height;
        let nr = (total + 4095) / 4096;
        header(req, CMD_RESOURCE_ATTACH_BACKING);
        put32(req, CTRL_HDR, 1); // resource_id
        put32(req, CTRL_HDR + 4, nr); // nr_entries
        let mut off = CTRL_HDR + 8;
        let mut a = backing;
        let mut remaining = total;
        for _ in 0..nr {
            let chunk = remaining.min(4096);
            put64(req, off, a);
            put32(req, off + 8, chunk);
            put32(req, off + 12, 0);
            off += 16;
            a += chunk as u64;
            remaining -= chunk;
        }
        let r = command(&mut state, off);
        if r != Some(RESP_OK_NODATA) {
            println!("virtio-gpu  : ATTACH_BACKING rejected (resp {:?})", r);
            return false;
        }

        // SET_SCANOUT 0 -> resource 1.
        header(req, CMD_SET_SCANOUT);
        put32(req, CTRL_HDR, 0); // r.x
        put32(req, CTRL_HDR + 4, 0); // r.y
        put32(req, CTRL_HDR + 8, width); // r.width
        put32(req, CTRL_HDR + 12, height); // r.height
        put32(req, CTRL_HDR + 16, 0); // scanout_id
        put32(req, CTRL_HDR + 20, 1); // resource_id
        let r = command(&mut state, CTRL_HDR + 24);
        if r != Some(RESP_OK_NODATA) {
            println!("virtio-gpu  : SET_SCANOUT rejected (resp {:?})", r);
            return false;
        }

        state.ready = true;
        true
    });

    ok
}

pub fn ready() -> bool {
    STATE.lock().ready
}

/// Transfer a dirty rectangle of the backing to the host resource and flush it
/// to the scanout. Coordinates are pixels; `stride_px` is the backing stride.
pub fn present(x0: u32, y0: u32, x1: u32, y1: u32, stride_px: u32) {
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    with_irqs_masked(|| {
        let mut state = STATE.lock();
        if !state.ready {
            return;
        }
        let w = x1 - x0;
        let h = y1 - y0;
        let offset = ((y0 * stride_px + x0) * 4) as u64;
        let req = unsafe { &mut REQ };

        // TRANSFER_TO_HOST_2D.
        header(req, CMD_TRANSFER_TO_HOST_2D);
        put32(req, CTRL_HDR, x0);
        put32(req, CTRL_HDR + 4, y0);
        put32(req, CTRL_HDR + 8, w);
        put32(req, CTRL_HDR + 12, h);
        put64(req, CTRL_HDR + 16, offset);
        put32(req, CTRL_HDR + 24, state.resource_id);
        put32(req, CTRL_HDR + 28, 0);
        if command(&mut state, CTRL_HDR + 32) != Some(RESP_OK_NODATA) {
            return;
        }

        // RESOURCE_FLUSH.
        header(req, CMD_RESOURCE_FLUSH);
        put32(req, CTRL_HDR, x0);
        put32(req, CTRL_HDR + 4, y0);
        put32(req, CTRL_HDR + 8, w);
        put32(req, CTRL_HDR + 12, h);
        put32(req, CTRL_HDR + 16, state.resource_id);
        put32(req, CTRL_HDR + 20, 0);
        command(&mut state, CTRL_HDR + 24);
    });
}

/// Number of scanouts reported by the device (for diagnostics).
pub fn display_info() -> Option<(u32, u32)> {
    with_irqs_masked(|| {
        let mut state = STATE.lock();
        if !state.ready {
            return None;
        }
        let req = unsafe { &mut REQ };
        header(req, CMD_GET_DISPLAY_INFO);
        if command(&mut state, CTRL_HDR) != Some(RESP_OK_DISPLAY_INFO) {
            return None;
        }
        let resp = unsafe { &RESP };
        // first scanout rect: enabled at +16, width at +8, height at +12
        let w = get32(resp, CTRL_HDR + 8);
        let h = get32(resp, CTRL_HDR + 12);
        Some((w, h))
    })
}