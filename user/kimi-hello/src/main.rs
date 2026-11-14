#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libkimi::{exit, getpid, print, print_u64};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    print("Hello from /hello, pid ");
    print_u64(getpid());
    print("!\n");
    print("Cheerio from EL0.\n");
    exit(0);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    exit(1);
}
