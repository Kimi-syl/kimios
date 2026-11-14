#![no_std]
#![no_main]

//! Exercises `brk`: grow the heap, touch every new byte, and (on failure)
//! report where the check broke.

use core::panic::PanicInfo;

use libkimi as k;

const PAGES: u64 = 4;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let start = k::brk();
    k::print("heaptest: start ");
    k::print_u64(start);
    k::print("\n");

    if start & 0xfff != 0 {
        fail("start not page aligned");
    }

    let request = start + PAGES * 4096;
    let end = k::sbrk(request);
    k::print("heaptest: end ");
    k::print_u64(end);
    k::print("\n");

    if end != request {
        fail("brk moved the break without rounding");
    }
    if k::brk() != end {
        fail("brk(0) disagrees with the returned break");
    }

    let length = (end - start) as usize;
    for i in 0..length {
        unsafe {
            ((start as usize + i) as *mut u8).write_volatile((i as u8) ^ 0xa5);
        }
    }
    for i in 0..length {
        let value = unsafe { ((start as usize + i) as *const u8).read_volatile() };
        if value != (i as u8) ^ 0xa5 {
            fail("pattern mismatch");
        }
    }
    k::print("heaptest: wrote and verified ");
    k::print_u64(length as u64);
    k::print(" bytes\n");

    // A non-page-aligned request rounds up to the next page.
    let rounded = k::sbrk(end + 1);
    if rounded != end + 4096 {
        fail("unaligned brk did not round up");
    }
    unsafe {
        ((rounded - 1) as *mut u8).write_volatile(0x5a);
        if ((rounded - 1) as *const u8).read_volatile() != 0x5a {
            fail("last byte of the rounded page is not writable");
        }
    }

    // Shrinking is not supported: the break must not move.
    if k::sbrk(start) != rounded {
        fail("shrink moved the break");
    }

    k::print("heaptest: PASS\n");
    k::exit(0);
}

fn fail(message: &str) -> ! {
    k::print("heaptest: FAIL: ");
    k::print(message);
    k::print("\n");
    k::exit(1);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    k::exit(2)
}
