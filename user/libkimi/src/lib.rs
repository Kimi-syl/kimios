//! libkimi: user-space syscall wrappers for KimiOS.
//!
//! Syscall ABI: `x8` = number, `x0..x3` = arguments, result in `x0`.
//! All pointers passed in arguments must refer to memory mapped in the
//! calling process.

#![no_std]

use core::arch::asm;

pub const SYS_WRITE: u64 = 1;
pub const SYS_EXIT: u64 = 2;
pub const SYS_READ: u64 = 3;
pub const SYS_OPEN: u64 = 4;
pub const SYS_READ_FILE: u64 = 5;
pub const SYS_CLOSE: u64 = 6;
pub const SYS_EXEC: u64 = 7;
pub const SYS_LIST: u64 = 8;

pub const STDIN: u64 = 0;
pub const STDOUT: u64 = 1;

const ERROR: u64 = u64::MAX;

#[inline(always)]
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

#[inline(always)]
fn syscall4(number: u64, arg0: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    let result;
    unsafe {
        asm!(
            "svc #0",
            in("x8") number,
            inlateout("x0") arg0 => result,
            in("x1") arg1,
            in("x2") arg2,
            in("x3") arg3,
        );
    }
    result
}

pub fn write(fd: u64, buffer: &[u8]) -> u64 {
    syscall3(SYS_WRITE, fd, buffer.as_ptr() as u64, buffer.len() as u64)
}

pub fn print(text: &str) {
    write(STDOUT, text.as_bytes());
}

pub fn print_bytes(bytes: &[u8]) {
    write(STDOUT, bytes);
}

/// Read a line (or up to `buffer.len()` bytes) from stdin.
pub fn read_line(buffer: &mut [u8]) -> usize {
    let result = syscall3(
        SYS_READ,
        STDIN,
        buffer.as_mut_ptr() as u64,
        buffer.len() as u64,
    );
    if result == ERROR {
        0
    } else {
        result as usize
    }
}

/// Open a RamFS path. Returns a handle on success.
pub fn open(path: &str) -> Option<u64> {
    let result = syscall3(
        SYS_OPEN,
        path.as_ptr() as u64,
        path.len() as u64,
        0,
    );
    if result == ERROR {
        None
    } else {
        Some(result)
    }
}

pub fn read_file(handle: u64, offset: usize, buffer: &mut [u8]) -> usize {
    let result = syscall4(
        SYS_READ_FILE,
        handle,
        offset as u64,
        buffer.as_mut_ptr() as u64,
        buffer.len() as u64,
    );
    if result == ERROR {
        0
    } else {
        result as usize
    }
}

pub fn close(handle: u64) {
    syscall3(SYS_CLOSE, handle, 0, 0);
}

/// Replace the current program with the ELF at `path`.
/// Only returns if loading failed (`u64::MAX`).
pub fn exec(path: &str) -> u64 {
    syscall3(SYS_EXEC, path.as_ptr() as u64, path.len() as u64, 0)
}

/// Write NUL-separated file names into `buffer`, returns bytes written.
pub fn list(buffer: &mut [u8]) -> usize {
    let result = syscall3(
        SYS_LIST,
        buffer.as_mut_ptr() as u64,
        buffer.len() as u64,
        0,
    );
    if result == ERROR {
        0
    } else {
        result as usize
    }
}

pub fn exit(status: u64) -> ! {
    syscall3(SYS_EXIT, status, 0, 0);
    loop {
        unsafe { asm!("wfe") };
    }
}
