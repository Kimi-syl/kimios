#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libkimi as k;

const BG: u32 = 0x0010_2040;
const RED: u32 = 0x00ff_3030;
const GREEN: u32 = 0x0030_ff30;
const BLUE: u32 = 0x0030_30ff;
const YELLOW: u32 = 0x00ff_ff30;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let Some((width, height, stride, format)) = k::fb_info() else {
        k::print("fbtest: no framebuffer\n");
        k::exit(1);
    };

    k::print("fbtest: ");
    k::print_u64(width);
    k::print("x");
    k::print_u64(height);
    k::print(" stride ");
    k::print_u64(stride);
    k::print(" format ");
    k::print_u64(format);
    k::print("\n");

    // Base colour over the whole screen, then four known rectangles.
    k::fb_fill(0, 0, width, height, BG);
    k::fb_fill(40, 120, 120, 120, RED);
    k::fb_fill(200, 120, 120, 120, GREEN);
    k::fb_fill(360, 120, 120, 120, BLUE);
    k::fb_fill(520, 120, 80, 240, YELLOW);

    k::print("fbtest: drew pattern; press enter to exit\n");
    let mut line = [0u8; 8];
    k::read_line(&mut line);

    k::print("fbtest: done\n");
    k::exit(0);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    k::print("fbtest: panic\n");
    k::exit(1);
}
