#![no_std]
#![no_main]

//! Waits for eight input events and prints them; used to verify keyboard and
//! pointer events reach user space through one queue.

use core::panic::PanicInfo;

use libkimi as k;

const WANTED: usize = 6;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    k::print("inputtest: waiting\n");
    for index in 0..WANTED {
        let Some(event) = k::input_wait() else {
            k::print("inputtest: FAIL: wait error\n");
            k::exit(1);
        };
        k::print("event ");
        k::print_u64(index as u64);
        k::print(": type ");
        k::print_u64(event.kind as u64);
        k::print(" code ");
        k::print_u64(event.code as u64);
        k::print(" value ");
        k::print_u64(event.value as u64);
        k::print("\n");
    }
    k::print("inputtest: PASS\n");
    k::exit(0);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    k::exit(2)
}
