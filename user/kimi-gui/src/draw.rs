//! Tiny software renderer for the KimiOS phone UI.
//!
//! Everything is drawn directly into the mapped framebuffer with clipped,
//! integer-only primitives: rectangles, rounded rectangles, circles, rings,
//! lines and 8x8 text.

use crate::font::FONT8X8;

pub struct Screen {
    pixels: *mut u32,
    width: i64,
    height: i64,
    stride: i64,
}

impl Screen {
    pub fn new(base: u64, width: u64, height: u64, stride_bytes: u64) -> Self {
        Self {
            pixels: base as *mut u32,
            width: width as i64,
            height: height as i64,
            stride: (stride_bytes / 4) as i64,
        }
    }

    pub fn width(&self) -> i64 {
        self.width
    }

    pub fn height(&self) -> i64 {
        self.height
    }

    #[inline]
    fn put(&self, x: i64, y: i64, color: u32) {
        if x >= 0 && y >= 0 && x < self.width && y < self.height {
            unsafe {
                self.pixels
                    .add((y * self.stride + x) as usize)
                    .write_volatile(color);
            }
        }
    }

    pub fn fill(&self, x: i64, y: i64, width: i64, height: i64, color: u32) {
        let x1 = (x + width).min(self.width);
        let y1 = (y + height).min(self.height);
        let x = x.max(0);
        let y = y.max(0);
        for row in y..y1 {
            for column in x..x1 {
                self.put(column, row, color);
            }
        }
    }

    /// Vertical gradient, one solid row at a time.
    pub fn gradient(&self, x: i64, y: i64, width: i64, height: i64, top: u32, bottom: u32) {
        if height <= 0 {
            return;
        }
        for row in 0..height {
            let color = lerp_color(top, bottom, (row * 255 / height.max(1)) as u32);
            self.fill(x, y + row, width, 1, color);
        }
    }

    pub fn round_rect(&self, x: i64, y: i64, width: i64, height: i64, radius: i64, color: u32) {
        let radius = radius.min(width / 2).min(height / 2).max(0);
        for row in 0..height {
            let dy = if row < radius {
                radius - row
            } else if row >= height - radius {
                row - (height - radius - 1)
            } else {
                0
            };
            let inset = if dy > 0 {
                radius - isqrt((radius * radius - dy * dy).max(0) as u64) as i64
            } else {
                0
            };
            self.fill(
                x + inset,
                y + row,
                (width - inset * 2).max(0),
                1,
                color,
            );
        }
    }

    /// Blend a solid rectangle over the existing pixels.
    pub fn fill_blend(&self, x: i64, y: i64, width: i64, height: i64, color: u32, alpha: u32) {
        let x1 = (x + width).min(self.width);
        let y1 = (y + height).min(self.height);
        let x = x.max(0);
        let y = y.max(0);
        let inverse = 255 - alpha.min(255);
        for row in y..y1 {
            for column in x..x1 {
                if column >= 0 && row >= 0 && column < self.width && row < self.height {
                    let index = (row * self.stride + column) as usize;
                    let destination = unsafe { self.pixels.add(index).read_volatile() };
                    let mix = |shift: u32| {
                        let front = (color >> shift) & 0xff;
                        let back = (destination >> shift) & 0xff;
                        ((front * alpha + back * inverse) / 255) & 0xff
                    };
                    let blended = (mix(16) << 16) | (mix(8) << 8) | mix(0);
                    unsafe {
                        self.pixels.add(index).write_volatile(blended);
                    }
                }
            }
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
        let radius = radius.min(width / 2).min(height / 2).max(0);
        for row in 0..height {
            let dy = if row < radius {
                radius - row
            } else if row >= height - radius {
                row - (height - radius - 1)
            } else {
                0
            };
            let inset = if dy > 0 {
                radius - isqrt((radius * radius - dy * dy).max(0) as u64) as i64
            } else {
                0
            };
            self.fill_blend(
                x + inset,
                y + row,
                (width - inset * 2).max(0),
                1,
                color,
                alpha,
            );
        }
    }

    pub fn circle(&self, cx: i64, cy: i64, radius: i64, color: u32) {
        if radius <= 0 {
            return;
        }
        let r2 = radius * radius;
        for dy in -radius..=radius {
            let dx = isqrt((r2 - dy * dy).max(0) as u64) as i64;
            self.fill(cx - dx, cy + dy, dx * 2 + 1, 1, color);
        }
    }

    pub fn ring(&self, cx: i64, cy: i64, radius: i64, thickness: i64, color: u32) {
        if radius <= 0 {
            return;
        }
        let inner = (radius - thickness).max(0);
        let outer_r2 = radius * radius;
        let inner_r2 = inner * inner;
        for dy in -radius..=radius {
            let dx = isqrt((outer_r2 - dy * dy).max(0) as u64) as i64;
            let idx = if dy.abs() <= inner {
                isqrt((inner_r2 - dy * dy).max(0) as u64) as i64
            } else {
                0
            };
            // Left and right arcs (avoid filling the middle).
            self.fill(cx - dx, cy + dy, (dx - idx).max(0), 1, color);
            self.fill(cx + idx + 1, cy + dy, (dx - idx).max(0), 1, color);
            if idx == 0 {
                self.fill(cx - dx, cy + dy, dx * 2 + 1, 1, color);
            }
        }
    }

    pub fn line(&self, x0: i64, y0: i64, x1: i64, y1: i64, thickness: i64, color: u32) {
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut error = dx + dy;
        let (mut x, mut y) = (x0, y0);
        let half = thickness / 2;
        loop {
            self.fill(x - half, y - half, thickness, thickness, color);
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

    /// Draw ASCII text; returns the x coordinate just past the last glyph.
    pub fn text(&self, x: i64, y: i64, text: &str, scale: i64, color: u32) -> i64 {
        let mut cursor = x;
        for byte in text.bytes() {
            if byte < 0x20 || byte > 0x7e {
                cursor += 8 * scale;
                continue;
            }
            let glyph = &FONT8X8[(byte - 0x20) as usize];
            for (row, bits) in glyph.iter().enumerate() {
                for column in 0..8 {
                    if bits & (1 << column) != 0 {
                        self.fill(
                            cursor + column * scale,
                            y + row as i64 * scale,
                            scale,
                            scale,
                            color,
                        );
                    }
                }
            }
            cursor += 9 * scale;
        }
        cursor
    }

    /// Dot-matrix text: each glyph pixel becomes a round dot of `radius` on a
    /// `cell`-pixel grid, the Nothing OS look for large clocks.
    pub fn text_dots(&self, x: i64, y: i64, text: &str, cell: i64, radius: i64, color: u32) -> i64 {
        let mut cursor = x;
        for byte in text.bytes() {
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

    pub fn text_width(text: &str, scale: i64) -> i64 {
        text.len() as i64 * 9 * scale - scale
    }

    pub fn text_centered(&self, cx: i64, y: i64, text: &str, scale: i64, color: u32) {
        let x = cx - Self::text_width(text, scale) / 2;
        self.text(x, y, text, scale, color);
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