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
pub const SYS_FB_INFO: u64 = 16;
pub const SYS_FB_FILL: u64 = 17;
pub const SYS_UNLINK: u64 = 18;
pub const SYS_FREEBLOCKS: u64 = 19;
pub const SYS_UPTIME: u64 = 20;
pub const SYS_SLEEP: u64 = 21;
pub const SYS_BRK: u64 = 22;
pub const SYS_PS: u64 = 23;
pub const SYS_KILL: u64 = 24;
pub const SYS_RENAME: u64 = 25;
pub const SYS_RTC: u64 = 26;
pub const SYS_STAT: u64 = 27;
pub const SYS_LISTDIR: u64 = 28;
pub const SYS_MKDIR: u64 = 29;
pub const SYS_RMDIR: u64 = 30;
pub const SYS_FB_MAP: u64 = 31;
pub const SYS_INPUT_POLL: u64 = 32;
pub const SYS_INPUT_WAIT: u64 = 33;
pub const SYS_NET: u64 = 34;
pub const SYS_SPAWN_ARGS: u64 = 35;
pub const SYS_FB_BACK: u64 = 36;
pub const SYS_FB_PRESENT: u64 = 37;

/// Coarse user-accessible window: ELF, stack, PE image/thunks/stack.
const USER_SPACE_START: u64 = 0x6000_0000;
const USER_SPACE_END: u64 = 0x6800_0000;

const ERROR: u64 = u64::MAX;
const MAX_USER_TRANSFER: u64 = 8192;

fn user_range(pointer: u64, length: u64) -> bool {
    let Some(end) = pointer.checked_add(length) else {
        return false;
    };
    pointer >= USER_SPACE_START
        && end <= USER_SPACE_END
        && process::user_range_ok(pointer, length)
}

fn user_bytes(pointer: u64, length: u64) -> Option<&'static [u8]> {
    if pointer == 0
        || length == 0
        || length > MAX_USER_TRANSFER
        || !user_range(pointer, length)
    {
        return None;
    }
    // SAFETY: the caller is an EL0 process whose mappings are trusted by this
    // toy kernel; the length is bounded above.
    Some(unsafe { core::slice::from_raw_parts(pointer as *const u8, length as usize) })
}

fn user_u64_buffer(pointer: u64, count: u64) -> Option<&'static mut [u64]> {
    let bytes = count.checked_mul(8)?;
    let end = pointer.checked_add(bytes)?;
    if pointer < USER_SPACE_START
        || end > USER_SPACE_END
        || pointer & 7 != 0
        || !process::user_range_ok(pointer, bytes)
    {
        return None;
    }
    // SAFETY: the pointer lies in the user window; the caller is an EL0
    // process and the length is bounded.
    Some(unsafe { core::slice::from_raw_parts_mut(pointer as *mut u64, count as usize) })
}

fn user_buffer(pointer: u64, length: u64) -> Option<&'static mut [u8]> {
    if pointer == 0
        || length == 0
        || length > MAX_USER_TRANSFER
        || !user_range(pointer, length)
    {
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

pub enum ReadOutcome {
    Done(u64),
    Switched,
}

/// read(fd, buf, len) -> bytes read. Consumes a complete line produced by the
/// console IRQ, blocking (and yielding the CPU) until one is available.
pub fn sys_read(
    frame: &mut ExceptionFrame,
    fd: u64,
    pointer: u64,
    length: u64,
) -> ReadOutcome {
    if fd != 0 {
        return ReadOutcome::Done(ERROR);
    }
    let Some(buffer) = user_buffer(pointer, length) else {
        return ReadOutcome::Done(ERROR);
    };

    if let Some(count) = crate::uart::take_line(buffer) {
        return ReadOutcome::Done(count as u64);
    }

    // Only one console reader can be blocked at a time.
    if process::reader_busy() {
        return ReadOutcome::Done(ERROR);
    }

    // Yield to another process; on wake the SVC re-executes and retries.
    if process::block_on_read(frame) {
        return ReadOutcome::Switched;
    }

    // Nothing else is runnable: idle in the kernel until a line arrives.
    crate::uart::wait_for_line();
    let count = crate::uart::take_line(buffer).unwrap_or(0);
    ReadOutcome::Done(count as u64)
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
    diskfs::list_dir("", |name, is_dir| {
        let mut path = [0u8; 48];
        path[..6].copy_from_slice(b"/disk/");
        let length = core::cmp::min(name.len(), path.len() - 7);
        path[6..6 + length].copy_from_slice(&name.as_bytes()[..length]);
        let mut end = 6 + length;
        if is_dir && end < path.len() {
            path[end] = b'/';
            end += 1;
        }
        if let Ok(path) = core::str::from_utf8(&path[..end]) {
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

/// spawn_args(path_ptr, path_len, args_ptr, args_len, argc) -> child PID.
/// `args` is a NUL-separated blob of argc argument strings (Linux argv[1..]).
pub fn sys_spawn_args(
    path_ptr: u64,
    path_len: u64,
    args_ptr: u64,
    args_len: u64,
    argc: u64,
) -> u64 {
    with_user_path(path_ptr, path_len, |path| {
        let mut args: [&str; 16] = [""; 16];
        let mut count = 0usize;
        if let Some(blob) = user_bytes(args_ptr, args_len) {
            for part in blob.split(|&b| b == 0) {
                if count >= 16 || count >= argc as usize {
                    break;
                }
                if let Ok(s) = core::str::from_utf8(part) {
                    if !s.is_empty() {
                        args[count] = s;
                        count += 1;
                    }
                }
            }
        }
        process::sys_spawn_args(path, &args[..count])
    })
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

/// unlink("/disk/name") -> 0 on success, u64::MAX otherwise.
pub fn sys_unlink(pointer: u64, length: u64) -> u64 {
    with_user_path(pointer, length, |path| {
        let Some(name) = path.strip_prefix("/disk/") else {
            return ERROR;
        };
        if diskfs::unlink(name) {
            0
        } else {
            ERROR
        }
    })
}

/// Milliseconds since boot (generic counter).
pub fn sys_uptime() -> u64 {
    crate::timer::uptime_ms()
}

/// net(op, ...): networking operations.
///   0 status(timeout)                 -> 0 if an IPv4 lease is configured
///   1 resolve(name_ptr,len,out_ptr)   -> 0, writes 4-byte IPv4 to out_ptr
///   2 tcp_connect(ip_u32,port,timeout)-> slot index, or ERROR
///   3 tcp_send(slot,buf,len)          -> bytes accepted
///   4 tcp_recv(slot,buf,len,timeout)  -> bytes read (0 = timeout/close)
///   5 tcp_close(slot)                 -> 0
pub fn sys_net(op: u64, a0: u64, a1: u64, a2: u64, a3: u64) -> u64 {
    match op {
        0 => {
            if !crate::net::present() {
                println!("net: no interface");
                return 1;
            }
            let timeout = if a0 == 0 { 5000 } else { a0 };
            crate::net::wait_for_ip(timeout);
            crate::net::print_status();
            if crate::net::ipv4().is_some() {
                0
            } else {
                1
            }
        }
        1 => {
            let Some(bytes) = user_bytes(a0, a1) else {
                return ERROR;
            };
            let Ok(name) = core::str::from_utf8(bytes) else {
                return ERROR;
            };
            let Some(out) = user_buffer(a2, 4) else {
                return ERROR;
            };
            let timeout = if a3 == 0 { 5000 } else { a3 };
            match crate::net::resolve(name, timeout) {
                Some(ip) => {
                    out.copy_from_slice(&ip.octets());
                    0
                }
                None => ERROR,
            }
        }
        2 => {
            let ip = smoltcp::wire::Ipv4Address::new(
                (a0 >> 24) as u8,
                (a0 >> 16) as u8,
                (a0 >> 8) as u8,
                a0 as u8,
            );
            let timeout = if a2 == 0 { 5000 } else { a2 };
            match crate::net::tcp_connect(ip, a1 as u16, timeout) {
                Some(slot) => slot as u64,
                None => ERROR,
            }
        }
        3 => {
            let Some(data) = user_bytes(a1, a2) else {
                return ERROR;
            };
            crate::net::tcp_send(a0 as usize, data) as u64
        }
        4 => {
            let Some(out) = user_buffer(a1, a2) else {
                return ERROR;
            };
            let timeout = if a3 == 0 { 5000 } else { a3 };
            crate::net::tcp_recv(a0 as usize, out, timeout) as u64
        }
        5 => {
            crate::net::tcp_close(a0 as usize);
            0
        }
        _ => ERROR,
    }
}

/// sleep(ms): block until the deadline; dispatching is handled by the caller.
pub fn sys_sleep(frame: &mut ExceptionFrame, ms: u64) -> process::SleepOutcome {
    process::sys_sleep(frame, ms)
}

/// rename(old, new) for `/disk/...` paths; 0 on success.
pub fn sys_rename(
    old_pointer: u64,
    old_length: u64,
    new_pointer: u64,
    new_length: u64,
) -> u64 {
    let Some(old_bytes) = user_bytes(old_pointer, old_length) else {
        return ERROR;
    };
    let Some(new_bytes) = user_bytes(new_pointer, new_length) else {
        return ERROR;
    };
    let (Ok(old), Ok(new)) = (
        core::str::from_utf8(old_bytes),
        core::str::from_utf8(new_bytes),
    ) else {
        return ERROR;
    };
    let (Some(old), Some(new)) =
        (old.strip_prefix("/disk/"), new.strip_prefix("/disk/"))
    else {
        return ERROR;
    };
    if diskfs::rename(old, new) {
        0
    } else {
        ERROR
    }
}

/// stat(path, out): write `[kind, size, blocks]` (kind 0 RamFS, 1 disk).
pub fn sys_stat(pointer: u64, length: u64, out_pointer: u64) -> u64 {
    let Some(out) = user_u64_buffer(out_pointer, 3) else {
        return ERROR;
    };
    with_user_path(pointer, length, |path| {
        if let Some(file) = ramfs::index_of(path).and_then(ramfs::file_at) {
            out[0] = 0;
            out[1] = file.data.len() as u64;
            out[2] = 0;
            return 0;
        }
        if let Some(name) = path.strip_prefix("/disk/") {
            if let Some((kind, size, blocks)) = diskfs::stat(name) {
                out[0] = kind;
                out[1] = size;
                out[2] = blocks;
                return 0;
            }
        }
        ERROR
    })
}

/// rtc(out): write `[epoch, year, month, day, hour, minute, second]`.
pub fn sys_rtc(pointer: u64) -> u64 {
    let Some(epoch) = crate::rtc::epoch_seconds() else {
        return ERROR;
    };
    let Some(out) = user_u64_buffer(pointer, 7) else {
        return ERROR;
    };
    let (year, month, day, hour, minute, second) = crate::rtc::civil(epoch);
    out[0] = epoch;
    out[1] = year;
    out[2] = month;
    out[3] = day;
    out[4] = hour;
    out[5] = minute;
    out[6] = second;
    0
}

/// ps(out, capacity): write process records, return the count.
pub fn sys_ps(pointer: u64, capacity: u64) -> u64 {
    let entries = capacity.min(process::MAX_PROCESSES as u64) as usize;
    if entries == 0 {
        return ERROR;
    }
    let Some(out) = user_u64_buffer(pointer, (entries * 4) as u64) else {
        return ERROR;
    };
    process::snapshot(out) as u64
}

/// kill(pid): terminate another process.
pub fn sys_kill(pid: u64) -> u64 {
    process::sys_kill(pid)
}

/// brk(request): query (0) or extend the user heap.
pub fn sys_brk(request: u64) -> u64 {
    process::sys_brk(request)
}

/// Map "/disk" and "/disk/..." to a disk-relative path.
fn disk_relative(path: &str) -> Option<&str> {
    if path == "/disk" {
        Some("")
    } else {
        path.strip_prefix("/disk/")
    }
}

/// listdir(path, out, len) -> bytes written; each entry is NUL-terminated.
pub fn sys_listdir(
    pointer: u64,
    length: u64,
    out_pointer: u64,
    out_length: u64,
) -> u64 {
    let Some(buffer) = user_buffer(out_pointer, out_length) else {
        return ERROR;
    };
    with_user_path(pointer, length, |path| {
        let Some(name) = disk_relative(path) else {
            return ERROR;
        };
        let mut written = 0;
        {
            let mut push = |child: &str, is_dir: bool| {
                for &byte in child.as_bytes() {
                    if written < buffer.len() {
                        buffer[written] = byte;
                        written += 1;
                    }
                }
                if is_dir && written < buffer.len() {
                    buffer[written] = b'/';
                    written += 1;
                }
                if written < buffer.len() {
                    buffer[written] = 0;
                    written += 1;
                }
            };
            diskfs::list_dir(name, &mut push);
        }
        written as u64
    })
}

/// mkdir("/disk/...") -> 0 on success.
pub fn sys_mkdir(pointer: u64, length: u64) -> u64 {
    with_user_path(pointer, length, |path| {
        let Some(name) = disk_relative(path) else {
            return ERROR;
        };
        if diskfs::mkdir(name) {
            0
        } else {
            ERROR
        }
    })
}

/// rmdir("/disk/...") -> 0 on success.
pub fn sys_rmdir(pointer: u64, length: u64) -> u64 {
    with_user_path(pointer, length, |path| {
        let Some(name) = disk_relative(path) else {
            return ERROR;
        };
        if diskfs::rmdir(name) {
            0
        } else {
            ERROR
        }
    })
}

pub enum InputOutcome {
    Done(u64),
    Switched,
}

/// input_poll() -> packed event or u64::MAX.
pub fn sys_input_poll() -> u64 {
    crate::input::pop()
        .map(crate::input::pack)
        .unwrap_or(ERROR)
}

/// input_wait() -> packed event; blocks until one is queued.
pub fn sys_input_wait(frame: &mut ExceptionFrame) -> InputOutcome {
    if let Some(event) = crate::input::pop() {
        return InputOutcome::Done(crate::input::pack(event));
    }
    if process::block_on_input(frame) {
        return InputOutcome::Switched;
    }
    crate::input::wait_for_event();
    InputOutcome::Done(
        crate::input::pop()
            .map(crate::input::pack)
            .unwrap_or(ERROR),
    )
}

/// fb_map() -> user address of the framebuffer, or u64::MAX.
pub fn sys_fb_map() -> u64 {
    if !crate::fb::ready() {
        return ERROR;
    }
    process::map_framebuffer().unwrap_or(ERROR)
}

/// Map the offscreen back buffer (double-buffered GUI). Returns its address.
pub fn sys_fb_back() -> u64 {
    if !crate::fb::ready() {
        return ERROR;
    }
    process::map_backbuffer().unwrap_or(ERROR)
}

/// Present the back buffer to the live framebuffer (one fast copy).
pub fn sys_fb_present() -> u64 {
    crate::fb::present();
    0
}

/// Number of free data blocks, or u64::MAX without a mounted disk.
pub fn sys_freeblocks() -> u64 {
    diskfs::free_blocks().unwrap_or(ERROR)
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

/// fb_info(out): write `[width, height, stride, format]` to user memory.
pub fn sys_fb_info(pointer: u64) -> u64 {
    let Some((width, height, stride, format)) = crate::fb::info() else {
        return ERROR;
    };
    let Some(out) = user_u64_buffer(pointer, 4) else {
        return ERROR;
    };
    out[0] = width as u64;
    out[1] = height as u64;
    out[2] = stride as u64;
    out[3] = format as u64;
    0
}

/// fb_fill(x, y, width, height, color) -> 0, or u64::MAX without a display.
pub fn sys_fb_fill(x: u64, y: u64, width: u64, height: u64, color: u64) -> u64 {
    let clipped = |value: u64| usize::try_from(value).unwrap_or(usize::MAX);
    if crate::fb::fill_rect(
        clipped(x),
        clipped(y),
        clipped(width),
        clipped(height),
        color as u32,
    ) {
        0
    } else {
        ERROR
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

/// WriteConsoleA(handle, buffer, length, written, reserved) -> BOOL.
pub fn pe_write_console_a(
    handle: u64,
    pointer: u64,
    length: u64,
    written_pointer: u64,
    _reserved: u64,
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

/// GetTickCount64(): milliseconds since boot.
pub fn pe_get_tick_count_64() -> u64 {
    crate::timer::uptime_ms()
}

/// GetTickCount(): 32-bit milliseconds since boot.
pub fn pe_get_tick_count() -> u64 {
    crate::timer::uptime_ms() & 0xffff_ffff
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
