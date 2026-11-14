//! EL0 entry and syscall implementations.
//!
//! Phase 10 loads `/init` from RamFS, maps its ELF segments into the user
//! address space, and enters it with `eret`. Phase 11 adds the file, stdin,
//! listing and exec syscalls used by the user shell.

use core::arch::asm;

use crate::exceptions::ExceptionFrame;
use crate::ramfs::Vfs;
use crate::{elf, pe, println, ramfs};

pub const SYS_WRITE: u64 = 1;
pub const SYS_EXIT: u64 = 2;
pub const SYS_READ: u64 = 3;
pub const SYS_OPEN: u64 = 4;
pub const SYS_READ_FILE: u64 = 5;
pub const SYS_CLOSE: u64 = 6;
pub const SYS_EXEC: u64 = 7;
pub const SYS_LIST: u64 = 8;

const ERROR: u64 = u64::MAX;
const MAX_USER_TRANSFER: u64 = 8192;

fn user_bytes(pointer: u64, length: u64) -> Option<&'static [u8]> {
    if pointer == 0 || length == 0 || length > MAX_USER_TRANSFER {
        return None;
    }
    // SAFETY: the caller is an EL0 process whose mappings are trusted by this
    // toy kernel; the length is bounded above.
    Some(unsafe { core::slice::from_raw_parts(pointer as *const u8, length as usize) })
}

fn user_buffer(pointer: u64, length: u64) -> Option<&'static mut [u8]> {
    if pointer == 0 || length == 0 || length > MAX_USER_TRANSFER {
        return None;
    }
    // SAFETY: as above; the syscall uses the buffer only for this call.
    Some(unsafe { core::slice::from_raw_parts_mut(pointer as *mut u8, length as usize) })
}

pub fn run_init() -> ! {
    println!();
    println!("Phase 12: RamFS, ELF/PE loaders, EL0 shell");

    let file = ramfs::RAMFS.open("/init").expect("no /init in RamFS");
    println!("RamFS      : /init ({} bytes)", file.data.len());

    let mut magic = [0u8; 4];
    let read = ramfs::RAMFS.read(file, 0, &mut magic);
    println!("VFS read   : {} bytes, magic {:02x?}", read, magic);

    let image = elf::load(file.data).expect("failed to load /init");
    println!(
        "ELF loaded : entry={:#x} stack_top={:#x} pages={}",
        image.entry, image.stack_top, image.pages
    );
    println!("Dropping to EL0...");

    unsafe {
        asm!(
            "msr sp_el0, {sp}",
            "msr elr_el1, {entry}",
            "msr spsr_el1, {spsr}",
            "eret",
            sp = in(reg) image.stack_top,
            entry = in(reg) image.entry,
            // EL0t, IRQs masked (kernel scheduler is kernel-only for now).
            spsr = in(reg) 0x80u64,
            options(noreturn),
        );
    }
}

/// write(fd, buf, len) -> bytes written, or u64::MAX on error.
pub fn sys_write(fd: u64, pointer: u64, length: u64) -> u64 {
    if fd != 1 {
        return ERROR;
    }

    let Some(bytes) = user_bytes(pointer, length) else {
        return ERROR;
    };

    crate::uart::write_bytes(bytes);
    length
}

/// read(fd, buf, len) -> bytes read. Blocks until newline (stdin only).
pub fn sys_read(fd: u64, pointer: u64, length: u64) -> u64 {
    if fd != 0 {
        return ERROR;
    }
    let Some(buffer) = user_buffer(pointer, length) else {
        return ERROR;
    };

    let mut count = 0;
    loop {
        let byte = crate::uart::read_byte();
        match byte {
            b'\r' | b'\n' => {
                crate::uart::write_bytes(b"\n");
                return count as u64;
            }
            0x7f | 0x08 => {
                if count > 0 {
                    count -= 1;
                    crate::uart::write_bytes(b"\x08 \x08");
                }
            }
            _ => {
                if count < buffer.len() {
                    buffer[count] = byte;
                    count += 1;
                    crate::uart::write_bytes(&[byte]);
                }
            }
        }
    }
}

/// open(path, len) -> handle or u64::MAX.
pub fn sys_open(pointer: u64, length: u64) -> u64 {
    let Some(bytes) = user_bytes(pointer, length) else {
        return ERROR;
    };
    let Ok(path) = core::str::from_utf8(bytes) else {
        return ERROR;
    };

    match ramfs::index_of(path) {
        Some(index) => index as u64,
        None => ERROR,
    }
}

/// read_file(handle, offset, buf, len) -> bytes read.
pub fn sys_read_file(handle: u64, offset: u64, pointer: u64, length: u64) -> u64 {
    let Some(buffer) = user_buffer(pointer, length) else {
        return ERROR;
    };
    let Some(file) = ramfs::file_at(handle as usize) else {
        return ERROR;
    };

    ramfs::RAMFS.read(file, offset as usize, buffer) as u64
}

pub fn sys_close(_handle: u64) -> u64 {
    0
}

/// list(buf, len) -> NUL-separated file names written into buf.
pub fn sys_list(pointer: u64, length: u64) -> u64 {
    let Some(buffer) = user_buffer(pointer, length) else {
        return ERROR;
    };

    let mut written = 0;
    for file in ramfs::files() {
        for &byte in file.name.as_bytes() {
            if written < buffer.len() {
                buffer[written] = byte;
                written += 1;
            }
        }
        if written < buffer.len() {
            buffer[written] = 0;
            written += 1;
        }
    }

    written as u64
}

/// exec(path, len) -> only returns on failure. On success, redirects the
/// exception return frame to the new ELF entry point.
pub fn sys_exec(frame: &mut ExceptionFrame, pointer: u64, length: u64) -> u64 {
    let Some(bytes) = user_bytes(pointer, length) else {
        return ERROR;
    };
    let Ok(user_path) = core::str::from_utf8(bytes) else {
        return ERROR;
    };

    // Loading a new image remaps and zeroes the user stack page, which may
    // hold the path buffer, so keep a kernel copy for the messages below.
    let mut path_buffer = [0u8; 96];
    let path_length = user_path.len().min(path_buffer.len() - 1);
    path_buffer[..path_length].copy_from_slice(&user_path.as_bytes()[..path_length]);
    let path = core::str::from_utf8(&path_buffer[..path_length]).unwrap_or("");

    let Some(index) = ramfs::index_of(path) else {
        println!("exec: {} not found", path);
        return ERROR;
    };
    let Some(file) = ramfs::file_at(index) else {
        return ERROR;
    };

    let result = if pe::is_pe(file.data) {
        pe::load(file.data).map(|image| (image.entry, image.stack_top))
    } else {
        elf::load(file.data).map(|image| (image.entry, image.stack_top))
    };

    match result {
        Ok((entry, stack_top)) => {
            println!("exec: {} -> entry {:#x}", path, entry);
            unsafe {
                asm!("msr sp_el0, {}", in(reg) stack_top, options(nostack));
            }
            frame.elr = entry;
            0
        }
        Err(error) => {
            println!("exec: {}", error);
            ERROR
        }
    }
}

/// GetStdHandle(STD_*_HANDLE) -> console handle 1.
pub fn pe_get_std_handle(_which: u64) -> u64 {
    1
}

/// WriteFile(handle, buffer, length, written, overlapped) -> BOOL.
pub fn pe_write_file(
    handle: u64,
    pointer: u64,
    length: u64,
    written_pointer: u64,
    _overlapped: u64,
) -> u64 {
    if handle != 1 {
        return 0;
    }

    if length != 0 {
        let Some(bytes) = user_bytes(pointer, length) else {
            return 0;
        };
        crate::uart::write_bytes(bytes);
    }

    if written_pointer != 0 {
        unsafe {
            core::ptr::write_unaligned(written_pointer as *mut u32, length as u32);
        }
    }

    1
}

/// ExitProcess(code) -> never returns.
pub fn pe_exit_process(code: u64) -> ! {
    println!();
    println!("[kernel] PE ExitProcess({})", code);
    crate::semihosting_exit(0x20026, 0);
}

pub fn pe_get_last_error() -> u64 {
    0
}

pub fn pe_set_last_error(_code: u64) -> u64 {
    0
}

/// exit(status) -> never returns.
pub fn sys_exit(status: u64) -> ! {
    println!();
    println!("[kernel] EL0 exit({})", status);
    crate::semihosting_exit(0x20026, 0);
}
