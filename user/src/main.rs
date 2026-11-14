#![no_std]
#![no_main]

use core::arch::asm;
use core::panic::PanicInfo;

const SYS_WRITE: u64 = 1;
const SYS_EXIT: u64 = 2;

fn syscall3(number: u64, arg0: u64, arg1: u64, arg2: u64) -> u64 {
    let result;
    unsafe {
        asm!(
            "svc #0",
            in("x8") number,
            inlateout("x0") arg0 => result,
            in("x1") arg1,
            in("x2") arg2,
        );
    }
    result
}

fn write(fd: u64, buffer: &[u8]) -> u64 {
    syscall3(SYS_WRITE, fd, buffer.as_ptr() as u64, buffer.len() as u64)
}

fn exit(status: u64) -> ! {
    syscall3(SYS_EXIT, status, 0, 0);
    loop {
        unsafe { asm!("wfe") };
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    write(1, b"Hello from an ELF binary loaded by KimiOS!\n");
    write(1, b"RamFS + xmas-elf + EL0 syscalls all working.\n");
    exit(0);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    exit(1);
}
