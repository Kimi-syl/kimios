#![no_std]
#![no_main]

//! Exercises `libkimi::Heap` through `alloc::vec::Vec`: repeated small
//! allocations reuse the free list, and one large vector forces `brk` growth.

extern crate alloc;

use alloc::vec::Vec;
use core::panic::PanicInfo;

use libkimi as k;

#[global_allocator]
static HEAP: k::Heap = k::Heap;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let mut checksum = 0u64;
    for round in 0..64u64 {
        let mut values: Vec<u64> = Vec::new();
        for index in 0..200u64 {
            values.push(index.wrapping_mul(3).wrapping_add(round));
        }
        if values.len() != 200 {
            fail("short vector");
        }
        checksum = checksum.wrapping_add(values[100]);
    }
    if checksum != 21_216 {
        fail("checksum mismatch");
    }
    k::print("alloctest: checksum ");
    k::print_u64(checksum);
    k::print("\n");

    let mut big: Vec<u64> = Vec::new();
    for index in 0..5_000u64 {
        big.push(index);
    }
    let sum: u64 = big.iter().copied().sum();
    if sum != 12_497_500 {
        fail("large vector sum mismatch");
    }
    k::print("alloctest: large vector sum ");
    k::print_u64(sum);
    k::print(", brk ");
    k::print_u64(k::brk());
    k::print("\n");

    k::print("alloctest: PASS\n");
    k::exit(0);
}

fn fail(message: &str) -> ! {
    k::print("alloctest: FAIL: ");
    k::print(message);
    k::print("\n");
    k::exit(1);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    k::exit(2)
}
