//! Syscall implementations and helper accessors for user memory.
//!
//! The SVC dispatch lives in `exceptions.rs`; this module turns raw arguments
//! into operations on the current process (FDs, exec, spawn, wait).

use crate::exceptions::ExceptionFrame;
use crate::process;
use crate::ramfs::Vfs;
use crate::{diskfs, println, ramfs};

pub const SYS_WRITE: u64 = 1;
pub const SYS_EXIT: u64 = 2;
pub const SYS_READ: u64 = 3;
pub const SYS_OPEN: u64 = 4;
pub const SYS_READ_FILE: u64 = 5;
pub const SYS_CLOSE: u64 = 6;
pub const SYS_EXEC: u64 = 7;
pub const SYS_LIST: u64 = 8;
pub const SYS_SPAWN: u64 = 9;
pub const SYS_WAIT: u64 = 10;
pub const SYS_GETPID: u64 = 11;
pub const SYS_STATS: u64 = 12;
pub const SYS_CREATE: u64 = 13;
pub const SYS_WRITE_FILE: u64 = 14;
pub const SYS_DISKINFO: u64 = 15;

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

/// Copy a user path into a kernel buffer and run `action` on it.
fn with_user_path(
    pointer: u64,
    length: u64,
    mut action: impl FnMut(&str) -> u64,
) -> u64 {
    let Some(bytes) = user_bytes(pointer, length) else {
        return ERROR;
    };
    let Ok(user_path) = core::str::from_utf8(bytes) else {
        return ERROR;
    };

    // Loading or switching can remap the user stack that holds the path, so
    // never keep a user pointer across the action.
    let mut buffer = [0u8; 96];
    let length = user_path.len().min(buffer.len() - 1);
    buffer[..length].copy_from_slice(&user_path.as_bytes()[..length]);
    let path = core::str::from_utf8(&buffer[..length]).unwrap_or("");
    action(path)
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

/// open(path, len) -> per-process descriptor or u64::MAX.
pub fn sys_open(pointer: u64, length: u64) -> u64 {
    with_user_path(pointer, length, |path| {
        if let Some(name) = path.strip_prefix("/disk/") {
            let Some(inode) = diskfs::lookup(name) else {
                return ERROR;
            };
            return process::open_fd(process::FD_DISK, inode).unwrap_or(ERROR);
        }

        let Some(index) = ramfs::index_of(path) else {
            return ERROR;
        };
        process::open_fd(process::FD_RAM, index).unwrap_or(ERROR)
    })
}

/// read_file(fd, offset, buf, len) -> bytes read.
pub fn sys_read_file(fd: u64, offset: u64, pointer: u64, length: u64) -> u64 {
    let Some(buffer) = user_buffer(pointer, length) else {
        return ERROR;
    };
    let Some((kind, index)) = process::fd_target(fd) else {
        return ERROR;
    };

    if kind == process::FD_DISK {
        return diskfs::read(index, offset, buffer) as u64;
    }

    let Some(file) = ramfs::file_at(index) else {
        return ERROR;
    };
    ramfs::RAMFS.read(file, offset as usize, buffer) as u64
}

pub fn sys_close(fd: u64) -> u64 {
    if process::close_fd(fd) {
        0
    } else {
        ERROR
    }
}

/// list(buf, len) -> NUL-separated file names written into buf.
pub fn sys_list(pointer: u64, length: u64) -> u64 {
    let Some(buffer) = user_buffer(pointer, length) else {
        return ERROR;
    };

    let mut written = 0;
    let mut push = |name: &str| {
        for &byte in name.as_bytes() {
            if written < buffer.len() {
                buffer[written] = byte;
                written += 1;
            }
        }
        if written < buffer.len() {
            buffer[written] = 0;
            written += 1;
        }
    };

    for file in ramfs::files() {
        push(file.name);
    }
    diskfs::for_each_name(|name| {
        let mut path = [0u8; 40];
        path[..6].copy_from_slice(b"/disk/");
        let length = core::cmp::min(name.len(), path.len() - 6);
        path[6..6 + length].copy_from_slice(&name.as_bytes()[..length]);
        if let Ok(path) = core::str::from_utf8(&path[..6 + length]) {
            push(path);
        }
    });

    written as u64
}

/// exec(path, len) -> replaces the current image; returns u64::MAX on failure.
pub fn sys_exec(frame: &mut ExceptionFrame, pointer: u64, length: u64) -> u64 {
    with_user_path(pointer, length, |path| process::sys_exec(frame, path))
}

/// spawn(path, len) -> child PID, or u64::MAX.
pub fn sys_spawn(pointer: u64, length: u64) -> u64 {
    with_user_path(pointer, length, process::sys_spawn)
}

pub fn sys_getpid() -> u64 {
    process::sys_getpid()
}

/// create("/disk/name") -> descriptor, creating or truncating a disk file.
pub fn sys_create(pointer: u64, length: u64) -> u64 {
    with_user_path(pointer, length, |path| {
        let Some(name) = path.strip_prefix("/disk/") else {
            return ERROR;
        };
        let Some(inode) = diskfs::create(name) else {
            return ERROR;
        };
        process::open_fd(process::FD_DISK, inode).unwrap_or(ERROR)
    })
}

/// write_file(fd, offset, buf, len) -> bytes written (disk files only).
pub fn sys_write_file(fd: u64, offset: u64, pointer: u64, length: u64) -> u64 {
    let Some((kind, index)) = process::fd_target(fd) else {
        return ERROR;
    };
    if kind != process::FD_DISK {
        return ERROR;
    }
    let Some(bytes) = user_bytes(pointer, length) else {
        return ERROR;
    };
    match diskfs::write(index, offset, bytes) {
        Ok(written) => written as u64,
        Err(_) => ERROR,
    }
}

/// Packed `(total_sectors << 32) | file_count`, or u64::MAX if unmounted.
pub fn sys_diskinfo() -> u64 {
    match diskfs::info() {
        Some((total, files)) => (total << 32) | (files & 0xffff_ffff),
        None => ERROR,
    }
}

/// Number of free physical frames (test/debug aid).
pub fn sys_stats() -> u64 {
    crate::pfa::stats().0 as u64
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

/// ExitProcess(code): terminate this process, not the machine.
pub fn pe_exit_process(frame: &mut ExceptionFrame, code: u64) {
    println!();
    println!("[kernel] PE ExitProcess({})", code);
    process::sys_exit(frame, code);
}

pub fn pe_get_last_error() -> u64 {
    0
}

pub fn pe_set_last_error(_code: u64) -> u64 {
    0
}
