//! QEMU ramfb framebuffer and text console.
//!
//! The `ramfb` device exposes an `etc/ramfb` fw_cfg file. Writing a small
//! configuration struct through the fw_cfg DMA interface tells QEMU where the
//! pixel buffer lives and how it is laid out. The buffer is ordinary RAM, so
//! drawing is just a store to memory; QEMU picks up the change on refresh.
//!
//! The resolution defaults to 1080x1920 and can be changed at boot with the
//! `kimios.fb=WIDTHxHEIGHT` kernel argument (parsed from the DTB). The static
//! buffer is sized for the maximum supported resolution.

use crate::font::FONT8X8;
use crate::fwcfg;
use crate::println;
use core::ptr::addr_of;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

pub const MAX_WIDTH: usize = 1440;
pub const MAX_HEIGHT: usize = 2560;

const COLOR_BG: u32 = 0x0010_141c;
const COLOR_FG: u32 = 0x00d0_e4f0;

/// DRM fourcc `XR24` (`DRM_FORMAT_XRGB8888`) in the endianness QEMU compares.
const FOURCC_XRGB8888: u32 = 0x3432_5258;

#[derive(Clone, Copy)]
struct Dims {
    width: usize,
    height: usize,
    stride: usize,
    scale: usize,
    cell: usize,
    cols: usize,
    rows: usize,
}

impl Dims {
    const fn new(width: usize, height: usize) -> Self {
        let scale = if width / 360 < 1 {
            1
        } else if width / 360 > 3 {
            3
        } else {
            width / 360
        };
        let cell = 8 * scale;
        Self {
            width,
            height,
            stride: width * 4,
            scale,
            cell,
            cols: width / cell,
            rows: height / cell,
        }
    }
}

static DIMS: Mutex<Dims> = Mutex::new(Dims::new(1080, 1920));

#[repr(align(4096))]
struct Framebuffer([u32; MAX_WIDTH * MAX_HEIGHT]);

static mut FRAMEBUFFER: Framebuffer = Framebuffer([COLOR_BG; MAX_WIDTH * MAX_HEIGHT]);

/// Offscreen back buffer. A GUI renders here, then `present()` copies the
/// finished frame to FRAMEBUFFER in one fast pass, so the display never shows
/// a half-drawn frame (which otherwise appears as black flicker on scroll).
/// Zero-initialized (BSS) so it does not bloat the kernel image.
static mut BACK: Framebuffer = Framebuffer([0; MAX_WIDTH * MAX_HEIGHT]);

struct Screen {
    column: usize,
    row: usize,
}

static SCREEN: Mutex<Screen> = Mutex::new(Screen { column: 0, row: 0 });
static ENABLED: AtomicBool = AtomicBool::new(false);
/// When false, console text is not mirrored (a GUI owns the framebuffer).
static CONSOLE: AtomicBool = AtomicBool::new(true);

#[allow(static_mut_refs)]
fn buffer() -> &'static mut [u32] {
    unsafe { &mut FRAMEBUFFER.0 }
}

fn dims() -> Dims {
    *DIMS.lock()
}

/// Select the framebuffer resolution before `init`. Values are clamped to the
/// supported range (16..=1440 x 16..=2560).
pub fn configure(width: usize, height: usize) {
    let width = width.clamp(16, MAX_WIDTH);
    let height = height.clamp(16, MAX_HEIGHT);
    *DIMS.lock() = Dims::new(width, height);
}

fn clear_active(fb: &mut [u32], d: &Dims) {
    for row in 0..d.height {
        fb[row * d.width..(row + 1) * d.width].fill(COLOR_BG);
    }
}

/// Configure ramfb and start the text console.
///
/// Returns `false` when the device is absent (no `-device ramfb`), leaving the
/// rest of the boot unaffected.
pub fn init() -> bool {
    let Some(selector) = fwcfg::find_file("etc/ramfb") else {
        println!("ramfb       : not configured (no etc/ramfb in fw_cfg)");
        return false;
    };

    let d = dims();
    let address = addr_of!(FRAMEBUFFER) as u64;

    // QEMU_PACKED RAMFBCfg: addr, fourcc, flags, width, height, stride.
    let mut config = [0u8; 28];
    config[0..8].copy_from_slice(&address.to_be_bytes());
    config[8..12].copy_from_slice(&FOURCC_XRGB8888.to_be_bytes());
    config[12..16].copy_from_slice(&0u32.to_be_bytes());
    config[16..20].copy_from_slice(&(d.width as u32).to_be_bytes());
    config[20..24].copy_from_slice(&(d.height as u32).to_be_bytes());
    config[24..28].copy_from_slice(&(d.stride as u32).to_be_bytes());

    if !fwcfg::write_file(selector, &config) {
        println!("ramfb       : fw_cfg DMA write failed");
        return false;
    }

    {
        let mut screen = SCREEN.lock();
        screen.column = 0;
        screen.row = 0;
        clear_active(buffer(), &d);
    }
    ENABLED.store(true, Ordering::Release);

    println!(
        "ramfb       : {}x{} XRGB8888 at {:#x} ({} KiB, {}x{} text)",
        d.width,
        d.height,
        address,
        (d.stride * d.height) / 1024,
        d.cols,
        d.rows
    );
    true
}

fn draw_glyph(fb: &mut [u32], column: usize, row: usize, byte: u8) {
    let d = dims();
    let glyph = if (0x20..=0x7e).contains(&byte) {
        &FONT8X8[(byte - 0x20) as usize]
    } else {
        &FONT8X8[0]
    };

    let x = column * d.cell;
    let y = row * d.cell;
    for (dy, bits) in glyph.iter().enumerate() {
        for dx in 0..8 {
            let color = if bits & (1 << dx) != 0 {
                COLOR_FG
            } else {
                COLOR_BG
            };
            for sy in 0..d.scale {
                let line = (y + dy * d.scale + sy) * d.width + x + dx * d.scale;
                for sx in 0..d.scale {
                    fb[line + sx] = color;
                }
            }
        }
    }
}

fn scroll(fb: &mut [u32]) {
    let d = dims();
    let active = d.height * d.width;
    fb.copy_within(d.cell * d.width..active, 0);
    let last = (d.rows - 1) * d.cell * d.width;
    fb[last..active].fill(COLOR_BG);
}

fn advance(screen: &mut Screen, fb: &mut [u32]) {
    let d = dims();
    screen.column = 0;
    if screen.row + 1 == d.rows {
        scroll(fb);
    } else {
        screen.row += 1;
    }
}

fn putc_locked(screen: &mut Screen, fb: &mut [u32], byte: u8) {
    let d = dims();
    match byte {
        b'\n' => advance(screen, fb),
        b'\r' => screen.column = 0,
        0x08 => {
            if screen.column > 0 {
                screen.column -= 1;
                draw_glyph(fb, screen.column, screen.row, b' ');
            }
        }
        _ => {
            draw_glyph(fb, screen.column, screen.row, byte);
            screen.column += 1;
            if screen.column == d.cols {
                advance(screen, fb);
            }
        }
    }
}

/// Mirror a byte from the console stream onto the framebuffer.
pub fn putc(byte: u8) {
    if !ENABLED.load(Ordering::Acquire) || !CONSOLE.load(Ordering::Acquire) {
        return;
    }
    let mut screen = SCREEN.lock();
    putc_locked(&mut screen, buffer(), byte);
}

/// Framebuffer geometry as `(width, height, stride, format)`.
pub fn info() -> Option<(usize, usize, usize, u32)> {
    if !ENABLED.load(Ordering::Acquire) {
        return None;
    }
    let d = dims();
    Some((d.width, d.height, d.stride, FOURCC_XRGB8888))
}

/// Whether a framebuffer was configured at boot.
pub fn ready() -> bool {
    ENABLED.load(Ordering::Acquire)
}

/// Physical address, stride in bytes and height of the framebuffer.
pub fn mapping() -> Option<(u64, usize, usize)> {
    if !ENABLED.load(Ordering::Acquire) {
        return None;
    }
    let d = dims();
    Some((addr_of!(FRAMEBUFFER) as u64, d.stride, d.height))
}

/// Physical address of the offscreen back buffer (same layout as the front).
pub fn back_mapping() -> Option<(u64, usize, usize)> {
    if !ENABLED.load(Ordering::Acquire) {
        return None;
    }
    let d = dims();
    Some((addr_of!(BACK) as u64, d.stride, d.height))
}

/// Copy a rectangle of the back buffer to the live framebuffer. Copying only
/// the changed region (dirty rectangle) instead of the whole 1080p frame keeps
/// the GUI fast. Coordinates are physical pixels, clamped to the active area.
pub fn present(x0: usize, y0: usize, x1: usize, y1: usize) {
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }
    let d = dims();
    let x0 = x0.min(d.width);
    let x1 = x1.min(d.width);
    let y0 = y0.min(d.height);
    let y1 = y1.min(d.height);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let stride_px = d.stride / 4;
    unsafe {
        let src = addr_of!(BACK) as *const u32;
        let dst = addr_of!(FRAMEBUFFER) as *mut u32;
        if x0 == 0 && x1 == d.width {
            // Full-width rows are contiguous: one copy.
            let start = y0 * stride_px;
            let count = (y1 - y0) * stride_px;
            core::ptr::copy_nonoverlapping(src.add(start), dst.add(start), count);
        } else {
            for y in y0..y1 {
                let off = y * stride_px + x0;
                core::ptr::copy_nonoverlapping(src.add(off), dst.add(off), x1 - x0);
            }
        }
    }
}

/// Enable or disable mirroring console text (GUI mode). Re-enabling clears
/// the pixels a GUI left behind and restarts the text cursor at the top.
pub fn set_console(enabled: bool) {
    CONSOLE.store(enabled, Ordering::Release);
    if enabled && ENABLED.load(Ordering::Acquire) {
        let mut screen = SCREEN.lock();
        let d = dims();
        clear_active(buffer(), &d);
        screen.column = 0;
        screen.row = 0;
    }
}

/// Whether console text (and therefore keyboard-to-line-editor input) is
/// currently owned by the kernel console rather than a GUI.
pub fn console_enabled() -> bool {
    CONSOLE.load(Ordering::Acquire)
}

/// Fill a clipped rectangle with an `0x00RRGGBB` colour.
///
/// Used by the `fb_fill` syscall; the console cursor is not moved, so text may
/// subsequently draw over the rectangle.
pub fn fill_rect(x: usize, y: usize, width: usize, height: usize, color: u32) -> bool {
    if !ENABLED.load(Ordering::Acquire) {
        return false;
    }
    let _screen = SCREEN.lock();
    let d = dims();

    let x1 = x.saturating_add(width).min(d.width);
    let y1 = y.saturating_add(height).min(d.height);
    if x >= x1 || y >= y1 {
        return true;
    }

    let fb = buffer();
    let color = color & 0x00ff_ffff;
    for row in y..y1 {
        fb[row * d.width + x..row * d.width + x1].fill(color);
    }
    true
}

/// Erase the previous cell. The UART echoes backspace-space-backspace; the
/// framebuffer only needs the final state.
pub fn backspace() {
    if !ENABLED.load(Ordering::Acquire) || !CONSOLE.load(Ordering::Acquire) {
        return;
    }
    let mut screen = SCREEN.lock();
    if screen.column > 0 {
        screen.column -= 1;
        draw_glyph(buffer(), screen.column, screen.row, b' ');
    }
}
