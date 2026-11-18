//! Storage for an installed bitmap font (the Marketplace "Custom" font slot).
//!
//! Downloadable font packages use the compact `.kfont` bitmap format (magic
//! "KFONT001", em, 95 glyph records, then an 8-bit alpha atlas) — small enough
//! to live in a fixed static buffer without an allocator. The built-in atlas
//! typefaces (NType82 / Ndot / Mono / Pixel) stay in `font_data`; this holds the
//! most recently applied downloaded font.

use crate::font_data::{Font, Glyph};
use libkimi as k;

const ZERO: Glyph = Glyph { advance: 0, x0: 0, y0: 0, w: 0, h: 0, offset: 0 };

static mut GLYPHS: [Glyph; 95] = [ZERO; 95];
static mut ALPHA: [u8; 8192] = [0; 8192];
static mut ALPHA_LEN: usize = 0;
static mut VALID: bool = false;
static mut NAME: [u8; 24] = [0; 24];
static mut NAME_LEN: usize = 0;
static mut FONT: Font = Font {
    em: 8,
    glyphs: unsafe { &*core::ptr::addr_of!(GLYPHS) },
    alpha: &[],
};

pub fn valid() -> bool { unsafe { VALID } }

pub fn name() -> &'static str {
    unsafe { core::str::from_utf8(&NAME[..NAME_LEN]).unwrap_or("Custom") }
}

pub fn font() -> Option<&'static Font> {
    unsafe { if VALID { Some(&*core::ptr::addr_of!(FONT)) } else { None } }
}

fn hex4(b: &[u8]) -> u16 {
    let mut v = 0u16;
    for &c in b.iter().take(4) {
        let d = match c { b'0'..=b'9' => c - b'0', b'a'..=b'f' => c - b'a' + 10, b'A'..=b'F' => c - b'A' + 10, _ => 0 };
        v = (v << 4) | d as u16;
    }
    v
}

/// Load a `.kfont` file into the custom slot. Returns true on success.
pub fn load(path: &str, label: &str) -> bool {
    let Some((_, size)) = k::stat(path) else { return false };
    let Some(h) = k::open(path) else { return false };
    let mut buf = [0u8; 8192 + 2048];
    let want = (size as usize).min(buf.len());
    let n = k::read_file(h, 0, &mut buf[..want]);
    k::close(h);
    let d = &buf[..n];
    if d.len() < 12 || &d[..8] != b"KFONT001" { return false; }
    let em = u16::from_le_bytes([d[8], d[9]]);
    let count = u16::from_le_bytes([d[10], d[11]]) as usize;
    if count != 95 || em == 0 { return false; }
    let glyph_bytes = 95 * 16;
    if d.len() < 12 + glyph_bytes { return false; }

    unsafe {
        for i in 0..95 {
            let o = 12 + i * 16;
            GLYPHS[i] = Glyph {
                advance: u16::from_le_bytes([d[o], d[o + 1]]),
                x0: i16::from_le_bytes([d[o + 2], d[o + 3]]),
                y0: i16::from_le_bytes([d[o + 4], d[o + 5]]),
                w: u16::from_le_bytes([d[o + 6], d[o + 7]]),
                h: u16::from_le_bytes([d[o + 8], d[o + 9]]),
                offset: u32::from_le_bytes([d[o + 10], d[o + 11], d[o + 12], d[o + 13]]),
            };
        }
        let alpha = &d[12 + glyph_bytes..];
        let alen = alpha.len().min(ALPHA.len());
        // validate offsets fit
        for i in 0..95 {
            let g = &GLYPHS[i];
            if g.w > 0 && g.h > 0 && (g.offset as usize + (g.w as usize * g.h as usize)) > alen {
                return false;
            }
        }
        ALPHA[..alen].copy_from_slice(&alpha[..alen]);
        ALPHA_LEN = alen;
        FONT.em = em;
        FONT.glyphs = &*core::ptr::addr_of!(GLYPHS);
        FONT.alpha = core::slice::from_raw_parts(ALPHA.as_ptr(), alen);
        let lb = label.as_bytes();
        NAME_LEN = lb.len().min(NAME.len());
        NAME[..NAME_LEN].copy_from_slice(&lb[..NAME_LEN]);
        VALID = true;
    }
    let _ = hex4; // (reserved for hex variant parsing)
    true
}

/// Write the built-in Pixel font out as a `.kfont` (used to seed the repo asset).
pub fn export_pixel(path: &str) -> bool {
    let src = &crate::font_data::PIXEL;
    let mut buf = [0u8; 12 + 95 * 16 + 6080];
    buf[..8].copy_from_slice(b"KFONT001");
    buf[8..10].copy_from_slice(&src.em.to_le_bytes());
    buf[10..12].copy_from_slice(&95u16.to_le_bytes());
    for i in 0..95 {
        let g = &src.glyphs[i];
        let o = 12 + i * 16;
        buf[o..o + 2].copy_from_slice(&g.advance.to_le_bytes());
        buf[o + 2..o + 4].copy_from_slice(&g.x0.to_le_bytes());
        buf[o + 4..o + 6].copy_from_slice(&g.y0.to_le_bytes());
        buf[o + 6..o + 8].copy_from_slice(&g.w.to_le_bytes());
        buf[o + 8..o + 10].copy_from_slice(&g.h.to_le_bytes());
        buf[o + 10..o + 14].copy_from_slice(&g.offset.to_le_bytes());
    }
    let alen = src.alpha.len().min(6080);
    buf[12 + 95 * 16..12 + 95 * 16 + alen].copy_from_slice(&src.alpha[..alen]);
    let total = 12 + 95 * 16 + alen;
    let Some(h) = k::create(path) else { return false };
    let w = k::write_file(h, 0, &buf[..total]);
    k::close(h);
    w == total
}
