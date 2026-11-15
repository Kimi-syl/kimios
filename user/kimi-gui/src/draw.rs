//! Software renderer for the KimiOS phone UI at 1080p.
//!
//! The UI is laid out in a 480x800 logical coordinate space; every primitive
//! maps to the 1080x1920 framebuffer with integer scaling. Text is sampled
//! with 4x supersampling so glyph edges are anti-aliased instead of blocky.

use crate::font::FONT8X8;

const LOGICAL_WIDTH: i64 = 480;
const LOGICAL_HEIGHT: i64 = 800;

pub struct Screen {
    pixels: *mut u32,
    physical_width: i64,
    physical_height: i64,
    stride: i64,
}

impl Screen {
    pub fn new(base: u64, width: u64, height: u64, stride_bytes: u64) -> Self {
        Self {
            pixels: base as *mut u32,
            physical_width: width as i64,
            physical_height: height as i64,
            stride: (stride_bytes / 4) as i64,
        }
    }

    pub fn width(&self) -> i64 {
        LOGICAL_WIDTH
    }

    pub fn height(&self) -> i64 {
        LOGICAL_HEIGHT
    }

    pub fn pixel(&self, x: i64, y: i64) -> u32 {
        let px = self.map_x(x).clamp(0, self.physical_width - 1);
        let py = self.map_y(y).clamp(0, self.physical_height - 1);
        unsafe { self.pixels.add((py * self.stride + px) as usize).read() }
    }

    fn map_x(&self, x: i64) -> i64 {
        x * self.physical_width / LOGICAL_WIDTH
    }

    fn map_y(&self, y: i64) -> i64 {
        y * self.physical_height / LOGICAL_HEIGHT
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

    /// Anti-aliased ASCII text. Returns the logical x just past the text.
    pub fn text(&self, x: i64, y: i64, text: &str, scale: i64, color: u32) -> i64 {
        let origin_x = self.map_x(x);
        let origin_y = self.map_y(y);
        let advance = (self.map_x(x + 9 * scale) - origin_x).max(1);
        let cell_width = (self.map_x(x + 8 * scale) - origin_x).max(1);
        let cell_height = (self.map_x(x + 8 * scale) - origin_x).max(1);
        let mut cursor = origin_x;

        if scale == 1 {
            for byte in text.bytes() {
                if byte < 0x20 || byte > 0x7e {
                    cursor += advance;
                    continue;
                }
                let glyph = &FONT8X8[(byte - 0x20) as usize];
                for (sy, bits) in glyph.iter().enumerate() {
                    for sx in 0..8 {
                        if bits & (1 << sx) == 0 {
                            continue;
                        }
                        let x0 = cursor + sx as i64 * cell_width / 8;
                        let x1 = cursor + (sx as i64 + 1) * cell_width / 8;
                        let y0 = origin_y + sy as i64 * cell_height / 8;
                        let y1 = origin_y + (sy as i64 + 1) * cell_height / 8;
                        self.phys_fill(x0, y0, (x1 - x0).max(1), (y1 - y0).max(1), color);
                    }
                }
                cursor += advance;
            }
            return x + text.len() as i64 * 9 * scale - scale;
        }

        let step_u = ((8 << 8) << 8) / cell_width;
        let step_v = ((8 << 8) << 8) / cell_height;

        for byte in text.bytes() {
            if byte < 0x20 || byte > 0x7e {
                cursor += advance;
                continue;
            }
            let glyph = &FONT8X8[(byte - 0x20) as usize];
            let mut v = 0i64;
            for row in 0..cell_height {
                let mut u = 0i64;
                for column in 0..cell_width {
                    let mut hits = 0;
                    for (du, dv) in [(64, 64), (192, 64), (64, 192), (192, 192)] {
                        let su = ((u + du * 256) >> 16).min(7) as usize;
                        let sv = ((v + dv * 256) >> 16).min(7) as usize;
                        if glyph[sv] & (1 << su) != 0 {
                            hits += 1;
                        }
                    }
                    if hits > 0 {
                        let coverage = hits * 255 / 4;
                        self.phys_blend(cursor + column, origin_y + row, color, coverage);
                    }
                    u += step_u;
                }
                v += step_v;
            }
            cursor += advance;
        }
        x + text.len() as i64 * 9 * scale - scale
    }

    pub fn text_width(text: &str, scale: i64) -> i64 {
        text.len() as i64 * 9 * scale - scale
    }

    pub fn text_centered(&self, cx: i64, y: i64, text: &str, scale: i64, color: u32) {
        let x = cx - Self::text_width(text, scale) / 2;
        self.text(x, y, text, scale, color);
    }

    pub fn text_dots(&self, x: i64, y: i64, text: &str, cell: i64, radius: i64, color: u32) -> i64 {
        let mut cursor = x;
        for byte in text.bytes() {
            if byte == b':' {
                self.circle(cursor + 3 * cell, y + 2 * cell + cell / 2, radius, color);
                self.circle(cursor + 3 * cell, y + 6 * cell + cell / 2, radius, color);
                cursor += 9 * cell;
                continue;
            }
            if byte < 0x20 || byte > 0x7e {
                cursor += 8 * cell;
                continue;
            }
            let glyph = &FONT8X8[(byte - 0x20) as usize];
            for (row, bits) in glyph.iter().enumerate() {
                for column in 0..8 {
                    if bits & (1 << column) != 0 {
                        self.circle(
                            cursor + column as i64 * cell + cell / 2,
                            y + row as i64 * cell + cell / 2,
                            radius,
                            color,
                        );
                    }
                }
            }
            cursor += 9 * cell;
        }
        cursor
    }

    pub fn dots_text_width(text: &str, cell: i64) -> i64 {
        text.len() as i64 * 9 * cell - cell
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