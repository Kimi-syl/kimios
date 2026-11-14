#![no_std]
#![no_main]

//! A quiet process that sleeps forever, useful as a `kill` target.

use core::panic::PanicInfo;

use libkimi as k;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    k::print("sleeper: pid ");
    k::print_u64(k::getpid());
    k::print(" sleeping\n");
    loop {
        k::sleep_ms(500);
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    k::exit(2)
}
