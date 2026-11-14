#![no_std]
#![no_main]

//! Passes an unmapped buffer to a syscall: the kernel must reject it with an
//! error rather than faulting while copying.

use core::panic::PanicInfo;

use libkimi as k;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let bogus = unsafe {
        core::slice::from_raw_parts_mut(0x7000_0000 as *mut u8, 16)
    };
    match k::listdir("/disk", bogus) {
        None => k::print("badptr: PASS: syscall rejected the buffer\n"),
        Some(count) => {
            k::print("badptr: FAIL: wrote ");
            k::print_u64(count as u64);
            k::print(" bytes to unmapped memory\n");
            k::exit(1);
        }
    }
    k::exit(0);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    k::exit(2)
}
