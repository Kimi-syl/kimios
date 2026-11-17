//! On-screen QWERTY keyboard for the text-entry apps (Notes, Messages,
//! Terminal, Browser URL). Drawn in the fixed 480x800 app space, docked at
//! the bottom of the screen. Key taps are translated to Linux keycodes and
//! fed through `apps::key`, so text editing stays in one place.

use crate::apps;
use crate::draw::Screen;

const SURFACE_HI: u32 = 0x001e_1e25;
const SURFACE_MAX: u32 = 0x0027_272f;
const OUTLINE: u32 = 0x0035_3540;
const TEXT: u32 = 0x00f2_f2f6;
const MUTED: u32 = 0x0093_93a0;
const RED: u32 = 0x00e0_352b;
const RED_DIM: u32 = 0x005c_1a16;
const PANEL: u32 = 0x000a_0a0f;

/// Top of the keyboard panel in app-space coordinates.
pub const TOP: i64 = 452;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// Linux keycode, dispatched with the current shift state.
    Char(u16),
    Shift,
    Hide,
    /// Always sends '?' (keycode 53 with shift).
    Question,
}

const ROW_Y: [i64; 5] = [468, 526, 588, 650, 712];
const ROW_H: [i64; 5] = [52, 56, 56, 56, 56];

const ROW0: [Key; 10] = [
    Key::Char(2), Key::Char(3), Key::Char(4), Key::Char(5), Key::Char(6),
    Key::Char(7), Key::Char(8), Key::Char(9), Key::Char(10), Key::Char(11),
];
const ROW1: [Key; 10] = [
    Key::Char(16), Key::Char(17), Key::Char(18), Key::Char(19), Key::Char(20),
    Key::Char(21), Key::Char(22), Key::Char(23), Key::Char(24), Key::Char(25),
];
const ROW2: [Key; 9] = [
    Key::Char(30), Key::Char(31), Key::Char(32), Key::Char(33), Key::Char(34),
    Key::Char(35), Key::Char(36), Key::Char(37), Key::Char(38),
];
const ROW3: [Key; 9] = [
    Key::Shift, Key::Char(44), Key::Char(45), Key::Char(46), Key::Char(47),
    Key::Char(48), Key::Char(49), Key::Char(50), Key::Char(14),
];
const ROW4: [Key; 6] = [
    Key::Hide, Key::Question, Key::Char(51), Key::Char(57), Key::Char(52), Key::Char(28),
];

fn row_keys(row: usize) -> &'static [Key] {
    match row {
        0 => &ROW0,
        1 => &ROW1,
        2 => &ROW2,
        3 => &ROW3,
        _ => &ROW4,
    }
}

fn key_rect(row: usize, index: usize) -> (i64, i64, i64, i64) {
    let y = ROW_Y[row];
    let h = ROW_H[row];
    let (x, w) = match row {
        0 | 1 => (7 + index as i64 * 47, 43),
        2 => (8 + index as i64 * 52, 48),
        3 => match index {
            0 => (6, 60),
            8 => (413, 60),
            _ => (70 + (index as i64 - 1) * 49, 45),
        },
        _ => {
            const X: [i64; 6] = [6, 66, 114, 162, 362, 410];
            const W: [i64; 6] = [56, 44, 44, 196, 44, 64];
            (X[index], W[index])
        }
    };
    (x, y, w, h)
}

/// The key under an app-space point, if any.
pub fn hit(point: (i64, i64)) -> Option<Key> {
    if point.1 < TOP {
        return None;
    }
    for row in 0..5usize {
        for (index, key) in row_keys(row).iter().enumerate() {
            let (x, y, w, h) = key_rect(row, index);
            if point.0 >= x && point.0 < x + w && point.1 >= y && point.1 < y + h {
                return Some(*key);
            }
        }
    }
    None
}

pub fn draw(screen: &Screen, shift: bool, pressed: Option<(i64, i64)>) {
    screen.round_rect(0, TOP, 480, 800 - TOP, 16, PANEL);
    screen.round_rect(220, TOP + 5, 40, 4, 2, 0x0050_505a);

    for row in 0..5usize {
        for (index, key) in row_keys(row).iter().enumerate() {
            let (x, y, w, h) = key_rect(row, index);
            let is_pressed = pressed.map_or(false, |p| {
                p.0 >= x && p.0 < x + w && p.1 >= y && p.1 < y + h
            });
            let mut color = match key {
                Key::Shift if shift => RED_DIM,
                Key::Char(28) => RED,
                Key::Shift | Key::Hide | Key::Question | Key::Char(14) => SURFACE_MAX,
                _ => SURFACE_HI,
            };
            if is_pressed {
                color = OUTLINE;
            }
            screen.round_rect(x, y, w, h, 12, color);
            let cx = x + w / 2;
            let cy = y + h / 2;
            match key {
                Key::Char(14) => draw_backspace(screen, cx, cy, TEXT),
                Key::Shift => draw_shift(screen, cx, cy, if shift { RED } else { TEXT }),
                Key::Hide => {
                    screen.line(cx - 9, cy - 4, cx, cy + 5, 2, MUTED);
                    screen.line(cx, cy + 5, cx + 9, cy - 4, 2, MUTED);
                }
                Key::Char(57) => screen.text_centered(cx, cy - 5, "space", 1, MUTED),
                Key::Char(28) => screen.text_centered(cx, cy - 5, "ENTER", 1, TEXT),
                Key::Question => screen.text_centered(cx, cy - 8, "?", 2, TEXT),
                Key::Char(code) => {
                    if let Some(byte) = apps::key_glyph(*code, shift) {
                        let label = [byte];
                        if let Ok(text) = core::str::from_utf8(&label) {
                            screen.text_centered(cx, cy - 8, text, 2, TEXT);
                        }
                    }
                }
            }
        }
    }
}

fn draw_backspace(screen: &Screen, cx: i64, cy: i64, color: u32) {
    screen.line(cx + 11, cy - 9, cx - 1, cy - 9, 2, color);
    screen.line(cx - 1, cy - 9, cx - 11, cy, 2, color);
    screen.line(cx - 11, cy, cx - 1, cy + 9, 2, color);
    screen.line(cx - 1, cy + 9, cx + 11, cy + 9, 2, color);
    screen.line(cx + 11, cy - 9, cx + 11, cy + 9, 2, color);
    screen.line(cx + 2, cy - 4, cx + 8, cy + 4, 2, color);
    screen.line(cx + 8, cy - 4, cx + 2, cy + 4, 2, color);
}

fn draw_shift(screen: &Screen, cx: i64, cy: i64, color: u32) {
    screen.line(cx, cy - 10, cx - 9, cy, 2, color);
    screen.line(cx, cy - 10, cx + 9, cy, 2, color);
    screen.line(cx - 9, cy, cx - 4, cy, 2, color);
    screen.line(cx + 4, cy, cx + 9, cy, 2, color);
    screen.line(cx - 4, cy, cx - 4, cy + 9, 2, color);
    screen.line(cx + 4, cy, cx + 4, cy + 9, 2, color);
    screen.line(cx - 4, cy + 9, cx + 4, cy + 9, 2, color);
}
