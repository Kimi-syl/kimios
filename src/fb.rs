//! QEMU ramfb framebuffer and text console.
//!
//! The `ramfb` device exposes an `etc/ramfb` fw_cfg file. Writing a small
//! configuration struct through the fw_cfg DMA interface tells QEMU where the
//! pixel buffer lives and how it is laid out. The buffer is ordinary RAM, so
//! drawing is just a store to memory; QEMU picks up the change on refresh.
//!
//! Once configured, every console byte written through the UART is mirrored
//! here as an 8x8 glyph. The console scrolls one text row at a time.

use crate::font::FONT8X8;
use crate::fwcfg;
use crate::println;
use core::ptr::addr_of;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

pub const WIDTH: usize = 480;
pub const HEIGHT: usize = 800;
pub const STRIDE: usize = WIDTH * 4;

const COLS: usize = WIDTH / 8;
const ROWS: usize = HEIGHT / 8;

const COLOR_BG: u32 = 0x0010_141c;
const COLOR_FG: u32 = 0x00d0_e4f0;

/// DRM fourcc `XR24` (`DRM_FORMAT_XRGB8888`) in the endianness QEMU compares.
const FOURCC_XRGB8888: u32 = 0x3432_5258;

#[repr(align(4096))]
struct Framebuffer([u32; WIDTH * HEIGHT]);

static mut FRAMEBUFFER: Framebuffer = Framebuffer([COLOR_BG; WIDTH * HEIGHT]);

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

/// Configure ramfb and start the text console.
///
/// Returns `false` when the device is absent (no `-device ramfb`), leaving the
/// rest of the boot unaffected.
pub fn init() -> bool {
    let Some(selector) = fwcfg::find_file("etc/ramfb") else {
        println!("ramfb       : not configured (no etc/ramfb in fw_cfg)");
        return false;
    };

    let address = addr_of!(FRAMEBUFFER) as u64;

    // QEMU_PACKED RAMFBCfg: addr, fourcc, flags, width, height, stride.
    let mut config = [0u8; 28];
    config[0..8].copy_from_slice(&address.to_be_bytes());
    config[8..12].copy_from_slice(&FOURCC_XRGB8888.to_be_bytes());
    config[12..16].copy_from_slice(&0u32.to_be_bytes());
    config[16..20].copy_from_slice(&(WIDTH as u32).to_be_bytes());
    config[20..24].copy_from_slice(&(HEIGHT as u32).to_be_bytes());
    config[24..28].copy_from_slice(&(STRIDE as u32).to_be_bytes());

    if !fwcfg::write_file(selector, &config) {
        println!("ramfb       : fw_cfg DMA write failed");
        return false;
    }

    {
        let mut screen = SCREEN.lock();
        screen.column = 0;
        screen.row = 0;
        buffer().fill(COLOR_BG);
    }
    ENABLED.store(true, Ordering::Release);

    println!(
        "ramfb       : {}x{} XRGB8888 at {:#x} ({} KiB, {}x{} text)",
        WIDTH,
        HEIGHT,
        address,
        (STRIDE * HEIGHT) / 1024,
        COLS,
        ROWS
    );
    true
}

fn draw_glyph(fb: &mut [u32], column: usize, row: usize, byte: u8) {
    let glyph = if (0x20..=0x7e).contains(&byte) {
        &FONT8X8[(byte - 0x20) as usize]
    } else {
        &FONT8X8[0]
    };

    let x = column * 8;
    let y = row * 8;
    for (dy, bits) in glyph.iter().enumerate() {
        let line = (y + dy) * WIDTH + x;
        for dx in 0..8 {
            fb[line + dx] = if bits & (1 << dx) != 0 {
                COLOR_FG
            } else {
                COLOR_BG
            };
        }
    }
}

fn scroll(fb: &mut [u32]) {
    fb.copy_within(8 * WIDTH.., 0);
    let last = (ROWS - 1) * 8 * WIDTH;
    fb[last..].fill(COLOR_BG);
}

fn advance(screen: &mut Screen, fb: &mut [u32]) {
    screen.column = 0;
    if screen.row + 1 == ROWS {
        scroll(fb);
    } else {
        screen.row += 1;
    }
}

fn putc_locked(screen: &mut Screen, fb: &mut [u32], byte: u8) {
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
            if screen.column == COLS {
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
    Some((WIDTH, HEIGHT, STRIDE, FOURCC_XRGB8888))
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
    Some((addr_of!(FRAMEBUFFER) as u64, STRIDE, HEIGHT))
}

/// Enable or disable mirroring console text (GUI mode). Re-enabling clears
/// the pixels a GUI left behind and restarts the text cursor at the top.
pub fn set_console(enabled: bool) {
    CONSOLE.store(enabled, Ordering::Release);
    if enabled && ENABLED.load(Ordering::Acquire) {
        let mut screen = SCREEN.lock();
        for pixel in buffer() {
            *pixel = COLOR_BG;
        }
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

    let x1 = x.saturating_add(width).min(WIDTH);
    let y1 = y.saturating_add(height).min(HEIGHT);
    if x >= x1 || y >= y1 {
        return true;
    }

    let fb = buffer();
    let color = color & 0x00ff_ffff;
    for row in y..y1 {
        fb[row * WIDTH + x..row * WIDTH + x1].fill(color);
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
