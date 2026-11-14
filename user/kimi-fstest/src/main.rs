#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libkimi as k;

const SIZE: usize = 800;
const CHUNK: usize = 200;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let mut ok = true;

    let Some(handle) = k::create("/disk/fstest.bin") else {
        k::print("fstest: create failed\n");
        k::exit(1);
    };

    for (offset, byte) in [(0usize, b'A'), (200, b'B'), (400, b'C'), (600, b'D')] {
        let block = [byte; CHUNK];
        if k::write_file(handle, offset, &block) != CHUNK {
            ok = false;
        }
    }
    k::close(handle);

    let Some(handle) = k::open("/disk/fstest.bin") else {
        k::print("fstest: reopen failed\n");
        k::exit(1);
    };

    let mut buffer = [0u8; SIZE];
    if k::read_file(handle, 0, &mut buffer) != SIZE {
        ok = false;
    }
    k::close(handle);

    for index in 0..SIZE {
        let expected = match index / CHUNK {
            0 => b'A',
            1 => b'B',
            2 => b'C',
            _ => b'D',
        };
        if buffer[index] != expected {
            ok = false;
            break;
        }
    }

    if ok {
        k::print("fstest: PASS (800 bytes across 2 sectors)\n");
        k::exit(0);
    } else {
        k::print("fstest: FAIL\n");
        k::exit(1);
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    k::print("fstest: panic\n");
    k::exit(2);
}
