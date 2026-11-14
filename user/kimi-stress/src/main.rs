#![no_std]
#![no_main]

//! Churns spawn/wait, file create/delete and the user allocator; free frame
//! counts before and after should match.

extern crate alloc;

use alloc::vec::Vec;
use core::panic::PanicInfo;

use libkimi as k;

const CHILDREN: usize = 10;
const FILE_CYCLES: usize = 20;
const ALLOC_CYCLES: usize = 100;

#[global_allocator]
static HEAP: k::Heap = k::Heap;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let frames_before = k::free_frames();
    k::print("stress: free frames ");
    k::print_u64(frames_before);
    k::print("\n");

    for round in 0..CHILDREN {
        let Some(pid) = k::spawn("/hello") else {
            fail("spawn failed");
        };
        match k::wait(pid) {
            Some(0) => {}
            Some(status) => {
                k::print("stress: child status ");
                k::print_u64(status);
                k::print("\n");
                fail("child failed");
            }
            None => fail("wait failed"),
        }
        if round % 5 == 0 {
            k::print("stress: children ");
            k::print_u64(round as u64 + 1);
            k::print("\n");
        }
    }

    k::unlink("/disk/stress/f");
    k::rmdir("/disk/stress");
    for _ in 0..FILE_CYCLES {
        if !k::mkdir("/disk/stress") {
            fail("mkdir failed");
        }
        let Some(handle) = k::create("/disk/stress/f") else {
            fail("create failed");
        };
        let payload = b"stress payload";
        if k::write_file(handle, 0, payload) != payload.len() {
            fail("write failed");
        }
        k::close(handle);
        if k::stat("/disk/stress/f").is_none() {
            fail("stat failed");
        }
        if !k::unlink("/disk/stress/f") {
            fail("unlink failed");
        }
        if !k::rmdir("/disk/stress") {
            fail("rmdir failed");
        }
    }
    k::print("stress: file cycles done\n");

    for round in 0..ALLOC_CYCLES {
        let size = (round % 64) * 8 + 8;
        let mut values: Vec<u64> = Vec::with_capacity(size);
        for index in 0..size {
            values.push(index as u64);
        }
        let expected: u64 = (0..size as u64).sum();
        if values.iter().copied().sum::<u64>() != expected {
            fail("allocator corrupted data");
        }
    }
    k::print("stress: alloc cycles done\n");

    // The heap high-water mark is intentionally not returned to the kernel
    // while this process lives; the shell checks `mem` before/after the whole
    // run to prove that process teardown returns every frame.
    let frames_after = k::free_frames();
    k::print("stress: free frames ");
    k::print_u64(frames_after);
    k::print(" (high-water while running)\n");
    k::print("stress: PASS\n");
    k::exit(0);
}

fn fail(message: &str) -> ! {
    k::print("stress: FAIL: ");
    k::print(message);
    k::print("\n");
    k::exit(1)
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    k::exit(2)
}
