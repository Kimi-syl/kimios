#![no_std]
#![no_main]

use core::arch::asm;
use core::panic::PanicInfo;
use libkimi as k;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let digit = k::getpid() % 10;

    for _ in 0..24 {
        k::print_u64(digit);
        // Busy work so the timer IRQ lands mid-loop.
        for _ in 0..3_000_000 {
            unsafe { asm!("nop", options(nomem, nostack)); }
        }
    }

    k::print("\n");
    k::exit(0);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    k::print("spin: panic\n");
    k::exit(1);
}
