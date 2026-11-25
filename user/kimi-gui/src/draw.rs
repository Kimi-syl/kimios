//! Software renderer for the KimiOS phone UI at 1080p.
//!
//! The UI is laid out in a 480x800 logical coordinate space; every primitive
//! maps to the 1080x1920 framebuffer with integer scaling. Text is sampled
//! with 4x supersampling so glyph edges are anti-aliased instead of blocky.

use crate::font_data::{self, Font};

const LOGICAL_WIDTH: i64 = 480;
const APP_HEIGHT: i64 = 800;

pub struct Screen {
    pixels: *mut u32,
    physical_width: i64,
    physical_height: i64,
    stride: i64,
    logical_height: i64,
    app_space: core::cell::Cell<bool>,
    /// Accumulated dirty bounding box in physical pixels [x0, y0, x1, y1].
    dirty: core::cell::Cell<[i64; 4]>,
}

impl Screen {
    pub fn new(base: u64, width: u64, height: u64, stride_bytes: u64) -> Self {
        let physical_width = width as i64;
        let physical_height = height as i64;
        let logical_height = (LOGICAL_WIDTH * physical_height / physical_width).max(480);
        Self {
            pixels: base as *mut u32,
            physical_width,
            physical_height,
            stride: (stride_bytes / 4) as i64,
            logical_height,
            app_space: core::cell::Cell::new(false),
            dirty: core::cell::Cell::new([i64::MAX, i64::MAX, i64::MIN, i64::MIN]),
        }
    }

    /// Expand the dirty bounding box to include a physical-pixel rectangle.
    fn mark_dirty(&self, x0: i64, y0: i64, x1: i64, y1: i64) {
        let mut d = self.dirty.get();
        if x0 < d[0] {
            d[0] = x0;
        }
        if y0 < d[1] {
            d[1] = y0;
        }
        if x1 > d[2] {
            d[2] = x1;
        }
        if y1 > d[3] {
            d[3] = y1;
        }
        self.dirty.set(d);
    }

    /// Clear the dirty box (call at the start of a frame).
    pub fn reset_dirty(&self) {
        self.dirty.set([i64::MAX, i64::MAX, i64::MIN, i64::MIN]);
    }

    /// The accumulated dirty box, if anything was drawn.
    pub fn dirty_rect(&self) -> Option<[i64; 4]> {
        let d = self.dirty.get();
        if d[0] < d[2] && d[1] < d[3] {
            Some(d)
        } else {
            None
        }
    }

    /// Mark the whole screen dirty (force a full present).
    pub fn mark_full_dirty(&self) {
        self.mark_dirty(0, 0, self.physical_width, self.physical_height);
    }

    /// Raw back-buffer pointer / stride / physical size (for cached blits).
    pub fn back_ptr(&self) -> *mut u32 { self.pixels }
    pub fn stride_pixels(&self) -> i64 { self.stride }
    pub fn physical_size(&self) -> (i64, i64) { (self.physical_width, self.physical_height) }

    /// Apps are laid out in a fixed 480x800 space; on shorter/wider screens
    /// that space is mapped onto the full framebuffer (uniform compression).
    pub fn set_app_space(&self, enabled: bool) {
        self.app_space.set(enabled);
    }

    /// Compress a design-space y into the real logical height (home/lock).
    pub fn sy(&self, y: i64) -> i64 {
        y * self.height() / APP_HEIGHT
    }

    pub fn width(&self) -> i64 {
        LOGICAL_WIDTH
    }

    pub fn height(&self) -> i64 {
        if self.app_space.get() { APP_HEIGHT } else { self.logical_height }
    }

    /// Home/lock logical height, independent of the app-space mode.
    pub fn logical_height(&self) -> i64 {
        self.logical_height
    }

    #[allow(dead_code)]
    pub fn pixel(&self, x: i64, y: i64) -> u32 {
        let px = self.map_x(x).clamp(0, self.physical_width - 1);
        let py = self.map_y(y).clamp(0, self.physical_height - 1);
        unsafe { self.pixels.add((py * self.stride + px) as usize).read() }
    }

    fn map_x(&self, x: i64) -> i64 {
        x * self.physical_width / LOGICAL_WIDTH
    }

    fn map_y(&self, y: i64) -> i64 {
        y * self.physical_height / self.height()
    }

    fn solid_run(&self, start: i64, length: i64, color: u32) {
        if length <= 0 {
            return;
        }
        if length < 16 {
            unsafe {
                let base = self.pixels.add(start as usize);
                for index in 0..length {
                    base.add(index as usize).write(color);
                }
            }
            return;
        }
        let pattern = color as u64 | ((color as u64) << 32);
        let mut index = 0i64;
        unsafe {
            let base = self.pixels.add(start as usize);
            if start % 2 != 0 {
                base.write(color);
                index = 1;
            }
            let pointer = base.add(index as usize) as *mut u64;
            let pairs = (length - index) / 2;
            for pair in 0..pairs {
                pointer.add(pair as usize).write(pattern);
            }
            if (length - index) % 2 == 1 {
                base.add((length - 1) as usize).write(color);
            }
        }
    }

    fn phys_fill(&self, x: i64, y: i64, width: i64, height: i64, color: u32) {
        let x0 = x.clamp(0, self.physical_width);
        let y0 = y.clamp(0, self.physical_height);
        let x1 = (x + width).clamp(0, self.physical_width);
        let y1 = (y + height).clamp(0, self.physical_height);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        self.mark_dirty(x0, y0, x1, y1);
        if x0 == 0 && x1 == self.physical_width && self.stride == self.physical_width {
            self.solid_run(y0 * self.stride, (y1 - y0) * self.stride, color);
            return;
        }
        for row in y0..y1 {
            self.solid_run(row * self.stride + x0, x1 - x0, color);
        }
    }

    fn phys_blend(&self, x: i64, y: i64, color: u32, alpha: u32) {
        if x < 0 || y < 0 || x >= self.physical_width || y >= self.physical_height {
            return;
        }
        let inverse = 255 - alpha.min(255);
        let index = y * self.stride + x;
        unsafe {
            let destination = self.pixels.add(index as usize).read();
            let mix = |shift: u32| {
                let front = (color >> shift) & 0xff;
                let back = (destination >> shift) & 0xff;
                ((front * alpha + back * inverse) / 255) & 0xff
            };
            let blended = (mix(16) << 16) | (mix(8) << 8) | mix(0);
            self.pixels.add(index as usize).write(blended);
        }
    }

    /// Blit an RGBA8 sprite sub-cell (`cw`×`ch` at `sx,sy` inside a `sw`×`sh`
    /// atlas) at logical `(dx,dy)`, nearest-neighbour scaled by `scale`
    /// art-pixels and alpha-blended. `mirror` flips horizontally. Used by the
    /// pet overlay; pixel-art stays crisp because each art pixel maps to an
    /// integer block of physical pixels.
    pub fn sprite(
        &self,
        dx: i64,
        dy: i64,
        scale: i64,
        src: &[u8],
        sw: i64,
        sh: i64,
        sx: i64,
        sy: i64,
        cw: i64,
        ch: i64,
        mirror: bool,
    ) {
        let pscale = (self.physical_width / LOGICAL_WIDTH) * scale.max(1);
        if pscale <= 0 || cw <= 0 || ch <= 0 || sw <= 0 || sh <= 0 {
            return;
        }
        let px0 = self.map_x(dx);
        let py0 = self.map_y(dy);
        self.mark_dirty(px0, py0, px0 + cw * pscale, py0 + ch * pscale);
        for ay in 0..ch {
            let row = sy + ay;
            if row < 0 || row >= sh {
                continue;
            }
            for ax in 0..cw {
                let col = sx + if mirror { cw - 1 - ax } else { ax };
                if col < 0 || col >= sw {
                    continue;
                }
                let o = ((row * sw + col) * 4) as usize;
                if o + 4 > src.len() {
                    continue;
                }
                let a = src[o + 3] as u32;
                if a == 0 {
                    continue;
                }
                let color =
                    ((src[o] as u32) << 16) | ((src[o + 1] as u32) << 8) | src[o + 2] as u32;
                let bx = px0 + ax * pscale;
                let by = py0 + ay * pscale;
                if a >= 255 {
                    self.phys_fill(bx, by, pscale, pscale, color);
                } else {
                    for yy in 0..pscale {
                        for xx in 0..pscale {
                            self.phys_blend(bx + xx, by + yy, color, a);
                        }
                    }
                }
            }
        }
    }

    pub fn fill(&self, x: i64, y: i64, width: i64, height: i64, color: u32) {
        self.phys_fill(
            self.map_x(x),
            self.map_y(y),
            self.map_x(x + width) - self.map_x(x),
            self.map_y(y + height) - self.map_y(y),
            color,
        );
    }

    pub fn gradient(&self, x: i64, y: i64, width: i64, height: i64, top: u32, bottom: u32) {
        if height <= 0 {
            return;
        }
        let x0 = self.map_x(x).max(0);
        let x1 = self.map_x(x + width).min(self.physical_width);
        let y0 = self.map_y(y).max(0);
        let y1 = self.map_y(y + height).min(self.physical_height);
        let total = (y1 - y0).max(1);
        for row in y0..y1 {
            let color = lerp_color(top, bottom, ((row - y0) * 255 / total) as u32);
            self.phys_fill(x0, row, x1 - x0, 1, color);
        }
    }

    pub fn fill_blend(&self, x: i64, y: i64, width: i64, height: i64, color: u32, alpha: u32) {
        let x0 = self.map_x(x);
        let y0 = self.map_y(y);
        let x1 = self.map_x(x + width);
        let y1 = self.map_y(y + height);
        self.mark_dirty(x0, y0, x1, y1);
        for row in y0..y1 {
            for column in x0..x1 {
                self.phys_blend(column, row, color, alpha);
            }
        }
    }

    pub fn round_rect(&self, x: i64, y: i64, width: i64, height: i64, radius: i64, color: u32) {
        let px = self.map_x(x);
        let py = self.map_y(y);
        let pw = self.map_x(x + width) - px;
        let ph = self.map_y(y + height) - py;
        let radius = self.map_x(radius).min(pw / 2).min(ph / 2).max(0);
        let top_rows = radius.min(ph);
        for row in 0..top_rows {
            let dy = radius - row;
            let inset = radius - isqrt(((radius * radius - dy * dy).max(0)) as u64) as i64;
            self.phys_fill(px + inset, py + row, (pw - inset * 2).max(0), 1, color);
        }
        if ph > radius * 2 {
            self.phys_fill(px, py + radius, pw, ph - radius * 2, color);
        }
        for row in ph.saturating_sub(radius)..ph {
            let dy = row - (ph - radius - 1);
            let inset = radius - isqrt(((radius * radius - dy * dy).max(0)) as u64) as i64;
            self.phys_fill(px + inset, py + row, (pw - inset * 2).max(0), 1, color);
        }
    }

    pub fn round_rect_blend(
        &self,
        x: i64,
        y: i64,
        width: i64,
        height: i64,
        radius: i64,
        color: u32,
        alpha: u32,
    ) {
        let px = self.map_x(x);
        let py = self.map_y(y);
        let pw = self.map_x(x + width) - px;
        let ph = self.map_y(y + height) - py;
        self.mark_dirty(px, py, px + pw, py + ph);
        let radius = self.map_x(radius).min(pw / 2).min(ph / 2).max(0);
        for row in 0..ph {
            let dy = if row < radius {
                radius - row
            } else if row >= ph - radius {
                row - (ph - radius - 1)
            } else {
                0
            };
            let inset = if dy > 0 {
                radius - isqrt(((radius * radius - dy * dy).max(0)) as u64) as i64
            } else {
                0
            };
            for column in px + inset..px + pw - inset {
                self.phys_blend(column, py + row, color, alpha);
            }
        }
    }

    pub fn circle(&self, cx: i64, cy: i64, radius: i64, color: u32) {
        if radius <= 0 {
            return;
        }
        let center_x = self.map_x(cx);
        let center_y = self.map_y(cy);
        let rx = self.map_x(radius);
        if rx <= 0 {
            return;
        }
        for dy in -rx..=rx {
            let dx = isqrt(((rx * rx - dy * dy).max(0)) as u64) as i64;
            self.phys_fill(center_x - dx, center_y + dy, dx * 2 + 1, 1, color);
        }
    }

    pub fn ring(&self, cx: i64, cy: i64, radius: i64, thickness: i64, color: u32) {
        if radius <= 0 || thickness <= 0 {
            return;
        }
        let center_x = self.map_x(cx);
        let center_y = self.map_y(cy);
        let rx = self.map_x(radius);
        let inner = self.map_x((radius - thickness).max(0));
        for dy in -rx..=rx {
            let outer_dx = isqrt(((rx * rx - dy * dy).max(0)) as u64) as i64;
            let inner_dx = if dy.abs() <= inner {
                isqrt(((inner * inner - dy * dy).max(0)) as u64) as i64
            } else {
                0
            };
            self.phys_fill(
                center_x - outer_dx,
                center_y + dy,
                (outer_dx - inner_dx).max(0),
                1,
                color,
            );
            self.phys_fill(
                center_x + inner_dx + 1,
                center_y + dy,
                (outer_dx - inner_dx).max(0),
                1,
                color,
            );
            if inner_dx == 0 {
                self.phys_fill(
                    center_x - outer_dx,
                    center_y + dy,
                    outer_dx * 2 + 1,
                    1,
                    color,
                );
            }
        }
    }

    pub fn line(&self, x0: i64, y0: i64, x1: i64, y1: i64, thickness: i64, color: u32) {
        let x0 = self.map_x(x0);
        let y0 = self.map_y(y0);
        let x1 = self.map_x(x1);
        let y1 = self.map_y(y1);
        let thickness = self.map_x(thickness.max(1)).max(1);
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut error = dx + dy;
        let (mut x, mut y) = (x0, y0);
        let half = thickness / 2;
        loop {
            self.phys_fill(x - half, y - half, thickness, thickness, color);
            if x == x1 && y == y1 {
                break;
            }
            let doubled = error * 2;
            if doubled >= dy {
                error += dy;
                x += sx;
            }
            if doubled <= dx {
                error += dx;
                y += sy;
            }
        }
    }

    /// Nothing OS type (NType82), anti-aliased. `scale` keeps the old
    /// 8px-cell layout units; the em size is 11 logical px per scale step.
    pub fn text(&self, x: i64, y: i64, text: &str, scale: i64, color: u32) -> i64 {
        self.font_text(x, y, text, 11 * scale, color, crate::theme::ui_font())
    }

    pub fn text_mono(&self, x: i64, y: i64, text: &str, scale: i64, color: u32) -> i64 {
        self.font_text(x, y, text, 11 * scale, color, &font_data::MONO)
    }

    pub fn font_text(
        &self,
        x: i64,
        y: i64,
        text: &str,
        size: i64,
        color: u32,
        font: &Font,
    ) -> i64 {
        let em = font.em as i64;
        let mut pen = x;
        for ch in text.chars() {
            let Some(glyph) = font.glyph_for(ch as u32) else {
                pen += size * 4 / 10;
                continue;
            };
            if glyph.w > 0 && glyph.h > 0 {
                self.draw_font_glyph(font, glyph, pen, y, size, color);
            }
            pen += glyph.advance as i64 * size / em;
        }
        pen
    }

    fn draw_font_glyph(
        &self,
        font: &Font,
        glyph: &font_data::Glyph,
        pen_x: i64,
        pen_y: i64,
        size: i64,
        color: u32,
    ) {
        let em = font.em as i64;
        let gx = pen_x + glyph.x0 as i64 * size / em;
        let gy = pen_y + glyph.y0 as i64 * size / em;
        let gw = ((glyph.w as i64 * size + em - 1) / em).max(1);
        let gh = ((glyph.h as i64 * size + em - 1) / em).max(1);
        let ox = self.map_x(gx);
        let oy = self.map_y(gy);
        let pw = (self.map_x(gx + gw) - ox).max(1);
        let ph = (self.map_y(gy + gh) - oy).max(1);
        self.mark_dirty(ox, oy, ox + pw, oy + ph);

        let w = glyph.w as i64;
        let h = glyph.h as i64;
        let step_u = (w << 16) / pw;
        let step_v = (h << 16) / ph;
        let base = glyph.offset as usize;

        let mut v = step_v / 2;
        for py in 0..ph {
            let sv = (v >> 16).min(h - 1);
            let sv1 = (sv + 1).min(h - 1);
            let fv = v & 0xffff;
            let mut u = step_u / 2;
            for px in 0..pw {
                let su = (u >> 16).min(w - 1);
                let su1 = (su + 1).min(w - 1);
                let fu = u & 0xffff;
                let a00 = font.alpha[base + (sv * w + su) as usize] as i64;
                let a10 = font.alpha[base + (sv * w + su1) as usize] as i64;
                let a01 = font.alpha[base + (sv1 * w + su) as usize] as i64;
                let a11 = font.alpha[base + (sv1 * w + su1) as usize] as i64;
                let top = a00 + ((a10 - a00) * fu >> 16);
                let bot = a01 + ((a11 - a01) * fu >> 16);
                let alpha = top + ((bot - top) * fv >> 16);
                if alpha > 0 {
                    self.phys_blend(ox + px, oy + py, color, alpha as u32);
                }
                u += step_u;
            }
            v += step_v;
        }
    }

    pub fn text_width(text: &str, scale: i64) -> i64 {
        Self::font_width(text, 11 * scale, crate::theme::ui_font())
    }

    pub fn font_width(text: &str, size: i64, font: &Font) -> i64 {
        let em = font.em as i64;
        let mut width = 0i64;
        for ch in text.chars() {
            match font.glyph_for(ch as u32) {
                Some(g) => width += g.advance as i64 * size / em,
                None => width += size * 4 / 10,
            }
        }
        width
    }

    pub fn text_centered(&self, cx: i64, y: i64, text: &str, scale: i64, color: u32) {
        let x = cx - Self::text_width(text, scale) / 2;
        self.text(x, y, text, scale, color);
    }

    /// Nothing OS dot-matrix type (Ndot-55) for clocks.
    pub fn text_dots(&self, x: i64, y: i64, text: &str, cell: i64, _radius: i64, color: u32) -> i64 {
        self.font_text(x, y, text, cell * 8, color, &font_data::DOT)
    }

    pub fn dots_text_width(text: &str, cell: i64) -> i64 {
        Self::font_width(text, cell * 8, &font_data::DOT)
    }
}

pub fn isqrt(value: u64) -> u64 {
    if value == 0 {
        return 0;
    }
    let mut x = value;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + value / x) / 2;
    }
    x
}

fn lerp_color(top: u32, bottom: u32, t: u32) -> u32 {
    let mix = |shift: u32| {
        let a = (top >> shift) & 0xff;
        let b = (bottom >> shift) & 0xff;
        ((a * (255 - t) + b * t) / 255) & 0xff
    };
    (mix(16) << 16) | (mix(8) << 8) | mix(0)
}