#![no_std]
#![no_main]

//! Minimal GUI: two overlapping windows, a clickable button and a mouse
//! cursor, drawn entirely from user space through the mapped framebuffer.
//! ESC exits; the text console returns when the process does.

use core::panic::PanicInfo;

use libkimi as k;

const DESKTOP: u32 = 0x0010_2030;
const SHADOW: u32 = 0x0008_1018;
const WINDOW: u32 = 0x00d8_dde4;
const TITLE: u32 = 0x0030_58a8;
const BORDER: u32 = 0x0020_2838;
const BUTTON_OFF: u32 = 0x0090_9090;
const BUTTON_ON: u32 = 0x0030_c060;

const CURSOR_WIDTH: usize = 12;
const CURSOR_HEIGHT: usize = 18;
const CURSOR: [u16; CURSOR_HEIGHT] = [
    0b1000_0000_0000,
    0b1100_0000_0000,
    0b1110_0000_0000,
    0b1111_0000_0000,
    0b1111_1000_0000,
    0b1111_1100_0000,
    0b1111_1110_0000,
    0b1111_1111_0000,
    0b1111_1111_1000,
    0b1111_1111_1100,
    0b1111_1100_0000,
    0b1110_1100_0000,
    0b1100_0110_0000,
    0b0000_0110_0000,
    0b0000_0011_0000,
    0b0000_0011_0000,
    0b0000_0001_1000,
    0b0000_0001_1000,
];

struct Screen {
    pixels: *mut u32,
    width: u64,
    height: u64,
    stride: u64,
}

impl Screen {
    fn fill(&self, x: u64, y: u64, width: u64, height: u64, color: u32) {
        let x1 = (x + width).min(self.width);
        let y1 = (y + height).min(self.height);
        for row in y..y1 {
            for column in x..x1 {
                unsafe {
                    self.pixels
                        .add((row * self.stride + column) as usize)
                        .write_volatile(color);
                }
            }
        }
    }

    fn read(&self, x: u64, y: u64) -> u32 {
        unsafe { self.pixels.add((y * self.stride + x) as usize).read_volatile() }
    }

    fn restore(&self, x: u64, y: u64, buffer: &[u32]) {
        for row in 0..CURSOR_HEIGHT as u64 {
            for column in 0..CURSOR_WIDTH as u64 {
                let index = (row * CURSOR_WIDTH as u64 + column) as usize;
                unsafe {
                    self.pixels
                        .add(((y + row) * self.stride + x + column) as usize)
                        .write_volatile(buffer[index]);
                }
            }
        }
    }

    fn save(&self, x: u64, y: u64, buffer: &mut [u32]) {
        for row in 0..CURSOR_HEIGHT as u64 {
            for column in 0..CURSOR_WIDTH as u64 {
                let index = (row * CURSOR_WIDTH as u64 + column) as usize;
                buffer[index] = self.read(x + column, y + row);
            }
        }
    }

    fn cursor(&self, x: u64, y: u64) {
        for row in 0..CURSOR_HEIGHT {
            for column in 0..CURSOR_WIDTH {
                if CURSOR[row] & (1 << (CURSOR_WIDTH - 1 - column)) != 0 {
                    unsafe {
                        self.pixels
                            .add(
                                ((y + row as u64) * self.stride
                                    + x
                                    + column as u64) as usize,
                            )
                            .write_volatile(0x0000_0000);
                    }
                }
            }
        }
    }
}

fn window(screen: &Screen, x: u64, y: u64, width: u64, height: u64) {
    screen.fill(x + 6, y + 6, width, height, SHADOW);
    screen.fill(x, y, width, height, BORDER);
    screen.fill(x + 2, y + 2, width - 4, 24, TITLE);
    screen.fill(x + 2, y + 26, width - 4, height - 28, WINDOW);
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let Some((width, height, stride, _)) = k::fb_info() else {
        k::print("gui: no framebuffer\n");
        k::exit(1);
    };
    let Some(base) = k::fb_map() else {
        k::print("gui: fb_map failed\n");
        k::exit(1);
    };

    let screen = Screen {
        pixels: base as *mut u32,
        width,
        height,
        stride: stride / 4,
    };

    let mut button_on = false;
    let mut cursor_x = width / 2;
    let mut cursor_y = height / 2;
    let mut saved = [0u32; CURSOR_WIDTH * CURSOR_HEIGHT];

    screen.fill(0, 0, width, height, DESKTOP);
    window(&screen, 320, 140, 260, 200);
    draw_front(&screen, button_on);
    screen.save(cursor_x, cursor_y, &mut saved);
    screen.cursor(cursor_x, cursor_y);

    loop {
        let Some(event) = k::input_wait() else {
            k::print("gui: input error\n");
            k::exit(1);
        };

        match (event.kind, event.code) {
            (k::EV_ABS, k::ABS_X) => {
                cursor_x = scale(event.value, width);
                redraw_cursor(&screen, &mut saved, &mut cursor_x, &mut cursor_y, width, height, None);
            }
            (k::EV_ABS, k::ABS_Y) => {
                cursor_y = scale(event.value, height);
                redraw_cursor(&screen, &mut saved, &mut cursor_x, &mut cursor_y, width, height, None);
            }
            (k::EV_KEY, 1) if event.value != 0 => break,
            (k::EV_KEY, k::BTN_LEFT) if event.value != 0 => {
                if cursor_x >= 90 && cursor_x < 270 && cursor_y >= 150 && cursor_y < 210 {
                    button_on = !button_on;
                    // Restore under the cursor, redraw the scene, then the
                    // cursor, so it stays on top.
                    screen.restore(cursor_x, cursor_y, &saved);
                    draw_front(&screen, button_on);
                    screen.save(cursor_x, cursor_y, &mut saved);
                    screen.cursor(cursor_x, cursor_y);
                }
            }
            _ => {}
        }
    }

    // Leave a clean scene without the cursor before exiting.
    screen.restore(cursor_x, cursor_y, &saved);
    k::exit(0);
}

fn redraw_cursor(
    screen: &Screen,
    saved: &mut [u32],
    cursor_x: &mut u64,
    cursor_y: &mut u64,
    width: u64,
    height: u64,
    _unused: Option<()>,
) {
    screen.restore(*cursor_x, *cursor_y, saved);
    *cursor_x = (*cursor_x).min(width - CURSOR_WIDTH as u64);
    *cursor_y = (*cursor_y).min(height - CURSOR_HEIGHT as u64);
    screen.save(*cursor_x, *cursor_y, saved);
    screen.cursor(*cursor_x, *cursor_y);
}

fn draw_front(screen: &Screen, button_on: bool) {
    let x = 60;
    let y = 60;
    let width = 360;
    let height = 240;
    window(screen, x, y, width, height);
    screen.fill(
        x + 30,
        y + 90,
        180,
        60,
        if button_on { BUTTON_ON } else { BUTTON_OFF },
    );
    screen.fill(x + 30, y + 90, 180, 2, BORDER);
    screen.fill(x + 30, y + 148, 180, 2, BORDER);
}

fn scale(value: u32, dimension: u64) -> u64 {
    (value as u64 * (dimension - CURSOR_WIDTH as u64)) / 32767
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    k::exit(2)
}
