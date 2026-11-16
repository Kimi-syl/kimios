//! Linux ABI translation layer (the "Desktop Lite" / WSL1-style pivot).
//!
//! Standard statically-linked AArch64 Linux binaries (musl/glibc, busybox)
//! issue Linux syscall numbers via `svc #0` with the number in x8. When the
//! current process was loaded as a Linux ELF (see elf::is_linux), the SVC
//! handler routes here instead of the native KimiOS ABI. We translate Linux
//! syscalls onto KimiOS services: UART/console, KimiFS, the frame allocator.
//!
//! This is a bounded personality — enough for static console binaries and
//! busybox applets (ls, cat, echo, mkdir, cp, ...). It is NOT a full Linux
//! kernel: no fork/clone, threads, signals delivery, epoll, dlopen or dynamic
//! linking. Unsupported calls return -ENOSYS (logged) so a binary fails loudly
//! rather than corrupting state. Interactive `busybox sh` needs fork/exec and
//! is therefore out of scope for now.

use crate::exceptions::ExceptionFrame;
use crate::ramfs::Vfs;
use crate::{diskfs, println, process, ramfs, uart};

// AArch64 Linux syscall numbers (asm-generic/unistd.h).
const L_GETCWD: u64 = 17;
const L_DUP: u64 = 23;
const L_DUP3: u64 = 24;
const L_FCNTL: u64 = 25;
const L_IOCTL: u64 = 29;
const L_FLOCK: u64 = 32;
const L_MKDIRAT: u64 = 34;
const L_UNLINKAT: u64 = 35;
const L_RENAMEAT: u64 = 38;
const L_FACCESSAT: u64 = 48;
const L_CHDIR: u64 = 49;
const L_OPENAT: u64 = 56;
const L_CLOSE: u64 = 57;
const L_PIPE2: u64 = 59;
const L_GETDENTS64: u64 = 61;
const L_LSEEK: u64 = 62;
const L_READ: u64 = 63;
const L_WRITE: u64 = 64;
const L_READV: u64 = 65;
const L_WRITEV: u64 = 66;
const L_PREAD64: u64 = 67;
const L_PWRITE64: u64 = 68;
const L_SENDFILE: u64 = 71;
const L_READLINKAT: u64 = 78;
const L_NEWFSTATAT: u64 = 79;
const L_FSTAT: u64 = 80;
const L_EXIT: u64 = 93;
const L_EXIT_GROUP: u64 = 94;
const L_SET_TID_ADDRESS: u64 = 96;
const L_FUTEX: u64 = 98;
const L_SET_ROBUST_LIST: u64 = 99;
const L_SCHED_GETAFFINITY: u64 = 123;
const L_TKILL: u64 = 130;
const L_TGKILL: u64 = 131;
const L_SIGALTSTACK: u64 = 132;
const L_RT_SIGACTION: u64 = 134;
const L_RT_SIGPROCMASK: u64 = 135;
const L_RT_SIGRETURN: u64 = 139;
const L_CLOCK_GETTIME: u64 = 113;
const L_UNAME: u64 = 160;
const L_GETRLIMIT: u64 = 163;
const L_SETRLIMIT: u64 = 164;
const L_UMASK: u64 = 166;
const L_GETTIMEOFDAY: u64 = 169;
const L_GETPID: u64 = 172;
const L_GETUID: u64 = 174;
const L_GETEUID: u64 = 175;
const L_GETGID: u64 = 176;
const L_GETEGID: u64 = 177;
const L_GETTID: u64 = 178;
const L_SYSINFO: u64 = 179;
const L_BRK: u64 = 214;
const L_MUNMAP: u64 = 215;
const L_MMAP: u64 = 222;
const L_MPROTECT: u64 = 226;
const L_MADVISE: u64 = 233;
const L_PRLIMIT64: u64 = 261;
const L_RENAMEAT2: u64 = 276;
const L_GETRANDOM: u64 = 278;
const L_STATX: u64 = 291;
const L_RSEQ: u64 = 293;

const AT_FDCWD: u64 = (-100i64) as u64;
const MAX_TRANSFER: u64 = 65536;

const S_IFDIR: u32 = 0o40000;
const S_IFREG: u32 = 0o100000;
const S_IFCHR: u32 = 0o20000;
const DT_DIR: u8 = 4;
const DT_REG: u8 = 8;

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

fn read_user_u64(pointer: u64) -> Option<u64> {
    if !process::user_range_ok(pointer, 8) {
        return None;
    }
    Some(unsafe { core::ptr::read_unaligned(pointer as *const u64) })
}

// -- path resolution --------------------------------------------------------

/// Normalize `path` (handling ".", "..", relative vs absolute) into `out`.
/// Relative paths are resolved against the process cwd. Returns the length.
fn resolve_path(path: &str, out: &mut [u8]) -> Option<usize> {
    let mut comps: [&str; 24] = [""; 24];
    let mut nc = 0usize;
    if !path.starts_with('/') {
        for c in process::linux_cwd().split('/') {
            if !c.is_empty() && nc < 24 {
                comps[nc] = c;
                nc += 1;
            }
        }
    }
    for c in path.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                if nc > 0 {
                    nc -= 1;
                }
            }
            _ => {
                if nc < 24 {
                    comps[nc] = c;
                    nc += 1;
                }
            }
        }
    }
    let mut n = 0usize;
    if out.is_empty() {
        return None;
    }
    out[0] = b'/';
    n += 1;
    for i in 0..nc {
        if i > 0 {
            if n >= out.len() {
                return None;
            }
            out[n] = b'/';
            n += 1;
        }
        let c = comps[i].as_bytes();
        if n + c.len() >= out.len() {
            return None;
        }
        out[n..n + c.len()].copy_from_slice(c);
        n += c.len();
    }
    Some(n)
}

/// Resolve a path relative to a directory fd (or cwd for AT_FDCWD).
fn resolve_at(dirfd: u64, path: &str, out: &mut [u8]) -> Option<usize> {
    if path.starts_with('/') {
        return resolve_path(path, out);
    }
    let base = if dirfd == AT_FDCWD {
        process::linux_cwd()
    } else {
        process::dir_path(dirfd)?
    };
    let mut tmp = [0u8; 160];
    let bn = base.len().min(120);
    tmp[..bn].copy_from_slice(base.as_bytes());
    let mut n = bn;
    if n == 0 || tmp[n - 1] != b'/' {
        tmp[n] = b'/';
        n += 1;
    }
    let pn = path.len().min(tmp.len() - n - 1);
    tmp[n..n + pn].copy_from_slice(path.as_bytes());
    n += pn;
    let s = core::str::from_utf8(&tmp[..n]).ok()?;
    resolve_path(s, out)
}

enum Target {
    Dir,
    Disk(usize),
    Ram(usize),
    None,
}

fn classify(resolved: &str) -> Target {
    if resolved == "/" || resolved == "/disk" {
        return Target::Dir;
    }
    if let Some(name) = resolved.strip_prefix("/disk/") {
        if name.is_empty() {
            return Target::Dir;
        }
        if let Some(inode) = diskfs::lookup(name) {
            return match diskfs::inode_kind(inode) {
                Some(2) => Target::Dir,
                _ => Target::Disk(inode),
            };
        }
        return Target::None;
    }
    if let Some(index) = ramfs::index_of(resolved) {
        return Target::Ram(index);
    }
    Target::None
}

/// Visit the entries of a resolved directory path.
fn visit_dir(path: &str, mut f: impl FnMut(&str, bool)) {
    if path == "/disk" {
        diskfs::list_dir("", f);
    } else if let Some(sub) = path.strip_prefix("/disk/") {
        diskfs::list_dir(sub, f);
    } else if path == "/" {
        for file in ramfs::files() {
            f(file.name, false);
        }
        f("disk", true);
    }
}

/// Fill a 128-byte aarch64 `struct stat`.
fn fill_stat(buffer: &mut [u8], mode: u32, size: u64) {
    for b in buffer.iter_mut() {
        *b = 0;
    }
    if buffer.len() < 128 {
        return;
    }
    buffer[16..20].copy_from_slice(&mode.to_le_bytes()); // st_mode
    buffer[20..24].copy_from_slice(&1u32.to_le_bytes()); // st_nlink
    buffer[48..56].copy_from_slice(&size.to_le_bytes()); // st_size
    buffer[56..60].copy_from_slice(&4096u32.to_le_bytes()); // st_blksize
    let blocks = (size + 511) / 512;
    buffer[64..72].copy_from_slice(&blocks.to_le_bytes()); // st_blocks
}

// -- dispatch ---------------------------------------------------------------

/// Returns Some(result) to store in x0, or None when the call switched
/// processes (exit / blocking read) and the frame was reused.
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
        L_PWRITE64 => Some(l_pwrite(a0, a1, a2, a3)),
        L_SENDFILE => Some(l_sendfile(a0, a1, a2, a3)),
        L_READV => l_readv(frame, a0, a1, a2),
        L_OPENAT => Some(l_openat(a0, a1, a2)),
        L_CLOSE => Some(if process::close_fd(a0) { 0 } else { eno(9) }),
        L_GETDENTS64 => Some(l_getdents64(a0, a1, a2)),
        L_LSEEK => Some(l_lseek(a0, a1, a2)),
        L_FCNTL => Some(l_fcntl(a0, a1)),
        L_IOCTL => Some(l_ioctl(a0, a1, a2)),
        L_DUP => Some(process::fd_dup(a0, None).unwrap_or(eno(9))),
        L_DUP3 => Some(process::fd_dup(a0, Some(a1)).unwrap_or(eno(9))),
        L_BRK => Some(process::linux_brk(a0)),
        L_MMAP => Some(l_mmap(a1, a2, a3, a4, a5)),
        L_MUNMAP | L_MPROTECT | L_MADVISE => Some(0),
        L_GETPID | L_GETTID | L_SET_TID_ADDRESS => Some(process::sys_getpid()),
        L_GETUID | L_GETEUID | L_GETGID | L_GETEGID => Some(0),
        L_SET_ROBUST_LIST | L_SYSINFO => Some(l_sysinfo(a0)),
        L_RSEQ | L_STATX => Some(eno(38)),
        L_PRLIMIT64 | L_GETRLIMIT | L_SETRLIMIT => Some(0),
        L_UMASK => Some(0o022),
        L_FLOCK | L_FUTEX => Some(0),
        L_PIPE2 => Some(eno(38)),
        L_TKILL | L_TGKILL | L_SIGALTSTACK | L_RT_SIGACTION | L_RT_SIGPROCMASK
        | L_RT_SIGRETURN => Some(0),
        L_SCHED_GETAFFINITY => Some(l_getaffinity(a1, a2)),
        L_FACCESSAT => Some(l_faccessat(a0, a1)),
        L_CHDIR => Some(l_chdir(a0)),
        L_GETCWD => Some(l_getcwd(a0, a1)),
        L_MKDIRAT => Some(l_mkdirat(a0, a1)),
        L_UNLINKAT => Some(l_unlinkat(a0, a1, a2)),
        L_RENAMEAT => Some(l_renameat(a0, a1, a2, a3)),
        L_RENAMEAT2 => Some(l_renameat(a0, a1, a2, a3)),
        L_READLINKAT => Some(eno(2)),
        L_CLOCK_GETTIME => Some(l_clock_gettime(a0, a1)),
        L_GETTIMEOFDAY => Some(l_gettimeofday(a0)),
        L_GETRANDOM => Some(l_getrandom(a0, a1)),
        L_UNAME => Some(l_uname(a0)),
        L_FSTAT => Some(l_fstat_fd(a0, a1)),
        L_NEWFSTATAT => Some(l_newfstatat(a0, a1, a2)),
        _ => {
            println!("[linux] unhandled syscall {} (x0={:#x})", number, a0);
            Some(eno(38))
        }
    }
}

// -- I/O --------------------------------------------------------------------

fn l_write(fd: u64, pointer: u64, length: u64) -> u64 {
    let Some(bytes) = lbytes(pointer, length) else {
        return eno(14);
    };
    if fd == 1 || fd == 2 {
        uart::write_bytes(bytes);
        return length;
    }
    match process::fd_write(fd, bytes) {
        Some(n) => n as u64,
        None => eno(9),
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

fn l_read(frame: &mut ExceptionFrame, fd: u64, pointer: u64, length: u64) -> Option<u64> {
    let Some(buffer) = lbuffer(pointer, length) else {
        return Some(eno(14));
    };
    if fd == 0 {
        if let Some(n) = uart::take_line(buffer) {
            return Some(n as u64);
        }
        if process::reader_busy() {
            return Some(eno(11));
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

fn l_readv(frame: &mut ExceptionFrame, fd: u64, iov_ptr: u64, count: u64) -> Option<u64> {
    if count > 16 {
        return Some(eno(22));
    }
    let mut total = 0u64;
    for i in 0..count {
        let entry = iov_ptr + i * 16;
        let Some(base) = read_user_u64(entry) else {
            return Some(eno(14));
        };
        let Some(len) = read_user_u64(entry + 8) else {
            return Some(eno(14));
        };
        if len == 0 {
            continue;
        }
        match l_read(frame, fd, base, len) {
            Some(n) if n < 0x8000_0000_0000_0000 => {
                total += n;
                if n < len {
                    break;
                }
            }
            other => return other,
        }
    }
    Some(total)
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

fn l_pwrite(fd: u64, pointer: u64, length: u64, offset: u64) -> u64 {
    let Some(bytes) = lbytes(pointer, length) else {
        return eno(14);
    };
    let Some((kind, index)) = process::fd_target(fd) else {
        return eno(9);
    };
    if kind != process::FD_DISK {
        return eno(9);
    }
    match diskfs::write(index, offset, bytes) {
        Ok(n) => n as u64,
        Err(_) => eno(5),
    }
}

fn l_sendfile(out_fd: u64, in_fd: u64, offset_ptr: u64, count: u64) -> u64 {
    let Some((kind, index)) = process::fd_target(in_fd) else {
        return eno(9);
    };
    let mut off = if offset_ptr != 0 {
        match read_user_u64(offset_ptr) {
            Some(o) => o,
            None => return eno(14),
        }
    } else {
        match process::fd_pos(in_fd) {
            Some(p) => p,
            None => return eno(9),
        }
    };
    let mut buf = [0u8; 4096];
    let mut total = 0u64;
    let mut remaining = count;
    while remaining > 0 {
        let want = remaining.min(buf.len() as u64) as usize;
        let n = if kind == process::FD_DISK {
            diskfs::read(index, off, &mut buf[..want])
        } else {
            match ramfs::file_at(index) {
                Some(f) => ramfs::RAMFS.read(f, off as usize, &mut buf[..want]),
                None => return eno(9),
            }
        };
        if n == 0 {
            break;
        }
        let w = if out_fd == 1 || out_fd == 2 {
            uart::write_bytes(&buf[..n]);
            n
        } else {
            match process::fd_write(out_fd, &buf[..n]) {
                Some(w) => w,
                None => return eno(9),
            }
        };
        total += w as u64;
        off += n as u64;
        remaining -= n as u64;
        if w < n {
            break;
        }
    }
    if offset_ptr != 0 {
        if let Some(buffer) = lbuffer(offset_ptr, 8) {
            buffer.copy_from_slice(&off.to_le_bytes());
        }
    } else {
        process::fd_set_pos(in_fd, off);
    }
    total
}

fn l_lseek(fd: u64, offset: u64, whence: u64) -> u64 {
    let Some(pos) = process::fd_pos(fd) else {
        return eno(9);
    };
    let size = match process::fd_target(fd) {
        Some((process::FD_DISK, index)) => diskfs::inode_size(index).unwrap_or(0),
        Some((_, index)) => ramfs::file_at(index).map(|f| f.data.len() as u64).unwrap_or(0),
        _ => 0,
    };
    let off = offset as i64;
    let new = match whence {
        0 => off,
        1 => pos as i64 + off,
        2 => size as i64 + off,
        _ => return eno(22),
    };
    if new < 0 {
        return eno(22);
    }
    process::fd_set_pos(fd, new as u64);
    new as u64
}

fn l_fcntl(fd: u64, cmd: u64) -> u64 {
    match cmd {
        1 => 0,          // F_GETFD
        3 => 2,          // F_GETFL -> O_RDWR
        2 | 4 => 0,      // F_SETFD / F_SETFL
        _ => {
            let _ = fd;
            0
        }
    }
}

fn l_ioctl(fd: u64, request: u64, arg: u64) -> u64 {
    if request == 0x5413 {
        // TIOCGWINSZ: report the text console geometry.
        if let Some(buffer) = lbuffer(arg, 8) {
            buffer[0..2].copy_from_slice(&45u16.to_le_bytes()); // rows
            buffer[2..4].copy_from_slice(&80u16.to_le_bytes()); // cols
            buffer[4..8].copy_from_slice(&0u32.to_le_bytes());
            return 0;
        }
        return eno(14);
    }
    let _ = fd;
    eno(25) // ENOTTY
}

// -- directories ------------------------------------------------------------

fn l_openat(dirfd: u64, path_ptr: u64, flags: u64) -> u64 {
    let Some(bytes) = lcstr(path_ptr) else {
        return eno(14);
    };
    let Ok(path) = core::str::from_utf8(bytes) else {
        return eno(22);
    };
    let mut resolved = [0u8; 160];
    let Some(n) = resolve_at(dirfd, path, &mut resolved) else {
        return eno(2);
    };
    let r = core::str::from_utf8(&resolved[..n]).unwrap_or("");

    match classify(r) {
        Target::Dir => process::open_dir(r).unwrap_or(eno(24)),
        Target::Disk(inode) => process::open_fd(process::FD_DISK, inode).unwrap_or(eno(24)),
        Target::Ram(index) => process::open_fd(process::FD_RAM, index).unwrap_or(eno(24)),
        Target::None => {
            let o_creat = flags & 0o100 != 0;
            if o_creat {
                if let Some(name) = r.strip_prefix("/disk/") {
                    if let Some(inode) = diskfs::create(name) {
                        return process::open_fd(process::FD_DISK, inode).unwrap_or(eno(24));
                    }
                }
            }
            eno(2)
        }
    }
}

fn l_getdents64(fd: u64, pointer: u64, length: u64) -> u64 {
    let Some(buffer) = lbuffer(pointer, length) else {
        return eno(14);
    };
    let Some(path) = process::dir_path(fd) else {
        return eno(9);
    };
    let Some(start) = process::fd_pos(fd) else {
        return eno(9);
    };
    let mut skip = start;
    let mut written = 0usize;
    let mut emitted = 0u64;
    visit_dir(path, |name, is_dir| {
        if skip > 0 {
            skip -= 1;
            return;
        }
        let reclen = (19 + name.len() + 1 + 7) & !7;
        if written + reclen > buffer.len() {
            return;
        }
        let rec = &mut buffer[written..written + reclen];
        for b in rec.iter_mut() {
            *b = 0;
        }
        let ino = start + emitted + 1;
        rec[0..8].copy_from_slice(&ino.to_le_bytes()); // d_ino
        rec[8..16].copy_from_slice(&ino.to_le_bytes()); // d_off
        rec[16..18].copy_from_slice(&(reclen as u16).to_le_bytes()); // d_reclen
        rec[18] = if is_dir { DT_DIR } else { DT_REG }; // d_type
        rec[19..19 + name.len()].copy_from_slice(name.as_bytes()); // d_name
        written += reclen;
        emitted += 1;
    });
    process::fd_set_pos(fd, start + emitted);
    written as u64
}

// -- stat -------------------------------------------------------------------

fn l_fstat_fd(fd: u64, pointer: u64) -> u64 {
    let Some(buffer) = lbuffer(pointer, 128) else {
        return eno(14);
    };
    if fd <= 2 {
        fill_stat(buffer, S_IFCHR | 0o620, 0);
        return 0;
    }
    match process::fd_target(fd) {
        Some((process::FD_DISK, index)) => {
            let size = diskfs::inode_size(index).unwrap_or(0);
            let mode = if diskfs::inode_kind(index) == Some(2) {
                S_IFDIR | 0o755
            } else {
                S_IFREG | 0o644
            };
            fill_stat(buffer, mode, size);
            0
        }
        Some((process::FD_RAM, index)) => {
            let size = ramfs::file_at(index).map(|f| f.data.len() as u64).unwrap_or(0);
            fill_stat(buffer, S_IFREG | 0o644, size);
            0
        }
        _ => {
            if process::dir_path(fd).is_some() {
                fill_stat(buffer, S_IFDIR | 0o755, 4096);
                0
            } else {
                eno(9)
            }
        }
    }
}

fn l_newfstatat(dirfd: u64, path_ptr: u64, pointer: u64) -> u64 {
    let Some(bytes) = lcstr(path_ptr) else {
        return eno(14);
    };
    let Ok(path) = core::str::from_utf8(bytes) else {
        return eno(22);
    };
    let Some(buffer) = lbuffer(pointer, 128) else {
        return eno(14);
    };
    let mut resolved = [0u8; 160];
    let Some(n) = resolve_at(dirfd, path, &mut resolved) else {
        return eno(2);
    };
    let r = core::str::from_utf8(&resolved[..n]).unwrap_or("");
    match classify(r) {
        Target::Dir => {
            fill_stat(buffer, S_IFDIR | 0o755, 4096);
            0
        }
        Target::Disk(inode) => {
            let size = diskfs::inode_size(inode).unwrap_or(0);
            fill_stat(buffer, S_IFREG | 0o644, size);
            0
        }
        Target::Ram(index) => {
            let size = ramfs::file_at(index).map(|f| f.data.len() as u64).unwrap_or(0);
            fill_stat(buffer, S_IFREG | 0o644, size);
            0
        }
        Target::None => eno(2),
    }
}

// -- filesystem mutations ---------------------------------------------------

fn l_faccessat(dirfd: u64, path_ptr: u64) -> u64 {
    let Some(bytes) = lcstr(path_ptr) else {
        return eno(14);
    };
    let Ok(path) = core::str::from_utf8(bytes) else {
        return eno(22);
    };
    let mut resolved = [0u8; 160];
    let Some(n) = resolve_at(dirfd, path, &mut resolved) else {
        return eno(2);
    };
    let r = core::str::from_utf8(&resolved[..n]).unwrap_or("");
    match classify(r) {
        Target::None => eno(2),
        _ => 0,
    }
}

fn l_chdir(path_ptr: u64) -> u64 {
    let Some(bytes) = lcstr(path_ptr) else {
        return eno(14);
    };
    let Ok(path) = core::str::from_utf8(bytes) else {
        return eno(22);
    };
    let mut resolved = [0u8; 160];
    let Some(n) = resolve_path(path, &mut resolved) else {
        return eno(2);
    };
    let r = core::str::from_utf8(&resolved[..n]).unwrap_or("");
    match classify(r) {
        Target::Dir => {
            process::set_cwd(r);
            0
        }
        Target::None => eno(2),
        _ => eno(20), // ENOTDIR
    }
}

fn l_getcwd(pointer: u64, length: u64) -> u64 {
    let cwd = process::linux_cwd();
    let need = cwd.len() + 1;
    if length < need as u64 {
        return eno(34); // ERANGE
    }
    let Some(buffer) = lbuffer(pointer, need as u64) else {
        return eno(14);
    };
    buffer[..cwd.len()].copy_from_slice(cwd.as_bytes());
    buffer[cwd.len()] = 0;
    pointer
}

fn l_mkdirat(dirfd: u64, path_ptr: u64) -> u64 {
    let Some(bytes) = lcstr(path_ptr) else {
        return eno(14);
    };
    let Ok(path) = core::str::from_utf8(bytes) else {
        return eno(22);
    };
    let mut resolved = [0u8; 160];
    let Some(n) = resolve_at(dirfd, path, &mut resolved) else {
        return eno(2);
    };
    let r = core::str::from_utf8(&resolved[..n]).unwrap_or("");
    if let Some(name) = r.strip_prefix("/disk/") {
        if diskfs::mkdir(name) {
            return 0;
        }
        return eno(17); // EEXIST
    }
    eno(30) // EROFS (ramfs)
}

fn l_unlinkat(dirfd: u64, path_ptr: u64, flags: u64) -> u64 {
    let Some(bytes) = lcstr(path_ptr) else {
        return eno(14);
    };
    let Ok(path) = core::str::from_utf8(bytes) else {
        return eno(22);
    };
    let mut resolved = [0u8; 160];
    let Some(n) = resolve_at(dirfd, path, &mut resolved) else {
        return eno(2);
    };
    let r = core::str::from_utf8(&resolved[..n]).unwrap_or("");
    if let Some(name) = r.strip_prefix("/disk/") {
        let ok = if flags & 0x200 != 0 {
            diskfs::rmdir(name)
        } else {
            diskfs::unlink(name)
        };
        return if ok { 0 } else { eno(2) };
    }
    eno(30)
}

fn l_renameat(olddirfd: u64, old_ptr: u64, newdirfd: u64, new_ptr: u64) -> u64 {
    let Some(ob) = lcstr(old_ptr) else {
        return eno(14);
    };
    let Some(nb) = lcstr(new_ptr) else {
        return eno(14);
    };
    let Ok(oldp) = core::str::from_utf8(ob) else {
        return eno(22);
    };
    let Ok(newp) = core::str::from_utf8(nb) else {
        return eno(22);
    };
    let mut oldr = [0u8; 160];
    let mut newr = [0u8; 160];
    let Some(on) = resolve_at(olddirfd, oldp, &mut oldr) else {
        return eno(2);
    };
    let Some(nn) = resolve_at(newdirfd, newp, &mut newr) else {
        return eno(2);
    };
    let o = core::str::from_utf8(&oldr[..on]).unwrap_or("");
    let nw = core::str::from_utf8(&newr[..nn]).unwrap_or("");
    if let (Some(oldname), Some(newname)) = (o.strip_prefix("/disk/"), nw.strip_prefix("/disk/")) {
        return if diskfs::rename(oldname, newname) { 0 } else { eno(2) };
    }
    eno(30)
}

// -- misc -------------------------------------------------------------------

fn l_mmap(length: u64, prot: u64, flags: u64, fd: u64, _offset: u64) -> u64 {
    let anonymous = flags & 0x20 != 0;
    if !anonymous || fd != u64::MAX {
        return eno(38);
    }
    let executable = prot & 0x4 != 0;
    match process::linux_mmap(length, executable) {
        Some(address) => address,
        None => eno(12),
    }
}

fn l_clock_gettime(_clock: u64, pointer: u64) -> u64 {
    let Some(buffer) = lbuffer(pointer, 16) else {
        return eno(14);
    };
    let ms = crate::timer::uptime_ms();
    buffer[..8].copy_from_slice(&(ms / 1000).to_le_bytes());
    buffer[8..16].copy_from_slice(&((ms % 1000) * 1_000_000).to_le_bytes());
    0
}

fn l_gettimeofday(pointer: u64) -> u64 {
    let Some(buffer) = lbuffer(pointer, 16) else {
        return eno(14);
    };
    let ms = crate::timer::uptime_ms();
    buffer[..8].copy_from_slice(&(ms / 1000).to_le_bytes());
    buffer[8..16].copy_from_slice(&((ms % 1000) * 1000).to_le_bytes());
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

fn l_getaffinity(length: u64, pointer: u64) -> u64 {
    let Some(buffer) = lbuffer(pointer, length.min(8)) else {
        return eno(14);
    };
    for b in buffer.iter_mut() {
        *b = 0;
    }
    buffer[0] = 1; // CPU0 only
    0
}

fn l_sysinfo(pointer: u64) -> u64 {
    if pointer == 0 {
        return 0;
    }
    let Some(buffer) = lbuffer(pointer, 112) else {
        return eno(14);
    };
    for b in buffer.iter_mut() {
        *b = 0;
    }
    let uptime = crate::timer::uptime_ms() / 1000;
    buffer[0..8].copy_from_slice(&uptime.to_le_bytes());
    let totalram: u64 = 128 * 1024 * 1024;
    buffer[32..40].copy_from_slice(&totalram.to_le_bytes());
    buffer[40..48].copy_from_slice(&(totalram / 2).to_le_bytes());
    buffer[104..108].copy_from_slice(&1u32.to_le_bytes()); // mem_unit
    0
}

fn l_uname(pointer: u64) -> u64 {
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