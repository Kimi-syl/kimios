//! libkimi: user-space syscall wrappers for KimiOS.
//!
//! Syscall ABI: `x8` = number, `x0..x3` = arguments, result in `x0`.
//! All pointers passed in arguments must refer to memory mapped in the
//! calling process.

#![no_std]

pub mod apk;
pub mod dvm;
pub mod kpkg;
pub mod shell;

#[cfg(target_os = "none")]
use core::arch::asm;

/// Library-OS host backend. When libkimi is compiled for a hosted target
/// (iOS / Linux / macOS) instead of bare metal, every syscall is routed to a
/// C-ABI dispatcher supplied by the embedding runtime rather than trapping to
/// EL1 via `svc #0`. The dispatcher implements the same syscall numbers using
/// host primitives (Metal/UIKit framebuffer, touches, mach time, sandboxed
/// filesystem, BSD sockets). The entire userland above this line is unchanged.
#[cfg(not(target_os = "none"))]
extern "C" {
    fn kimi_host_syscall(number: u64, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> u64;
    fn kimi_host_cntpct() -> u64;
}

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
pub const SYS_GFX_BENCH: u64 = 38;
pub const SYS_INSTR_MAP: u64 = 39;

/// Latency-mailbox slot indices (shared with the kernel `instr` module and the
/// Android shared-memory reader).
pub const IBOX_MAGIC: usize = 0;
pub const IBOX_SEQ: usize = 1;
pub const IBOX_CNTFRQ: usize = 2;
pub const IBOX_HB: usize = 3;
pub const IBOX_INPUT: usize = 4;
pub const IBOX_HANDLER: usize = 5;
pub const IBOX_DRAWN: usize = 6;
pub const IBOX_PRESENT: usize = 7;
pub const IBOX_RECT: usize = 8;
pub const IBOX_IVAL: usize = 9;
pub const IBOX_ACCUM: usize = 10;
pub const IBOX_ACK: usize = 11;
pub const IBOX_FRAME: usize = 12;

pub const EV_KEY: u16 = 1;
pub const EV_REL: u16 = 2;
pub const EV_ABS: u16 = 3;
pub const ABS_X: u16 = 0;
pub const ABS_Y: u16 = 1;
pub const BTN_LEFT: u16 = 0x110;
pub const BTN_TOUCH: u16 = 0x14a;

pub const KEY_ESC: u16 = 1;
pub const KEY_ENTER: u16 = 28;
pub const KEY_SPACE: u16 = 57;
pub const KEY_UP: u16 = 103;

#[derive(Clone, Copy)]
pub struct InputEvent {
    pub kind: u16,
    pub code: u16,
    pub value: u32,
}

fn unpack(raw: u64) -> InputEvent {
    InputEvent {
        kind: (raw >> 48) as u16,
        code: ((raw >> 32) & 0xffff) as u16,
        value: raw as u32,
    }
}

/// Non-blocking event fetch; `None` when the queue is empty.
pub fn input_poll() -> Option<InputEvent> {
    let raw = syscall3(SYS_INPUT_POLL, 0, 0, 0);
    (raw != ERROR).then(|| unpack(raw))
}

/// Block until an input event is available.
pub fn input_wait() -> Option<InputEvent> {
    let raw = syscall3(SYS_INPUT_WAIT, 0, 0, 0);
    (raw != ERROR).then(|| unpack(raw))
}

pub const STDIN: u64 = 0;
pub const STDOUT: u64 = 1;

const ERROR: u64 = u64::MAX;

#[cfg(target_os = "none")]
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

#[cfg(not(target_os = "none"))]
#[inline(always)]
fn syscall3(number: u64, arg0: u64, arg1: u64, arg2: u64) -> u64 {
    unsafe { kimi_host_syscall(number, arg0, arg1, arg2, 0, 0) }
}

#[cfg(target_os = "none")]
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

#[cfg(not(target_os = "none"))]
#[inline(always)]
fn syscall4(number: u64, arg0: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    unsafe { kimi_host_syscall(number, arg0, arg1, arg2, arg3, 0) }
}

#[cfg(target_os = "none")]
#[inline(always)]
fn syscall5(number: u64, arg0: u64, arg1: u64, arg2: u64, arg3: u64, arg4: u64) -> u64 {
    let result;
    unsafe {
        asm!(
            "svc #0",
            in("x8") number,
            inlateout("x0") arg0 => result,
            in("x1") arg1,
            in("x2") arg2,
            in("x3") arg3,
            in("x4") arg4,
        );
    }
    result
}

#[cfg(not(target_os = "none"))]
#[inline(always)]
fn syscall5(number: u64, arg0: u64, arg1: u64, arg2: u64, arg3: u64, arg4: u64) -> u64 {
    unsafe { kimi_host_syscall(number, arg0, arg1, arg2, arg3, arg4) }
}

pub fn write(fd: u64, buffer: &[u8]) -> u64 {
    syscall3(SYS_WRITE, fd, buffer.as_ptr() as u64, buffer.len() as u64)
}

pub fn print(text: &str) {
    print_bytes(text.as_bytes());
}

pub fn print_bytes(bytes: &[u8]) {
    if CAP_ACTIVE.load(core::sync::atomic::Ordering::SeqCst) {
        capture_push(bytes);
    } else {
        write(STDOUT, bytes);
    }
}

/// Console capture: while active, `print*` output is appended to an internal
/// buffer instead of the console, so a GUI can run a command and display its
/// output itself. Single-threaded per process; not re-entrant.
const CAP_SIZE: usize = 8192;
static CAP_ACTIVE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static mut CAP_LEN: usize = 0;
static mut CAP_BUF: [u8; CAP_SIZE] = [0; CAP_SIZE];

/// Start redirecting `print*` output into the capture buffer (clears it).
pub fn capture_begin() {
    unsafe {
        CAP_LEN = 0;
    }
    CAP_ACTIVE.store(true, core::sync::atomic::Ordering::SeqCst);
}

/// Stop redirecting; subsequent `print*` output goes to the console again.
pub fn capture_end() {
    CAP_ACTIVE.store(false, core::sync::atomic::Ordering::SeqCst);
}

/// Drain up to `out.len()` captured bytes, resetting the buffer. Returns the
/// number of bytes copied. Call repeatedly until it returns 0.
pub fn capture_take(out: &mut [u8]) -> usize {
    unsafe {
        let len = CAP_LEN;
        let n = len.min(out.len());
        core::ptr::copy_nonoverlapping(CAP_BUF.as_ptr(), out.as_mut_ptr(), n);
        CAP_LEN = 0;
        n
    }
}

fn capture_push(bytes: &[u8]) {
    unsafe {
        let len = CAP_LEN;
        let n = bytes.len().min(CAP_SIZE - len);
        let buf = core::ptr::addr_of_mut!(CAP_BUF) as *mut u8;
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), buf.add(len), n);
        CAP_LEN = len + n;
    }
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

/// Create a child process from an ELF or PE at `path`.
pub fn spawn(path: &str) -> Option<u64> {
    let result = syscall3(
        SYS_SPAWN,
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

/// Create a child process with command-line arguments (Linux argv[1..]).
pub fn spawn_args(path: &str, args: &[&str]) -> Option<u64> {
    let mut blob = [0u8; 256];
    let mut n = 0usize;
    let mut count = 0u64;
    for a in args {
        if n + a.len() + 1 > blob.len() {
            break;
        }
        blob[n..n + a.len()].copy_from_slice(a.as_bytes());
        n += a.len();
        blob[n] = 0;
        n += 1;
        count += 1;
    }
    let result = syscall5(
        SYS_SPAWN_ARGS,
        path.as_ptr() as u64,
        path.len() as u64,
        blob.as_ptr() as u64,
        n as u64,
        count,
    );
    if result == ERROR {
        None
    } else {
        Some(result)
    }
}

/// Wait for `pid` (0 = any child); returns its exit status.
pub fn wait(pid: u64) -> Option<u64> {
    let result = syscall3(SYS_WAIT, pid, 0, 0);
    if result == ERROR {
        None
    } else {
        Some(result)
    }
}

pub fn getpid() -> u64 {
    syscall3(SYS_GETPID, 0, 0, 0)
}

/// Create or truncate `/disk/<name>`; returns a descriptor.
pub fn create(path: &str) -> Option<u64> {
    let result = syscall3(
        SYS_CREATE,
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

pub fn write_file(handle: u64, offset: usize, data: &[u8]) -> usize {
    let result = syscall4(
        SYS_WRITE_FILE,
        handle,
        offset as u64,
        data.as_ptr() as u64,
        data.len() as u64,
    );
    if result == ERROR {
        0
    } else {
        result as usize
    }
}

/// Packed `(total_sectors << 32) | file_count`; `None` without a disk.
pub fn diskinfo() -> Option<u64> {
    let result = syscall3(SYS_DISKINFO, 0, 0, 0);
    if result == ERROR {
        None
    } else {
        Some(result)
    }
}

/// Milliseconds since boot.
pub fn uptime_ms() -> u64 {
    syscall3(SYS_UPTIME, 0, 0, 0)
}

/// Bring up networking (DHCP) and print status. Returns 0 if configured.
pub fn net_status(timeout_ms: u64) -> u64 {
    syscall5(SYS_NET, 0, timeout_ms, 0, 0, 0)
}

/// Resolve a hostname to a big-endian IPv4 address.
pub fn net_resolve(name: &str) -> Option<u32> {
    let mut out = [0u8; 4];
    let result = syscall5(
        SYS_NET, 1,
        name.as_ptr() as u64, name.len() as u64,
        out.as_mut_ptr() as u64, 5000,
    );
    if result == ERROR { None } else { Some(u32::from_be_bytes(out)) }
}

/// Open a TCP connection; returns a slot index.
pub fn net_tcp_connect(ip: u32, port: u16, timeout_ms: u64) -> Option<u64> {
    let result = syscall5(SYS_NET, 2, ip as u64, port as u64, timeout_ms, 0);
    if result == ERROR { None } else { Some(result) }
}

pub fn net_tcp_send(slot: u64, data: &[u8]) -> usize {
    syscall5(SYS_NET, 3, slot, data.as_ptr() as u64, data.len() as u64, 0) as usize
}

pub fn net_tcp_recv(slot: u64, buffer: &mut [u8], timeout_ms: u64) -> usize {
    syscall5(SYS_NET, 4, slot, buffer.as_mut_ptr() as u64, buffer.len() as u64, timeout_ms) as usize
}

pub fn net_tcp_close(slot: u64) {
    syscall5(SYS_NET, 5, slot, 0, 0, 0);
}

/// Sleep for at least `ms` milliseconds (other processes keep running).
pub fn sleep_ms(ms: u64) {
    syscall3(SYS_SLEEP, ms, 0, 0);
}

/// Remove a `/disk/...` file.
pub fn unlink(path: &str) -> bool {
    syscall3(SYS_UNLINK, path.as_ptr() as u64, path.len() as u64, 0) == 0
}

/// Free data blocks, or `None` without a mounted disk.
pub fn free_blocks() -> Option<u64> {
    let result = syscall3(SYS_FREEBLOCKS, 0, 0, 0);
    if result == ERROR {
        None
    } else {
        Some(result)
    }
}

/// Fill `out` with `[pid, parent, state, exit_status]` records; returns the
/// number of processes written.
pub fn ps(out: &mut [u64]) -> usize {
    let result = syscall3(SYS_PS, out.as_mut_ptr() as u64, out.len() as u64, 0);
    if result == ERROR {
        0
    } else {
        result as usize
    }
}

/// Terminate `pid`; returns true when it was marked a zombie.
pub fn kill(pid: u64) -> bool {
    syscall3(SYS_KILL, pid, 0, 0) == 0
}

/// Rename a `/disk/...` file.
pub fn rename(old: &str, new: &str) -> bool {
    syscall4(
        SYS_RENAME,
        old.as_ptr() as u64,
        old.len() as u64,
        new.as_ptr() as u64,
        new.len() as u64,
    ) == 0
}

/// List a `/disk/...` directory into `buffer` (NUL-separated, dirs end in
/// `/`). Returns the number of bytes written.
pub fn listdir(path: &str, buffer: &mut [u8]) -> Option<usize> {
    let result = syscall4(
        SYS_LISTDIR,
        path.as_ptr() as u64,
        path.len() as u64,
        buffer.as_mut_ptr() as u64,
        buffer.len() as u64,
    );
    if result == ERROR {
        None
    } else {
        Some(result as usize)
    }
}

/// Create a `/disk/...` directory.
pub fn mkdir(path: &str) -> bool {
    syscall3(SYS_MKDIR, path.as_ptr() as u64, path.len() as u64, 0) == 0
}

/// Remove an empty `/disk/...` directory.
pub fn rmdir(path: &str) -> bool {
    syscall3(SYS_RMDIR, path.as_ptr() as u64, path.len() as u64, 0) == 0
}

/// File metadata: `(kind, size)` where kind 0 is RamFS and 1 is a disk file.
pub fn stat(path: &str) -> Option<(u64, u64)> {
    let mut fields = [0u64; 3];
    let result = syscall4(
        SYS_STAT,
        path.as_ptr() as u64,
        path.len() as u64,
        fields.as_mut_ptr() as u64,
        0,
    );
    if result == ERROR {
        None
    } else {
        Some((fields[0], fields[1]))
    }
}

static mut DEXFILE: [u8; 65536] = [0; 65536];

/// Load a DEX file from the filesystem and execute its `main` with the
/// mini-Dalvik VM. Output is retrieved with `dvm::take_output`.
pub fn dvm_run_file(path: &str) -> bool {
    let Some((_, size)) = stat(path) else {
        return false;
    };
    let Some(handle) = open(path) else {
        return false;
    };
    let want = (size as usize).min(65536);
    let buf = unsafe { &mut *core::ptr::addr_of_mut!(DEXFILE) };
    let got = read_file(handle, 0, &mut buf[..want]);
    close(handle);
    if got < want {
        return false;
    }
    dvm::run(&buf[..got])
}

/// Read the UTC clock: `[epoch, year, month, day, hour, minute, second]`.
pub fn rtc() -> Option<[u64; 7]> {
    let mut fields = [0u64; 7];
    if syscall3(SYS_RTC, fields.as_mut_ptr() as u64, 0, 0) == ERROR {
        None
    } else {
        Some(fields)
    }
}

/// Map the framebuffer read/write at a fixed address and return it. The text
/// console stops drawing once a process takes over the display.
pub fn fb_map() -> Option<u64> {
    let result = syscall3(SYS_FB_MAP, 0, 0, 0);
    if result == ERROR {
        None
    } else {
        Some(result)
    }
}

/// Map the offscreen back buffer for double-buffered rendering. Render here,
/// then call `fb_present()` to publish the finished frame without tearing.
pub fn fb_back() -> Option<u64> {
    let result = syscall3(SYS_FB_BACK, 0, 0, 0);
    if result == ERROR {
        None
    } else {
        Some(result)
    }
}

/// Publish a rectangle of the back buffer to the display (dirty-rect copy).
pub fn fb_present(x0: u64, y0: u64, x1: u64, y1: u64) {
    syscall4(SYS_FB_PRESENT, x0, y0, x1, y1);
}

/// Map the latency-instrumentation mailbox; returns its user virtual address.
pub fn instr_map() -> Option<u64> {
    let result = syscall3(SYS_INSTR_MAP, 0, 0, 0);
    if result == ERROR {
        None
    } else {
        Some(result)
    }
}

/// Read the ARM generic counter (EL0-accessible), for fine-grained timestamps.
pub fn cntpct() -> u64 {
    #[cfg(target_os = "none")]
    {
        let v: u64;
        unsafe { core::arch::asm!("mrs {}, cntpct_el0", out(reg) v, options(nomem, nostack)) };
        v
    }
    #[cfg(not(target_os = "none"))]
    {
        unsafe { kimi_host_cntpct() }
    }
}

/// Run the display-path benchmark; results are printed to the console.
pub fn gfx_bench() {
    syscall3(SYS_GFX_BENCH, 0, 0, 0);
}

/// Current program break (one past the last heap byte).
pub fn brk() -> u64 {
    syscall3(SYS_BRK, 0, 0, 0)
}

/// Grow the heap to at least `address`; returns the new break.
pub fn sbrk(address: u64) -> u64 {
    syscall3(SYS_BRK, address, 0, 0)
}

/// Framebuffer geometry `(width, height, stride, format)`, if a display exists.
pub fn fb_info() -> Option<(u64, u64, u64, u64)> {
    let mut info = [0u64; 4];
    if syscall3(SYS_FB_INFO, info.as_mut_ptr() as u64, 0, 0) == ERROR {
        return None;
    }
    Some((info[0], info[1], info[2], info[3]))
}

/// Fill a rectangle with an `0x00RRGGBB` colour. Returns false without a display.
pub fn fb_fill(x: u64, y: u64, width: u64, height: u64, color: u32) -> bool {
    syscall5(SYS_FB_FILL, x, y, width, height, color as u64) == 0
}

/// Free physical frames (debug aid).
pub fn free_frames() -> u64 {
    syscall3(SYS_STATS, 0, 0, 0)
}

/// Print an unsigned decimal number.
pub fn print_u64(mut value: u64) {
    let mut buffer = [0u8; 20];
    let mut index = buffer.len();
    loop {
        index -= 1;
        buffer[index] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    print_bytes(&buffer[index..]);
}

pub fn print_i64(value: i64) {
    if value < 0 {
        print("-");
    }
    print_u64(value.unsigned_abs());
}

pub fn exit(status: u64) -> ! {
    syscall3(SYS_EXIT, status, 0, 0);
    #[cfg(target_os = "none")]
    loop {
        unsafe { asm!("wfe") };
    }
    #[cfg(not(target_os = "none"))]
    loop {
        // The host dispatcher normally terminates this thread inside SYS_EXIT;
        // if it returns, park instead of executing the bare-metal `wfe`.
        syscall3(SYS_SLEEP, 1000, 0, 0);
    }
}

/// A minimal global allocator backed by `brk`.
///
/// Freed blocks are kept in a singly linked first-fit list; new memory is
/// carved from a bump pointer that grows the program break a page at a time.
/// Blocks are 16-byte aligned (larger alignments are served from the bump
/// path), which is enough for `alloc` collections and typical user code.
///
/// Processes are single-threaded and the kernel never preempts inside this
/// crate's bookkeeping, so no lock is needed: the free list is private to the
/// process's address space.
pub struct Heap;

const HEADER: usize = 16;
const ALIGN: usize = 16;

#[repr(C)]
struct Block {
    size: usize,
    next: *mut Block,
}

static mut FREE: *mut Block = core::ptr::null_mut();
static NEXT: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
static END: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

fn align_up(value: usize, align: usize) -> Option<usize> {
    value.checked_add(align - 1).map(|sum| sum & !(align - 1))
}

impl Heap {
    unsafe fn take_free(&self, size: usize) -> Option<*mut u8> {
        let mut previous: *mut Block = core::ptr::null_mut();
        let mut current = FREE;
        while !current.is_null() {
            let block = &mut *current;
            if block.size >= size + HEADER {
                if previous.is_null() {
                    FREE = block.next;
                } else {
                    (*previous).next = block.next;
                }
                let used = align_up(size + HEADER, ALIGN).unwrap_or(block.size);
                if block.size >= used + HEADER + ALIGN {
                    let rest = (current as usize + used) as *mut Block;
                    (*rest).size = block.size - used;
                    (*rest).next = FREE;
                    FREE = rest;
                    (*current).size = used;
                }
                return Some((current as usize + HEADER) as *mut u8);
            }
            previous = current;
            current = block.next;
        }
        None
    }

    unsafe fn bump(&self, size: usize, align: usize) -> Option<*mut u8> {
        let mut start = NEXT.load(core::sync::atomic::Ordering::Relaxed);
        if start == 0 {
            start = brk() as usize;
        }
        let payload = align_up(start + HEADER, align)?;
        let wanted = payload.checked_add(size)?;
        let mut end = END.load(core::sync::atomic::Ordering::Relaxed);
        if wanted > end {
            let rounded = align_up(wanted, 4096)?;
            let grown = sbrk(rounded as u64) as usize;
            if grown < wanted {
                return None;
            }
            end = grown;
            END.store(end, core::sync::atomic::Ordering::Relaxed);
        }
        NEXT.store(wanted, core::sync::atomic::Ordering::Relaxed);
        let block = (payload - HEADER) as *mut Block;
        (*block).size = size + HEADER;
        (*block).next = core::ptr::null_mut();
        Some(payload as *mut u8)
    }
}

unsafe impl core::alloc::GlobalAlloc for Heap {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        let size = layout.size().max(1);
        let align = layout.align().max(ALIGN);
        if align > ALIGN {
            return self.bump(size, align).unwrap_or(core::ptr::null_mut());
        }
        match self.take_free(size) {
            Some(pointer) => pointer,
            None => self.bump(size, align).unwrap_or(core::ptr::null_mut()),
        }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, _layout: core::alloc::Layout) {
        if pointer.is_null() {
            return;
        }
        let block = (pointer as usize - HEADER) as *mut Block;
        (*block).next = FREE;
        FREE = block;
    }
}
