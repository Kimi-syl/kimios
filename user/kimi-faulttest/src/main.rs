#![no_std]
#![no_main]

//! Faults on an unmapped address: the kernel must kill only this process.

use core::panic::PanicInfo;

use libkimi as k;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    k::print("faulttest: reading unmapped memory\n");
    unsafe {
        core::ptr::read_volatile(0x7000_0000 as *const u64);
    }
    k::print("faulttest: FAIL: fault was not delivered\n");
    k::exit(1);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    k::exit(2)
}
