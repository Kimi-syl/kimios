//! Runtime appearance: theme palette + UI font selection.
//!
//! The GUI's colours used to be hardcoded constants duplicated across modules.
//! They are now read from a global atomic palette (`BLACK()`, `SURFACE()`,
//! `TEXT()`, `RED()`, …) so a theme can be applied at runtime — from the
//! Marketplace app or Settings — and persisted to `/disk/config.cfg`. The
//! default theme reproduces the original Nothing-OS dark look exactly.

#![allow(non_snake_case)]

use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use crate::font_data::{self, Font};
use libkimi as k;

struct Pal {
    black: AtomicU32,
    surface: AtomicU32,
    surface_hi: AtomicU32,
    surface_max: AtomicU32,
    outline: AtomicU32,
    text: AtomicU32,
    muted: AtomicU32,
    accent: AtomicU32,
    accent_dim: AtomicU32,
    green: AtomicU32,
    panel: AtomicU32,
}

static P: Pal = Pal {
    black: AtomicU32::new(0x0000_0000),
    surface: AtomicU32::new(0x0014_1419),
    surface_hi: AtomicU32::new(0x001e_1e25),
    surface_max: AtomicU32::new(0x0027_272f),
    outline: AtomicU32::new(0x0035_3540),
    text: AtomicU32::new(0x00f2_f2f6),
    muted: AtomicU32::new(0x0093_93a0),
    accent: AtomicU32::new(0x00e0_352b),
    accent_dim: AtomicU32::new(0x005c_1a16),
    green: AtomicU32::new(0x0060_d18a),
    panel: AtomicU32::new(0x000a_0a0f),
};

static THEME: AtomicUsize = AtomicUsize::new(0);
static FONT: AtomicUsize = AtomicUsize::new(0);
static CUSTOM: AtomicUsize = AtomicUsize::new(0);
static mut CUSTOM_NAME: [u8; 24] = [0; 24];
static mut CUSTOM_NAME_LEN: usize = 0;

pub fn BLACK() -> u32 { P.black.load(Ordering::Relaxed) }
pub fn SURFACE() -> u32 { P.surface.load(Ordering::Relaxed) }
pub fn SURFACE_HI() -> u32 { P.surface_hi.load(Ordering::Relaxed) }
pub fn SURFACE_MAX() -> u32 { P.surface_max.load(Ordering::Relaxed) }
pub fn OUTLINE() -> u32 { P.outline.load(Ordering::Relaxed) }
pub fn TEXT() -> u32 { P.text.load(Ordering::Relaxed) }
pub fn MUTED() -> u32 { P.muted.load(Ordering::Relaxed) }
pub fn RED() -> u32 { P.accent.load(Ordering::Relaxed) }
pub fn RED_DIM() -> u32 { P.accent_dim.load(Ordering::Relaxed) }
pub fn GREEN() -> u32 { P.green.load(Ordering::Relaxed) }
pub fn PANEL() -> u32 { P.panel.load(Ordering::Relaxed) }

/// A built-in theme preset (also the layout used by downloadable theme files).
pub struct ThemeDef {
    pub name: &'static str,
    pub black: u32,
    pub surface: u32,
    pub surface_hi: u32,
    pub surface_max: u32,
    pub outline: u32,
    pub text: u32,
    pub muted: u32,
    pub accent: u32,
    pub accent_dim: u32,
    pub green: u32,
    pub panel: u32,
}

pub static THEMES: &[ThemeDef] = &[
    ThemeDef { name: "Nothing Dark", black: 0x00000000, surface: 0x141419, surface_hi: 0x1e1e25, surface_max: 0x27272f, outline: 0x353540, text: 0xf2f2f6, muted: 0x9393a0, accent: 0xe0352b, accent_dim: 0x5c1a16, green: 0x60d18a, panel: 0x0a0a0f },
    ThemeDef { name: "Graphite Light", black: 0xfafafa, surface: 0xf0f0f4, surface_hi: 0xe4e4ea, surface_max: 0xd6d6de, outline: 0xbcbcc8, text: 0x16161c, muted: 0x5e5e6a, accent: 0xe0352b, accent_dim: 0xf3cfcb, green: 0x1f8a4c, panel: 0xe8e8ee },
    ThemeDef { name: "Ocean", black: 0x0a1018, surface: 0x101c28, surface_hi: 0x182838, surface_max: 0x203448, outline: 0x2e4458, text: 0xeef4fa, muted: 0x8aa0b4, accent: 0x2b8fe0, accent_dim: 0x143a5c, green: 0x35c1a0, panel: 0x081018 },
    ThemeDef { name: "Forest", black: 0x0a120a, surface: 0x121e12, surface_hi: 0x1a2a1a, surface_max: 0x223622, outline: 0x304830, text: 0xeef6ee, muted: 0x8aa88a, accent: 0x46b060, accent_dim: 0x1c4024, green: 0x7ad08a, panel: 0x081008 },
    ThemeDef { name: "Sunset", black: 0x140e08, surface: 0x201610, surface_hi: 0x2c2018, surface_max: 0x382a20, outline: 0x4c3a2c, text: 0xfaf2ea, muted: 0xb49a86, accent: 0xe07b2b, accent_dim: 0x5c3414, green: 0xd0a035, panel: 0x100a06 },
    ThemeDef { name: "Lavender", black: 0x100e18, surface: 0x181428, surface_hi: 0x221c38, surface_max: 0x2c2448, outline: 0x3e3458, text: 0xf2eefa, muted: 0xa094b4, accent: 0x9b6de0, accent_dim: 0x3a285c, green: 0x6d9be0, panel: 0x0c0a14 },
    ThemeDef { name: "Mono", black: 0x080808, surface: 0x121212, surface_hi: 0x1c1c1c, surface_max: 0x262626, outline: 0x3a3a3a, text: 0xf0f0f0, muted: 0x8a8a8a, accent: 0xf0f0f0, accent_dim: 0x404040, green: 0xc0c0c0, panel: 0x0a0a0a },
];

pub fn theme_count() -> usize { THEMES.len() }
pub fn theme_name(i: usize) -> &'static str { THEMES[i].name }
pub fn active_theme() -> usize { THEME.load(Ordering::Relaxed) }

pub fn apply_theme(i: usize) {
    let t = &THEMES[i.min(THEMES.len() - 1)];
    P.black.store(t.black, Ordering::Relaxed);
    P.surface.store(t.surface, Ordering::Relaxed);
    P.surface_hi.store(t.surface_hi, Ordering::Relaxed);
    P.surface_max.store(t.surface_max, Ordering::Relaxed);
    P.outline.store(t.outline, Ordering::Relaxed);
    P.text.store(t.text, Ordering::Relaxed);
    P.muted.store(t.muted, Ordering::Relaxed);
    P.accent.store(t.accent, Ordering::Relaxed);
    P.accent_dim.store(t.accent_dim, Ordering::Relaxed);
    P.green.store(t.green, Ordering::Relaxed);
    P.panel.store(t.panel, Ordering::Relaxed);
    THEME.store(i, Ordering::Relaxed);
    CUSTOM.store(0, Ordering::Relaxed);
}

pub fn is_custom() -> bool { CUSTOM.load(Ordering::Relaxed) == 1 }

pub fn custom_name() -> &'static str {
    unsafe { core::str::from_utf8(&CUSTOM_NAME[..CUSTOM_NAME_LEN]).unwrap_or("Custom") }
}

/// Apply a palette parsed from a downloaded theme file (11 hex colours).
pub fn apply_custom(v: &[u32; 11], name: &str) {
    P.black.store(v[0], Ordering::Relaxed);
    P.surface.store(v[1], Ordering::Relaxed);
    P.surface_hi.store(v[2], Ordering::Relaxed);
    P.surface_max.store(v[3], Ordering::Relaxed);
    P.outline.store(v[4], Ordering::Relaxed);
    P.text.store(v[5], Ordering::Relaxed);
    P.muted.store(v[6], Ordering::Relaxed);
    P.accent.store(v[7], Ordering::Relaxed);
    P.accent_dim.store(v[8], Ordering::Relaxed);
    P.green.store(v[9], Ordering::Relaxed);
    P.panel.store(v[10], Ordering::Relaxed);
    unsafe {
        let nb = name.as_bytes();
        CUSTOM_NAME_LEN = nb.len().min(CUSTOM_NAME.len());
        CUSTOM_NAME[..CUSTOM_NAME_LEN].copy_from_slice(&nb[..CUSTOM_NAME_LEN]);
    }
    CUSTOM.store(1, Ordering::Relaxed);
    THEME.store(usize::MAX, Ordering::Relaxed);
}

/// Load a `.theme` file: 11 hex colours (RRGGBB), one per line, in palette order
/// black, surface, surface_hi, surface_max, outline, text, muted, accent,
/// accent_dim, green, panel. Applies it as a custom theme named `label`.
pub fn load_theme_file(path: &str, label: &str) -> bool {
    let Some((_, size)) = k::stat(path) else { return false };
    let Some(h) = k::open(path) else { return false };
    let mut buf = [0u8; 256];
    let want = (size as usize).min(buf.len());
    let n = k::read_file(h, 0, &mut buf[..want]);
    k::close(h);
    let mut v = [0u32; 11];
    let mut idx = 0usize;
    let mut cur = [0u8; 6];
    let mut cn = 0usize;
    let mut started = false;
    for &b in &buf[..n] {
        let hex = matches!(b, b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F');
        if hex {
            if cn < 6 { cur[cn] = b; cn += 1; }
            started = true;
        } else if started {
            if cn == 6 && idx < 11 { v[idx] = hex6(&cur); idx += 1; }
            cn = 0; started = false;
            if idx >= 11 { break; }
        }
    }
    if started && cn == 6 && idx < 11 { v[idx] = hex6(&cur); idx += 1; }
    if idx < 11 { return false; }
    apply_custom(&v, label);
    true
}

fn hex6(b: &[u8; 6]) -> u32 {
    let mut v = 0u32;
    for &c in b.iter() {
        let d = match c { b'0'..=b'9' => c - b'0', b'a'..=b'f' => c - b'a' + 10, b'A'..=b'F' => c - b'A' + 10, _ => 0 };
        v = (v << 4) | d as u32;
    }
    v
}

// ---- UI font selection -----------------------------------------------------

pub fn font_count() -> usize { 5 }
pub fn font_name(i: usize) -> &'static str {
    match i {
        1 => "Ndot-55",
        2 => "NType82 Mono",
        3 => "Pixel 8x8",
        4 => crate::font_store::name(),
        _ => "NType82",
    }
}
pub fn active_font() -> usize { FONT.load(Ordering::Relaxed) }
pub fn apply_font(i: usize) { FONT.store(i.min(4), Ordering::Relaxed); }

/// The font used for proportional UI text (`Screen::text`).
pub fn ui_font() -> &'static Font {
    match FONT.load(Ordering::Relaxed) {
        1 => &font_data::DOT,
        2 => &font_data::MONO,
        3 => &font_data::PIXEL,
        4 => crate::font_store::font().unwrap_or(&font_data::SANS),
        _ => &font_data::SANS,
    }
}

// ---- persistence (/disk/config.cfg: "theme=N font=M") ----------------------

const CFG: &str = "/disk/config.cfg";
const CUSTOM_CFG: &str = "/disk/theme_custom.cfg";

pub fn save() {
    let mut buf = [0u8; 48];
    let mut n = 0usize;
    n = put(&mut buf, n, b"theme=");
    if is_custom() {
        save_custom_palette();
        n = put_dec(&mut buf, n, 99); // custom-theme sentinel
    } else {
        n = put_dec(&mut buf, n, THEME.load(Ordering::Relaxed));
    }
    n = put(&mut buf, n, b"\nfont=");
    n = put_dec(&mut buf, n, FONT.load(Ordering::Relaxed));
    n = put(&mut buf, n, b"\n");
    if let Some(h) = k::create(CFG) {
        k::write_file(h, 0, &buf[..n]);
        k::close(h);
    }
}

/// Persist the active custom palette as 11 RRGGBB lines (reloaded via sentinel 99).
fn save_custom_palette() {
    let cols = [
        P.black.load(Ordering::Relaxed), P.surface.load(Ordering::Relaxed),
        P.surface_hi.load(Ordering::Relaxed), P.surface_max.load(Ordering::Relaxed),
        P.outline.load(Ordering::Relaxed), P.text.load(Ordering::Relaxed),
        P.muted.load(Ordering::Relaxed), P.accent.load(Ordering::Relaxed),
        P.accent_dim.load(Ordering::Relaxed), P.green.load(Ordering::Relaxed),
        P.panel.load(Ordering::Relaxed),
    ];
    let mut buf = [0u8; 8 * 11];
    let mut n = 0usize;
    for c in cols {
        n = put_hex6(&mut buf, n, c);
        if n < buf.len() { buf[n] = b'\n'; n += 1; }
    }
    if let Some(h) = k::create(CUSTOM_CFG) {
        k::write_file(h, 0, &buf[..n]);
        k::close(h);
    }
}

fn put_hex6(buf: &mut [u8], n: usize, v: u32) -> usize {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut tmp = [0u8; 6];
    for i in 0..6 {
        tmp[5 - i] = HEX[((v >> (i * 4)) & 0xf) as usize];
    }
    put(buf, n, &tmp)
}

fn put(buf: &mut [u8], n: usize, s: &[u8]) -> usize {
    let take = s.len().min(buf.len() - n);
    buf[n..n + take].copy_from_slice(&s[..take]);
    n + take
}

fn put_dec(buf: &mut [u8], n: usize, mut v: usize) -> usize {
    let mut tmp = [0u8; 20];
    let mut i = tmp.len();
    loop { i -= 1; tmp[i] = b'0' + (v % 10) as u8; v /= 10; if v == 0 { break; } }
    put(buf, n, &tmp[i..])
}

pub fn load() {
    let Some((_, size)) = k::stat(CFG) else { return };
    let Some(h) = k::open(CFG) else { return };
    let mut buf = [0u8; 64];
    let want = (size as usize).min(buf.len());
    let n = k::read_file(h, 0, &mut buf[..want]);
    k::close(h);
    let data = &buf[..n];
    if let Some(t) = field(data, b"theme=") {
        if t == 99 {
            load_theme_file(CUSTOM_CFG, "Custom");
        } else {
            apply_theme(t);
        }
    }
    if let Some(f) = field(data, b"font=") { apply_font(f); }
}

fn field(data: &[u8], key: &[u8]) -> Option<usize> {
    let pos = data.windows(key.len()).position(|w| w == key)?;
    let mut v = 0usize;
    let mut digits = 0;
    for &b in &data[pos + key.len()..] {
        if b < b'0' || b > b'9' { break; }
        v = v * 10 + (b - b'0') as usize;
        digits += 1;
        if digits > 4 { break; }
    }
    if digits == 0 { None } else { Some(v) }
}
