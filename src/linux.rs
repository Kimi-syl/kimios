//! Linux ABI translation layer (the "Desktop Lite" / WSL1-style pivot).
//!
//! Standard statically-linked AArch64 Linux binaries (musl/glibc) issue Linux
//! syscall numbers via `svc #0` with the number in x8. When the current
//! process was loaded as a Linux ELF (see elf::is_linux), the SVC handler
//! routes here instead of the native KimiOS ABI. We translate the core Linux
//! syscalls onto KimiOS services: UART/console, KimiFS, the frame allocator.
//!
//! This is a bounded subset — enough for static console binaries. It is NOT a
//! full Linux kernel: no threads/futex waiting, signals, epoll, dlopen, or
//! dynamic linking. Unsupported calls return -ENOSYS (logged) so a binary
//! fails loudly rather than corrupting state.

use crate::exceptions::ExceptionFrame;
use crate::ramfs::Vfs;
use crate::{diskfs, println, process, ramfs, uart};

// AArch64 Linux syscall numbers (asm-generic/unistd.h).
const L_IOCTL: u64 = 29;
const L_FACCESSAT: u64 = 48;
const L_FLOCK: u64 = 73;
const L_READLINKAT: u64 = 78;
const L_OPENAT: u64 = 56;
const L_CLOSE: u64 = 57;
const L_READ: u64 = 63;
const L_WRITE: u64 = 64;
const L_READV: u64 = 65;
const L_WRITEV: u64 = 66;
const L_PREAD64: u64 = 67;
const L_PWRITE64: u64 = 68;
const L_NEWFSTATAT: u64 = 79;
const L_FSTAT: u64 = 80;
const L_EXIT: u64 = 93;
const L_EXIT_GROUP: u64 = 94;
const L_SET_TID_ADDRESS: u64 = 96;
const L_FUTEX: u64 = 98;
const L_SET_ROBUST_LIST: u64 = 99;
const L_TKILL: u64 = 130;
const L_TGKILL: u64 = 131;
const L_SIGALTSTACK: u64 = 132;
const L_RT_SIGACTION: u64 = 134;
const L_RT_SIGPROCMASK: u64 = 135;
const L_RT_SIGRETURN: u64 = 139;
const L_CLOCK_GETTIME: u64 = 113;
const L_UNAME: u64 = 160;
const L_GETPID: u64 = 172;
const L_GETTID: u64 = 178;
const L_SYSINFO: u64 = 179;
const L_BRK: u64 = 214;
const L_MUNMAP: u64 = 215;
const L_MMAP: u64 = 222;
const L_MPROTECT: u64 = 226;
const L_MADVISE: u64 = 233;
const L_PRLIMIT64: u64 = 261;
const L_GETRANDOM: u64 = 278;
const L_STATX: u64 = 291;
const L_RSEQ: u64 = 293;

const MAX_TRANSFER: u64 = 65536;

fn eno(e: u64) -> u64 {
    (-(e as i64)) as u64
}

fn lbytes(pointer: u64, length: u64) -> Option<&'static [u8]> {
    if pointer == 0 || length == 0 || length > MAX_TRANSFER || !process::user_range_ok(pointer, length)
    {
        return None;
    }
    Some(unsafe { core::slice::from_raw_parts(pointer as *const u8, length as usize) })
}

fn lbuffer(pointer: u64, length: u64) -> Option<&'static mut [u8]> {
    if pointer == 0 || length == 0 || length > MAX_TRANSFER || !process::user_range_ok(pointer, length)
    {
        return None;
    }
    Some(unsafe { core::slice::from_raw_parts_mut(pointer as *mut u8, length as usize) })
}

/// Read a NUL-terminated user C string (bounded to 256 bytes).
fn lcstr(pointer: u64) -> Option<&'static [u8]> {
    if pointer == 0 || !process::user_range_ok(pointer, 1) {
        return None;
    }
    let mut n = 0u64;
    while n < 256 {
        if !process::user_range_ok(pointer + n, 1) {
            break;
        }
        let b = unsafe { *((pointer + n) as *const u8) };
        if b == 0 {
            break;
        }
        n += 1;
    }
    Some(unsafe { core::slice::from_raw_parts(pointer as *const u8, n as usize) })
}

/// Dispatch a Linux syscall. Returns Some(result) to store in x0, or None when
/// the call switched processes (exit / blocking read) and the frame was reused.
pub fn dispatch(frame: &mut ExceptionFrame, number: u64) -> Option<u64> {
    let a0 = frame.x[0];
    let a1 = frame.x[1];
    let a2 = frame.x[2];
    let a3 = frame.x[3];
    let a4 = frame.x[4];
    let a5 = frame.x[5];

    match number {
        L_EXIT | L_EXIT_GROUP => {
            process::sys_exit(frame, a0);
            None
        }
        L_WRITE => Some(l_write(a0, a1, a2)),
        L_WRITEV => Some(l_writev(a0, a1, a2)),
        L_READ => l_read(frame, a0, a1, a2),
        L_PREAD64 => Some(l_pread(a0, a1, a2, a3)),
        L_OPENAT => Some(l_openat(a1, a2)),
        L_CLOSE => Some(if process::close_fd(a0) { 0 } else { eno(9) }),
        L_BRK => Some(process::linux_brk(a0)),
        L_MMAP => Some(l_mmap(a1, a2, a3, a4, a5)),
        L_MUNMAP => Some(0),
        L_MPROTECT | L_MADVISE => Some(0),
        L_GETPID | L_GETTID | L_SET_TID_ADDRESS => Some(process::sys_getpid()),
        L_SET_ROBUST_LIST | L_SYSINFO => Some(0),
        L_RSEQ | L_STATX => Some(eno(38)),
        L_PRLIMIT64 => Some(0),
        L_IOCTL => Some(eno(25)), // ENOTTY: not a terminal ioctl we support
        L_FUTEX => Some(0),       // single-threaded: no contention
        L_FLOCK => Some(0),
        L_READLINKAT => Some(eno(2)), // ENOENT (e.g. /proc/self/exe)
        // Single-threaded: signal bookkeeping is accepted but inert.
        L_TKILL | L_TGKILL | L_SIGALTSTACK | L_RT_SIGACTION | L_RT_SIGPROCMASK
        | L_RT_SIGRETURN => Some(0),
        L_FACCESSAT => Some(0),
        L_CLOCK_GETTIME => Some(l_clock_gettime(a0, a1)),
        L_GETRANDOM => Some(l_getrandom(a0, a1)),
        L_UNAME => Some(l_uname(a0)),
        L_FSTAT => Some(l_fstat_fd(a0, a1)),
        L_NEWFSTATAT => Some(l_fstat_path(a1, a2)),
        _ => {
            println!("[linux] unhandled syscall {} (x0={:#x})", number, a0);
            Some(eno(38))
        }
    }
}

fn l_write(fd: u64, pointer: u64, length: u64) -> u64 {
    let Some(bytes) = lbytes(pointer, length) else {
        return eno(14); // EFAULT
    };
    if fd == 1 || fd == 2 {
        uart::write_bytes(bytes);
        return length;
    }
    match process::fd_write(fd, bytes) {
        Some(n) => n as u64,
        None => eno(9), // EBADF
    }
}

fn l_writev(fd: u64, iov_ptr: u64, count: u64) -> u64 {
    if count > 16 {
        return eno(22);
    }
    let mut total = 0u64;
    for i in 0..count {
        let entry = iov_ptr + i * 16;
        let Some(base) = read_user_u64(entry) else {
            return eno(14);
        };
        let Some(len) = read_user_u64(entry + 8) else {
            return eno(14);
        };
        if len == 0 {
            continue;
        }
        let n = l_write(fd, base, len);
        if n > 0x8000_0000_0000_0000 {
            return n;
        }
        total += n;
    }
    total
}

fn read_user_u64(pointer: u64) -> Option<u64> {
    if !process::user_range_ok(pointer, 8) {
        return None;
    }
    Some(unsafe { core::ptr::read_unaligned(pointer as *const u64) })
}

fn l_read(frame: &mut ExceptionFrame, fd: u64, pointer: u64, length: u64) -> Option<u64> {
    let Some(buffer) = lbuffer(pointer, length) else {
        return Some(eno(14));
    };
    if fd == 0 {
        if let Some(n) = uart::take_line(buffer) {
            return Some(n as u64);
        }
        if process::reader_busy() {
            return Some(eno(11)); // EAGAIN
        }
        if process::block_on_read(frame) {
            return None;
        }
        uart::wait_for_line();
        let n = uart::take_line(buffer).unwrap_or(0);
        return Some(n as u64);
    }
    match process::fd_read(fd, buffer) {
        Some(n) => Some(n as u64),
        None => Some(eno(9)),
    }
}

fn l_pread(fd: u64, pointer: u64, length: u64, offset: u64) -> u64 {
    let Some(buffer) = lbuffer(pointer, length) else {
        return eno(14);
    };
    let Some((kind, index)) = process::fd_target(fd) else {
        return eno(9);
    };
    let n = if kind == process::FD_DISK {
        diskfs::read(index, offset, buffer)
    } else {
        match ramfs::file_at(index) {
            Some(file) => ramfs::RAMFS.read(file, offset as usize, buffer),
            None => return eno(9),
        }
    };
    n as u64
}

fn l_openat(path_ptr: u64, flags: u64) -> u64 {
    let Some(bytes) = lcstr(path_ptr) else {
        return eno(14);
    };
    let Ok(path) = core::str::from_utf8(bytes) else {
        return eno(22);
    };
    // O_CREAT on a /disk path creates the file.
    let o_creat = flags & 0o100 != 0;
    if let Some(name) = path.strip_prefix("/disk/") {
        if let Some(inode) = diskfs::lookup(name) {
            return process::open_fd(process::FD_DISK, inode).unwrap_or(eno(24));
        }
        if o_creat {
            if let Some(inode) = diskfs::create(name) {
                return process::open_fd(process::FD_DISK, inode).unwrap_or(eno(24));
            }
        }
        return eno(2); // ENOENT
    }
    if let Some(index) = ramfs::index_of(path) {
        return process::open_fd(process::FD_RAM, index).unwrap_or(eno(24));
    }
    eno(2)
}

fn l_mmap(length: u64, prot: u64, flags: u64, fd: u64, _offset: u64) -> u64 {
    let anonymous = flags & 0x20 != 0; // MAP_ANONYMOUS
    if !anonymous || fd != u64::MAX {
        // File-backed mmap is not supported yet.
        return eno(38);
    }
    let executable = prot & 0x4 != 0; // PROT_EXEC
    match process::linux_mmap(length, executable) {
        Some(address) => address,
        None => eno(12), // ENOMEM
    }
}

fn l_clock_gettime(_clock: u64, pointer: u64) -> u64 {
    let Some(buffer) = lbuffer(pointer, 16) else {
        return eno(14);
    };
    let ms = crate::timer::uptime_ms();
    let sec = (ms / 1000) as u64;
    let nsec = ((ms % 1000) * 1_000_000) as u64;
    buffer[..8].copy_from_slice(&sec.to_le_bytes());
    buffer[8..16].copy_from_slice(&nsec.to_le_bytes());
    0
}

fn l_getrandom(pointer: u64, length: u64) -> u64 {
    let Some(buffer) = lbuffer(pointer, length) else {
        return eno(14);
    };
    let mut seed = crate::timer::uptime_ms().wrapping_mul(0x9e37_79b9_7f4a_7c15);
    for byte in buffer.iter_mut() {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        *byte = seed as u8;
    }
    length
}

fn l_uname(pointer: u64) -> u64 {
    // struct utsname: six 65-byte fields (sysname, nodename, release, version,
    // machine, domainname).
    let Some(buffer) = lbuffer(pointer, 65 * 6) else {
        return eno(14);
    };
    for b in buffer.iter_mut() {
        *b = 0;
    }
    let mut put = |off: usize, s: &[u8]| {
        let n = s.len().min(64);
        buffer[off..off + n].copy_from_slice(&s[..n]);
    };
    put(0, b"Linux");
    put(65, b"kimios");
    put(130, b"6.6.0-kimios");
    put(195, b"#1 SMP");
    put(260, b"aarch64");
    put(325, b"(none)");
    0
}

/// Fill a 128-byte aarch64 `struct stat` for an open fd.
fn l_fstat_fd(fd: u64, pointer: u64) -> u64 {
    let Some(buffer) = lbuffer(pointer, 128) else {
        return eno(14);
    };
    for b in buffer.iter_mut() {
        *b = 0;
    }
    if fd <= 2 {
        // Character device (console): st_mode = S_IFCHR | 0620.
        let mode: u32 = 0o20000 | 0o620;
        buffer[16..20].copy_from_slice(&mode.to_le_bytes());
        let rdev: u64 = 0x0501; // tty-ish
        buffer[24..32].copy_from_slice(&rdev.to_le_bytes());
        return 0;
    }
    let Some((kind, index)) = process::fd_target(fd) else {
        return eno(9);
    };
    let size = if kind == process::FD_DISK {
        diskfs::inode_size(index).unwrap_or(0)
    } else {
        ramfs::file_at(index).map(|f| f.data.len() as u64).unwrap_or(0)
    };
    let mode: u32 = 0o100000 | 0o644; // S_IFREG
    buffer[16..20].copy_from_slice(&mode.to_le_bytes());
    buffer[48..56].copy_from_slice(&size.to_le_bytes());
    let blksize: u32 = 4096;
    buffer[56..60].copy_from_slice(&blksize.to_le_bytes());
    0
}

fn l_fstat_path(path_ptr: u64, pointer: u64) -> u64 {
    let Some(bytes) = lcstr(path_ptr) else {
        return eno(14);
    };
    let Ok(path) = core::str::from_utf8(bytes) else {
        return eno(22);
    };
    let Some(buffer) = lbuffer(pointer, 128) else {
        return eno(14);
    };
    for b in buffer.iter_mut() {
        *b = 0;
    }
    let size = if let Some(name) = path.strip_prefix("/disk/") {
        match diskfs::lookup(name) {
            Some(inode) => diskfs::inode_size(inode).unwrap_or(0),
            None => return eno(2),
        }
    } else {
        match ramfs::index_of(path).and_then(ramfs::file_at) {
            Some(file) => file.data.len() as u64,
            None => return eno(2),
        }
    };
    let mode: u32 = 0o100000 | 0o644;
    buffer[16..20].copy_from_slice(&mode.to_le_bytes());
    buffer[48..56].copy_from_slice(&size.to_le_bytes());
    0
}