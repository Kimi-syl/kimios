//! Storage for an installed font (the Marketplace "Custom" font slot).
//!
//! Supports two compact bitmap formats, small enough to live in fixed static
//! buffers without an allocator:
//!
//! - `.kfont` v1 ("KFONT001"): em + 95 ASCII glyph records + 8-bit alpha atlas.
//! - `.kfont` v2 ("KFONT002"): em + ascent/descent + arbitrary Unicode glyphs
//!   referenced by a sorted (codepoint -> index) cmap + a larger alpha atlas.
//!   Produced from real outline fonts (TTF/OTF) by `tools/kfont_convert.py`.
//!
//! The built-in atlas typefaces (NType82 / Ndot / Mono / Pixel) stay in
//! `font_data`; this holds the most recently applied downloaded font.

use crate::font_data::{Font, Glyph};
use libkimi as k;

const ZERO: Glyph = Glyph { advance: 0, x0: 0, y0: 0, w: 0, h: 0, offset: 0 };
const MAX_GLYPHS: usize = 512;
const ALPHA_CAP: usize = 131072;
const META_CAP: usize = 16384;

static mut GLYPHS: [Glyph; MAX_GLYPHS] = [ZERO; MAX_GLYPHS];
static mut CMAP: [(u32, u16); MAX_GLYPHS] = [(0, 0); MAX_GLYPHS];
static mut ALPHA: [u8; ALPHA_CAP] = [0; ALPHA_CAP];
static mut META: [u8; META_CAP] = [0; META_CAP];
static mut GLYPH_COUNT: usize = 0;
static mut CMAP_COUNT: usize = 0;
static mut ALPHA_LEN: usize = 0;
static mut VALID: bool = false;
static mut NAME: [u8; 24] = [0; 24];
static mut NAME_LEN: usize = 0;
static mut FONT: Font = Font {
    em: 8,
    ascent: 8,
    descent: 0,
    glyphs: &[],
    cmap: &[],
    alpha: &[],
};

pub fn valid() -> bool { unsafe { VALID } }

pub fn name() -> &'static str {
    unsafe { core::str::from_utf8(&NAME[..NAME_LEN]).unwrap_or("Custom") }
}

pub fn font() -> Option<&'static Font> {
    unsafe { if VALID { Some(&*core::ptr::addr_of!(FONT)) } else { None } }
}

fn rd_u16(d: &[u8], o: usize) -> u16 { u16::from_le_bytes([d[o], d[o + 1]]) }
fn rd_i16(d: &[u8], o: usize) -> i16 { i16::from_le_bytes([d[o], d[o + 1]]) }
fn rd_u32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

/// Load a `.kfont` (v1 or v2) file into the custom slot. Returns true on success.
pub fn load(path: &str, label: &str) -> bool {
    let Some((_, size)) = k::stat(path) else { return false };
    let size = size as usize;
    if size < 32 { return false; }
    let Some(h) = k::open(path) else { return false };

    // Read the header first to size the metadata region.
    let mut hdr = [0u8; 32];
    if k::read_file(h, 0, &mut hdr) < 32 { k::close(h); return false; }
    let v2 = &hdr[..8] == b"KFONT002";
    let v1 = &hdr[..8] == b"KFONT001";
    if !v1 && !v2 { k::close(h); return false; }

    let (em, ascent, descent, gc, cc, meta_len, atlas_len) = if v2 {
        let em = rd_u16(&hdr, 8);
        let ascent = rd_i16(&hdr, 10);
        let descent = rd_i16(&hdr, 12);
        let gc = rd_u16(&hdr, 16) as usize;
        let cc = rd_u16(&hdr, 18) as usize;
        let alen = rd_u32(&hdr, 24) as usize;
        let ml = 32 + gc * 16 + cc * 8;
        if em == 0 || gc == 0 || gc > MAX_GLYPHS || cc > MAX_GLYPHS || ml > META_CAP {
            k::close(h); return false;
        }
        (em, ascent, descent, gc, cc, ml, alen)
    } else {
        let em = rd_u16(&hdr, 8);
        let count = rd_u16(&hdr, 10) as usize;
        if em == 0 || count != 95 { k::close(h); return false; }
        let ml = 12 + 95 * 16;
        (em, em as i16, 0i16, 95, 0, ml, size.saturating_sub(ml))
    };

    if meta_len + atlas_len > size || atlas_len > ALPHA_CAP { k::close(h); return false; }

    // Read metadata (glyph records + cmap) into BSS, then the atlas into BSS.
    let meta = unsafe { &mut *core::ptr::addr_of_mut!(META) };
    if k::read_file(h, 0, &mut meta[..meta_len]) < meta_len { k::close(h); return false; }
    let alen = atlas_len.min(ALPHA_CAP);
    let alpha = unsafe { &mut *core::ptr::addr_of_mut!(ALPHA) };
    let got = k::read_file(h, meta_len, &mut alpha[..alen]);
    k::close(h);
    if got < alen { return false; }

    unsafe {
        let glyphs = &mut *core::ptr::addr_of_mut!(GLYPHS);
        let cmap = &mut *core::ptr::addr_of_mut!(CMAP);
        let goff = if v2 { 32 } else { 12 };
        for i in 0..gc {
            let o = goff + i * 16;
            glyphs[i] = Glyph {
                advance: rd_u16(meta, o),
                x0: rd_i16(meta, o + 2),
                y0: rd_i16(meta, o + 4),
                w: rd_u16(meta, o + 6),
                h: rd_u16(meta, o + 8),
                offset: rd_u32(meta, o + 10),
            };
            let g = &glyphs[i];
            if g.w > 0 && g.h > 0 && (g.offset as usize + g.w as usize * g.h as usize) > alen {
                return false;
            }
        }
        if v2 {
            let coff = 32 + gc * 16;
            for i in 0..cc {
                let o = coff + i * 8;
                cmap[i] = (rd_u32(meta, o), rd_u16(meta, o + 4));
                if cmap[i].1 as usize >= gc { return false; }
            }
        }
        GLYPH_COUNT = gc;
        CMAP_COUNT = cc;
        ALPHA_LEN = alen;
        FONT.em = em;
        FONT.ascent = ascent;
        FONT.descent = descent;
        FONT.glyphs = core::slice::from_raw_parts(glyphs.as_ptr(), gc);
        FONT.cmap = core::slice::from_raw_parts(cmap.as_ptr(), cc);
        FONT.alpha = core::slice::from_raw_parts(alpha.as_ptr(), alen);
        let lb = label.as_bytes();
        NAME_LEN = lb.len().min(NAME.len());
        NAME[..NAME_LEN].copy_from_slice(&lb[..NAME_LEN]);
        VALID = true;
    }
    true
}

/// Write the built-in Pixel font out as a `.kfont` v1 (seeds the repo asset).
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
