#![no_std]
#![no_main]

//! Maps the framebuffer and draws directly, proving user-space pixel access is
//! fast enough for a GUI compositor.

use core::panic::PanicInfo;

use libkimi as k;

const BG: u32 = 0x0010_2040;
const RED: u32 = 0x00ff_3030;
const GREEN: u32 = 0x0030_ff30;
const BLUE: u32 = 0x0030_30ff;
const YELLOW: u32 = 0x00ff_ff30;
const WHITE: u32 = 0x00f0_f0f0;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let Some((width, height, stride, format)) = k::fb_info() else {
        fail("no framebuffer");
    };
    let Some(base) = k::fb_map() else {
        fail("fb_map failed");
    };
    let pixels_per_row = stride / 4;
    let pixels = base as *mut u32;

    k::print("gfxtest: ");
    k::print_u64(width);
    k::print("x");
    k::print_u64(height);
    k::print(" stride ");
    k::print_u64(stride);
    k::print(" format ");
    k::print_u64(format);
    k::print("\n");

    let start = k::uptime_ms();
    for frame in 0..8u32 {
        for y in 0..height {
            for x in 0..width {
                let red = ((x * 255 / width) as u32 + frame * 16) & 0xff;
                let green = (y * 255 / height) as u32;
                let blue = ((x + y) as u32) & 0xff;
                unsafe {
                    pixels
                        .add((y * pixels_per_row + x) as usize)
                        .write_volatile((red << 16) | (green << 8) | blue);
                }
            }
        }
    }
    let draw_ms = k::uptime_ms().saturating_sub(start);

    draw_rect(pixels, pixels_per_row, 0, 0, width, height, BG);
    draw_rect(pixels, pixels_per_row, 40, 120, 120, 120, RED);
    draw_rect(pixels, pixels_per_row, 200, 120, 120, 120, GREEN);
    draw_rect(pixels, pixels_per_row, 360, 120, 120, 120, BLUE);
    draw_rect(pixels, pixels_per_row, 520, 120, 80, 240, YELLOW);
    // A one-pixel white frame to check exact edges.
    draw_rect(pixels, pixels_per_row, 40, 260, 560, 1, WHITE);
    draw_rect(pixels, pixels_per_row, 40, 380, 560, 1, WHITE);

    k::print("gfxtest: 8 frames in ");
    k::print_u64(draw_ms);
    k::print(" ms, pattern drawn; press enter\n");
    let mut line = [0u8; 8];
    k::read_line(&mut line);
    k::exit(0);
}

fn draw_rect(
    pixels: *mut u32,
    stride_pixels: u64,
    x: u64,
    y: u64,
    width: u64,
    height: u64,
    color: u32,
) {
    for row in y..y + height {
        for column in x..x + width {
            unsafe {
                pixels
                    .add((row * stride_pixels + column) as usize)
                    .write_volatile(color);
            }
        }
    }
}

fn fail(message: &str) -> ! {
    k::print("gfxtest: FAIL: ");
    k::print(message);
    k::print("\n");
    k::exit(1)
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    k::exit(2)
}
