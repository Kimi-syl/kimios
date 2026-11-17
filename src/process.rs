//! Process model: PIDs, per-process address spaces, FDs and lifecycle.
//!
//! Processes are cooperatively scheduled at syscall boundaries. A process runs
//! at EL0 until it blocks in `wait`, exits, or replaces itself with `exec`.
//! Switching rewrites the exception frame that `vectors.s` will restore and
//! installs the next process's TTBR0, so `eret` resumes it directly.
//!
//! `fork` is deliberately not implemented; `spawn(path)` creates a process
//! from an ELF or PE image and `wait(pid)` returns its exit status.

use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::exceptions::ExceptionFrame;
use crate::ramfs::Vfs;
use crate::{diskfs, elf, mmu, pe, println, ramfs};

pub const MAX_PROCESSES: usize = 16;
pub const MAX_FDS: usize = 8;
/// User heap window: `brk` grows page by page from here.
pub const USER_HEAP_START: u64 = 0x6080_0000;
pub const USER_HEAP_LIMIT: u64 = 0x60C0_0000;
/// Anonymous `mmap` region for Linux processes (grows upward).
pub const LINUX_MMAP_START: u64 = 0x5000_0000;
const LINUX_MMAP_LIMIT: u64 = 0x6000_0000;
const LINUX_BRK_LIMIT: u64 = 0x2000_0000;
/// Fixed user mapping for the framebuffer (read/write, no execute).
pub const USER_FB_VA: u64 = 0x6600_0000;
pub const USER_INSTR_VA: u64 = 0x6700_0000;
pub const SYS_ERROR: u64 = u64::MAX;

const INIT_PID: u64 = 1;
static NEXT_PID: AtomicU64 = AtomicU64::new(INIT_PID);
static USER_ACTIVE: AtomicBool = AtomicBool::new(false);
/// PID currently blocked in `read`, or 0. The console IRQ wakes it.
static READER_PID: AtomicU64 = AtomicU64::new(0);
/// PID currently blocked in `input_wait`, or 0. The input IRQ wakes it.
static INPUT_PID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Free,
    Ready,
    Running,
    Blocked,
    Zombie,
}

pub const FD_RAM: u8 = 0;
pub const FD_DISK: u8 = 1;
pub const FD_DIR: u8 = 2;
pub const FD_CONSOLE: u8 = 3;

#[derive(Clone, Copy)]
pub struct Descriptor {
    pub used: bool,
    pub kind: u8,
    pub index: usize,
    /// Current file position (Linux read/write).
    pub pos: u64,
}

const EMPTY_DESCRIPTOR: Descriptor = Descriptor {
    used: false,
    kind: FD_RAM,
    index: 0,
    pos: 0,
};

pub struct Process {
    state: State,
    pub pid: u64,
    pub parent: u64,
    pub exit_status: u64,
    wait_target: u64,
    /// True when blocked in `read` rather than in `wait`.
    read_wait: bool,
    /// True when blocked in `input_wait`.
    input_wait: bool,
    /// Deadline in milliseconds for a blocked sleeper (0 = not sleeping).
    sleep_at: u64,
    /// Set by the timer IRQ so a woken sleeper's retried SVC returns 0.
    sleep_done: bool,
    /// Current program break (one past the last byte of the user heap).
    brk: u64,
    /// True when this process uses the Linux syscall ABI (static Linux ELF).
    pub linux: bool,
    /// Linux program break (grows from the end of the data segment).
    linux_brk: u64,
    /// Next anonymous `mmap` address for a Linux process.
    linux_mmap: u64,
    /// Linux current working directory (NUL-free, e.g. "/disk").
    cwd: [u8; 64],
    cwd_len: usize,
    /// Resolved path for each directory descriptor (FD_DIR).
    dir_path: [[u8; 64]; MAX_FDS],
    dir_len: [usize; MAX_FDS],
    /// True when the framebuffer has been mapped into this process.
    fb_mapped: bool,
    space: Option<mmu::AddressSpace>,
    frame: ExceptionFrame,
    sp_el0: u64,
    fds: [Descriptor; MAX_FDS],
}

const EMPTY_PROCESS: Process = Process {
    state: State::Free,
    pid: 0,
    parent: 0,
    exit_status: 0,
    wait_target: 0,
    read_wait: false,
    input_wait: false,
    sleep_at: 0,
    sleep_done: false,
    brk: 0,
    linux: false,
    linux_brk: 0,
    linux_mmap: 0,
    cwd: [0; 64],
    cwd_len: 0,
    dir_path: [[0; 64]; MAX_FDS],
    dir_len: [0; MAX_FDS],
    fb_mapped: false,
    space: None,
    frame: ExceptionFrame::EMPTY,
    sp_el0: 0,
    fds: [EMPTY_DESCRIPTOR; MAX_FDS],
};

static mut PROCESSES: [Process; MAX_PROCESSES] = [EMPTY_PROCESS; MAX_PROCESSES];
static mut CURRENT: usize = usize::MAX;

pub enum WaitOutcome {
    Status(u64),
    Switched,
}

#[allow(static_mut_refs)]
fn current_slot() -> Option<usize> {
    let slot = unsafe { CURRENT };
    if slot < MAX_PROCESSES && unsafe { PROCESSES[slot].state } == State::Running {
        Some(slot)
    } else {
        None
    }
}

pub fn current_pid() -> u64 {
    current_slot()
        .map(|slot| unsafe { PROCESSES[slot].pid })
        .unwrap_or(0)
}

#[allow(static_mut_refs)]
fn slot_of_pid(pid: u64) -> Option<usize> {
    if pid == 0 {
        return None;
    }
    (0..MAX_PROCESSES).find(|&slot| {
        let process = unsafe { &PROCESSES[slot] };
        process.state != State::Free && process.pid == pid
    })
}

#[allow(static_mut_refs)]
fn free_slot() -> Option<usize> {
    (0..MAX_PROCESSES).find(|&slot| unsafe { PROCESSES[slot].state } == State::Free)
}

#[allow(static_mut_refs)]
fn is_child(process: &Process, parent: u64, target: u64) -> bool {
    process.state != State::Free
        && process.parent == parent
        && (target == 0 || process.pid == target)
}

#[allow(static_mut_refs)]
fn find_child(parent: u64, target: u64, state: State) -> Option<usize> {
    (0..MAX_PROCESSES).find(|&slot| {
        let process = unsafe { &PROCESSES[slot] };
        is_child(process, parent, target) && process.state == state
    })
}

#[allow(static_mut_refs)]
fn has_child(parent: u64, target: u64) -> bool {
    (0..MAX_PROCESSES).any(|slot| is_child(unsafe { &PROCESSES[slot] }, parent, target))
}

#[allow(static_mut_refs)]
fn pick_ready() -> Option<usize> {
    (0..MAX_PROCESSES).find(|&slot| unsafe { PROCESSES[slot].state } == State::Ready)
}

#[allow(static_mut_refs)]
fn pick_ready_after(current: usize) -> Option<usize> {
    (1..MAX_PROCESSES).find_map(|offset| {
        let slot = (current + offset) % MAX_PROCESSES;
        (unsafe { PROCESSES[slot].state } == State::Ready).then_some(slot)
    })
}

pub fn user_active() -> bool {
    USER_ACTIVE.load(Ordering::Acquire)
}

#[allow(static_mut_refs)]
fn reap(slot: usize) {
    unsafe {
        if PROCESSES[slot].fb_mapped {
            crate::fb::set_console(true);
        }
        if let Some(space) = PROCESSES[slot].space.take() {
            space.destroy();
        }
        PROCESSES[slot] = EMPTY_PROCESS;
    }
}

#[allow(static_mut_refs)]
fn release_dead(slot: usize) {
    unsafe {
        if PROCESSES[slot].fb_mapped {
            crate::fb::set_console(true);
        }
        if let Some(space) = PROCESSES[slot].space.take() {
            space.destroy();
        }
        PROCESSES[slot] = EMPTY_PROCESS;
    }
}

fn load_image(
    space: &mut mmu::AddressSpace,
    image: &[u8],
    argv: &[&str],
) -> Result<(u64, u64, u64, bool), &'static str> {
    if pe::is_pe(image) {
        let loaded = pe::load(image, space)?;
        Ok((loaded.entry, loaded.stack_top, USER_HEAP_START, false))
    } else if elf::is_linux(image) {
        let loaded = elf::load_linux(image, space, argv)?;
        Ok((loaded.entry, loaded.sp, loaded.brk, true))
    } else {
        let loaded = elf::load(image, space)?;
        Ok((loaded.entry, loaded.stack_top, USER_HEAP_START, false))
    }
}

/// Load an executable image from RamFS (`/name`) or KimiFS (`/disk/name`).
fn load_path(
    space: &mut mmu::AddressSpace,
    path: &str,
    argv: &[&str],
) -> Result<(u64, u64, u64, bool), &'static str> {
    if let Some(name) = path.strip_prefix("/disk/") {
        let inode = diskfs::lookup(name).ok_or("executable not found")?;
        let size = diskfs::inode_size(inode).ok_or("cannot stat executable")? as usize;
        if size == 0 || size > 3 * 1024 * 1024 {
            return Err("executable too large");
        }
        let layout = core::alloc::Layout::from_size_align(size, 16).map_err(|_| "bad size")?;
        let buf = crate::allocator::alloc(layout).ok_or("out of memory for executable")?;
        let slice = unsafe { core::slice::from_raw_parts_mut(buf.as_ptr(), size) };
        let n = diskfs::read(inode, 0, slice);
        let result = load_image(space, &slice[..n], argv);
        crate::allocator::dealloc(buf);
        result
    } else {
        let file = ramfs::index_of(path)
            .and_then(ramfs::file_at)
            .ok_or("executable not found")?;
        load_image(space, file.data, argv)
    }
}

fn basename(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[i + 1..],
        None => path,
    }
}

/// Create a process from an embedded file. Returns its process slot.
fn create(parent: u64, path: &str) -> Result<usize, &'static str> {
    create_args(parent, path, &[])
}

/// Create a process with extra command-line arguments (Linux argv[1..]).
#[allow(static_mut_refs)]
fn create_args(parent: u64, path: &str, extra: &[&str]) -> Result<usize, &'static str> {
    let slot = free_slot().ok_or("process table full")?;
    let mut space = mmu::AddressSpace::new().ok_or("out of memory for page tables")?;

    let mut argv: [&str; 17] = [""; 17];
    argv[0] = basename(path);
    let n = extra.len().min(16);
    for i in 0..n {
        argv[i + 1] = extra[i];
    }
    let (entry, sp, brk, linux) = load_path(&mut space, path, &argv[..1 + n])?;

    let pid = NEXT_PID.fetch_add(1, Ordering::AcqRel);
    unsafe {
        PROCESSES[slot] = Process {
            state: State::Ready,
            pid,
            parent,
            exit_status: 0,
            wait_target: 0,
            read_wait: false,
            input_wait: false,
            sleep_at: 0,
            sleep_done: false,
            brk: USER_HEAP_START,
            linux,
            linux_brk: brk,
            linux_mmap: LINUX_MMAP_START,
            cwd: {
                let mut c = [0u8; 64];
                if linux {
                    c[..5].copy_from_slice(b"/disk");
                }
                c
            },
            cwd_len: if linux { 5 } else { 0 },
            dir_path: [[0; 64]; MAX_FDS],
            dir_len: [0; MAX_FDS],
            fb_mapped: false,
            space: Some(space),
            frame: ExceptionFrame {
                x: [0; 31],
                elr: entry,
                spsr: 0, // EL0t, IRQs enabled (timer preemption)
            },
            sp_el0: sp,
            fds: [EMPTY_DESCRIPTOR; MAX_FDS],
        };
        if linux {
            // Reserve stdin/stdout/stderr so file/dir fds start at 3.
            for i in 0..3 {
                PROCESSES[slot].fds[i] = Descriptor {
                    used: true,
                    kind: FD_CONSOLE,
                    index: 0,
                    pos: 0,
                };
            }
        }
    }

    println!(
        "[kernel] created pid {} (parent {}) at {:#x}",
        pid, parent, entry
    );
    Ok(slot)
}

/// Boot the first user process (`/init`) and drop to EL0.
#[allow(static_mut_refs)]
pub fn start_init() -> ! {
    println!();
    println!("Phase 13: per-process address spaces, spawn/exec/wait");
    let slot = create(0, "/init").expect("failed to start /init");

    USER_ACTIVE.store(true, Ordering::Release);
    crate::timer::start_periodic(crate::timer::frequency() / 20);
    println!("[kernel] EL0 timer preemption enabled (50 ms quantum)");

    // Console input: route the PL011 RX interrupt and set its FIFO trigger.
    crate::gic::enable_spi(crate::gic::UART_INTID, 0x20);
    crate::uart::enable_rx_interrupt();
    println!(
        "[kernel] console input: UART RX IRQ (INTID {})",
        crate::gic::UART_INTID
    );

    unsafe {
        PROCESSES[slot].state = State::Running;
        CURRENT = slot;

        let process = &PROCESSES[slot];
        process
            .space
            .as_ref()
            .expect("init has no address space")
            .activate();

        asm!(
            "msr sp_el0, {sp}",
            "msr elr_el1, {entry}",
            "msr spsr_el1, {spsr}",
            "eret",
            sp = in(reg) process.sp_el0,
            entry = in(reg) process.frame.elr,
            spsr = in(reg) process.frame.spsr,
            options(noreturn),
        );
    }
}

/// Timer-IRQ hook: rotate to another runnable process when one exists.
///
/// Called with interrupts masked from the IRQ vector. The frame on the kernel
/// stack is the interrupted EL0 context; `switch_to` saves it and rewrites the
/// frame with the next process's state.
pub fn preempt(frame: &mut ExceptionFrame) {
    if !user_active() {
        return;
    }

    let Some(current) = current_slot() else {
        return;
    };
    let Some(next) = pick_ready_after(current) else {
        return;
    };

    switch_to(frame, next);
}

/// Save the current process and resume `next`, rewriting the exception frame.
#[allow(static_mut_refs)]
fn switch_to(frame: &mut ExceptionFrame, next: usize) {
    let previous = unsafe { CURRENT };

    unsafe {
        if previous < MAX_PROCESSES {
            let sp: u64;
            asm!("mrs {}, sp_el0", out(reg) sp, options(nomem, nostack));
            PROCESSES[previous].sp_el0 = sp;
            PROCESSES[previous].frame = *frame;
            if PROCESSES[previous].state == State::Running {
                PROCESSES[previous].state = State::Ready;
            }
        }

        let next_process = &PROCESSES[next];
        *frame = next_process.frame;
        PROCESSES[next].state = State::Running;
        CURRENT = next;

        next_process
            .space
            .as_ref()
            .expect("process has no address space")
            .activate();

        asm!(
            "msr sp_el0, {sp}",
            sp = in(reg) next_process.sp_el0,
            options(nomem, nostack),
        );
    }
}

/// Whether some process is already blocked waiting for console input.
pub fn reader_busy() -> bool {
    READER_PID.load(Ordering::Acquire) != 0
}

/// Block the current process until console input arrives.
///
/// The calling SVC is rewound by one instruction, so when the console IRQ
/// marks this process ready the resumed EL0 context re-executes the `svc` and
/// retries the read. Returns `false` when it could not switch (no other
/// runnable process); the caller then waits in the kernel.
#[allow(static_mut_refs)]
pub fn block_on_read(frame: &mut ExceptionFrame) -> bool {
    if !user_active() {
        return false;
    }
    let Some(slot) = current_slot() else {
        return false;
    };
    let Some(next) = pick_ready() else {
        return false;
    };

    let pid = unsafe { PROCESSES[slot].pid };
    READER_PID.store(pid, Ordering::Release);
    unsafe {
        PROCESSES[slot].state = State::Blocked;
        PROCESSES[slot].read_wait = true;
    }
    frame.elr = frame.elr.wrapping_sub(4);
    switch_to(frame, next);
    true
}

/// Block the current process until an input event is queued.
///
/// Like `block_on_read`, the SVC is rewound so the retried call pops the event.
#[allow(static_mut_refs)]
pub fn block_on_input(frame: &mut ExceptionFrame) -> bool {
    if !user_active() {
        return false;
    }
    let Some(slot) = current_slot() else {
        return false;
    };
    let Some(next) = pick_ready() else {
        return false;
    };

    let pid = unsafe { PROCESSES[slot].pid };
    INPUT_PID.store(pid, Ordering::Release);
    unsafe {
        PROCESSES[slot].state = State::Blocked;
        PROCESSES[slot].input_wait = true;
    }
    frame.elr = frame.elr.wrapping_sub(4);
    switch_to(frame, next);
    true
}

/// Input-IRQ hook: make the process blocked in `input_wait` runnable.
#[allow(static_mut_refs)]
pub fn wake_input() {
    let pid = INPUT_PID.swap(0, Ordering::AcqRel);
    if pid == 0 {
        return;
    }
    if let Some(slot) = slot_of_pid(pid) {
        unsafe {
            if PROCESSES[slot].state == State::Blocked {
                PROCESSES[slot].state = State::Ready;
                PROCESSES[slot].input_wait = false;
            }
        }
    }
}

/// Console-IRQ hook: move the process blocked in `read` back to the ready
/// queue. Runs with interrupts masked.
#[allow(static_mut_refs)]
pub fn wake_reader() {
    let pid = READER_PID.swap(0, Ordering::AcqRel);
    if pid == 0 {
        return;
    }
    if let Some(slot) = slot_of_pid(pid) {
        unsafe {
            if PROCESSES[slot].state == State::Blocked {
                PROCESSES[slot].state = State::Ready;
                PROCESSES[slot].read_wait = false;
            }
        }
    }
}

pub enum SleepOutcome {
    Done(u64),
    Switched,
}

/// Timer-IRQ hook: make expired sleepers runnable. Runs with IRQs masked.
#[allow(static_mut_refs)]
pub fn wake_sleepers(now_ms: u64) {
    for slot in 0..MAX_PROCESSES {
        unsafe {
            let process = &mut PROCESSES[slot];
            if process.state == State::Blocked
                && process.sleep_at != 0
                && now_ms >= process.sleep_at
            {
                process.sleep_at = 0;
                process.sleep_done = true;
                process.state = State::Ready;
            }
        }
    }
}

/// sleep(ms): block until `ms` milliseconds have elapsed.
///
/// With another runnable process the SVC is rewound and the CPU switches; the
/// retried syscall sees `sleep_done` and returns 0. If this is the only
/// process, the kernel idles with `wfi` until the timer IRQ marks it ready.
#[allow(static_mut_refs)]
pub fn sys_sleep(frame: &mut ExceptionFrame, ms: u64) -> SleepOutcome {
    if !user_active() {
        return SleepOutcome::Done(SYS_ERROR);
    }
    let Some(slot) = current_slot() else {
        return SleepOutcome::Done(SYS_ERROR);
    };

    unsafe {
        if PROCESSES[slot].sleep_done {
            PROCESSES[slot].sleep_done = false;
            return SleepOutcome::Done(0);
        }
    }

    if ms == 0 {
        return SleepOutcome::Done(0);
    }

    let deadline = crate::timer::uptime_ms().saturating_add(ms);
    unsafe {
        PROCESSES[slot].sleep_at = deadline;
        PROCESSES[slot].state = State::Blocked;
    }

    match pick_ready() {
        Some(next) => {
            frame.elr = frame.elr.wrapping_sub(4);
            switch_to(frame, next);
            SleepOutcome::Switched
        }
        None => {
            // No other process: idle in the kernel until the timer IRQ wakes
            // this one (it is re-marked Running on the way out).
            loop {
                if crate::timer::uptime_ms() >= deadline {
                    break;
                }
                crate::gic::enable_irqs();
                unsafe { asm!("wfi") };
                crate::gic::disable_irqs();
            }
            unsafe {
                PROCESSES[slot].sleep_at = 0;
                PROCESSES[slot].sleep_done = false;
                PROCESSES[slot].state = State::Running;
            }
            SleepOutcome::Done(0)
        }
    }
}

/// Block the current process until `target` (0 = any child) exits.
#[allow(static_mut_refs)]
pub fn sys_wait(frame: &mut ExceptionFrame, target: u64) -> WaitOutcome {
    let Some(slot) = current_slot() else {
        return WaitOutcome::Status(SYS_ERROR);
    };
    let me = unsafe { PROCESSES[slot].pid };

    if let Some(child) = find_child(me, target, State::Zombie) {
        let status = unsafe { PROCESSES[child].exit_status };
        reap(child);
        return WaitOutcome::Status(status);
    }

    if !has_child(me, target) {
        return WaitOutcome::Status(SYS_ERROR);
    }

    unsafe {
        PROCESSES[slot].state = State::Blocked;
        PROCESSES[slot].wait_target = target;
        PROCESSES[slot].read_wait = false;
    }

    match pick_ready() {
        Some(next) => switch_to(frame, next),
        None => {
            // The child may be alive but blocked (e.g. a GUI waiting for
            // input). Do not fail the wait: idle until an interrupt makes
            // some process runnable, then switch to it.
            crate::gic::enable_irqs();
            let next = loop {
                if let Some(ready) = pick_ready() {
                    break ready;
                }
                unsafe { asm!("wfi") };
            };
            crate::gic::disable_irqs();
            switch_to(frame, next);
        }
    }
    WaitOutcome::Switched
}

/// Terminate the current process and resume another one.
#[allow(static_mut_refs)]
pub fn sys_exit(frame: &mut ExceptionFrame, status: u64) {
    let Some(slot) = current_slot() else {
        println!("exit: no current process");
        crate::semihosting_exit(0x20026, 1);
    };

    let me = unsafe { PROCESSES[slot].pid };
    let parent = unsafe { PROCESSES[slot].parent };

    if READER_PID.load(Ordering::Acquire) == me {
        READER_PID.store(0, Ordering::Release);
    }
    if INPUT_PID.load(Ordering::Acquire) == me {
        INPUT_PID.store(0, Ordering::Release);
    }

    unsafe {
        PROCESSES[slot].state = State::Zombie;
        PROCESSES[slot].exit_status = status;
    }
    println!("[kernel] pid {} exited with status {}", me, status);

    // Orphaned children are adopted by init.
    unsafe {
        for index in 0..MAX_PROCESSES {
            if PROCESSES[index].state != State::Free
                && PROCESSES[index].parent == me
            {
                PROCESSES[index].parent = INIT_PID;
            }
        }
    }

    // Hand the status to a blocked parent (and free the slot eagerly, because
    // the parent is resumed directly from the saved syscall frame).
    let mut eager_reap = false;
    if let Some(parent_slot) = slot_of_pid(parent) {
        unsafe {
            // A parent blocked in `read` or `sleep` is not waiting for this
            // child.
            let process = &mut PROCESSES[parent_slot];
            if process.state == State::Blocked
                && !process.read_wait
                && process.sleep_at == 0
                && (process.wait_target == 0 || process.wait_target == me)
            {
                process.frame.x[0] = status;
                process.state = State::Ready;
                process.wait_target = 0;
                eager_reap = true;
            }
        }
    }

    // Resume a ready process. If none is ready but other processes are still
    // blocked (e.g. the shell waiting for console input), idle until one is
    // woken by an interrupt instead of shutting down.
    let mut next = pick_ready();
    if next.is_none() {
        if !has_live_process_except(slot) {
            println!("[kernel] no runnable process; shutting down");
            crate::semihosting_exit(0x20026, 0);
        }
        crate::gic::enable_irqs();
        loop {
            if let Some(ready) = pick_ready() {
                next = Some(ready);
                break;
            }
            unsafe { asm!("wfi") };
        }
        crate::gic::disable_irqs();
    }

    // Reap before switching: `switch_to` resumes the parent via `eret` and
    // never returns, so cleanup placed after it would be dead code.
    if eager_reap {
        release_dead(slot);
    }

    if let Some(next) = next {
        switch_to(frame, next);
    }
}

/// Whether any process besides `except` is still alive (not Free or Zombie).
#[allow(static_mut_refs)]
fn has_live_process_except(except: usize) -> bool {
    (0..MAX_PROCESSES).any(|slot| {
        slot != except && {
            let state = unsafe { PROCESSES[slot].state };
            state != State::Free && state != State::Zombie
        }
    })
}

/// Replace the current image, keeping PID and file descriptors.
#[allow(static_mut_refs)]
pub fn sys_exec(frame: &mut ExceptionFrame, path: &str) -> u64 {
    let Some(slot) = current_slot() else {
        return SYS_ERROR;
    };

    let mut space = match mmu::AddressSpace::new() {
        Some(space) => space,
        None => return SYS_ERROR,
    };
    let (entry, sp, brk, linux) = match load_path(&mut space, path, &[basename(path)]) {
        Ok(image) => image,
        Err(error) => {
            println!("exec: {}", error);
            return SYS_ERROR;
        }
    };

    unsafe {
        let old = PROCESSES[slot].space.replace(space);
        PROCESSES[slot].sp_el0 = sp;
        PROCESSES[slot].brk = USER_HEAP_START;
        PROCESSES[slot].linux = linux;
        PROCESSES[slot].linux_brk = brk;
        PROCESSES[slot].linux_mmap = LINUX_MMAP_START;
        if PROCESSES[slot].fb_mapped {
            crate::fb::set_console(true);
            PROCESSES[slot].fb_mapped = false;
        }
        frame.elr = entry;

        PROCESSES[slot]
            .space
            .as_ref()
            .expect("exec lost address space")
            .activate();

        if let Some(old) = old {
            old.destroy();
        }

        asm!(
            "msr sp_el0, {sp}",
            sp = in(reg) sp,
            options(nomem, nostack),
        );
    }

    println!("[kernel] pid {} exec {} -> {:#x}", current_pid(), path, entry);
    0
}

/// Create a child process from `path`.
pub fn sys_spawn(path: &str) -> u64 {
    let parent = current_pid();
    match create(parent, path) {
        Ok(slot) => unsafe { PROCESSES[slot].pid },
        Err(error) => {
            println!("spawn: {}", error);
            SYS_ERROR
        }
    }
}

/// Create a child process with command-line arguments (Linux argv[1..]).
pub fn sys_spawn_args(path: &str, args: &[&str]) -> u64 {
    let parent = current_pid();
    match create_args(parent, path, args) {
        Ok(slot) => unsafe { PROCESSES[slot].pid },
        Err(error) => {
            println!("spawn: {}", error);
            SYS_ERROR
        }
    }
}

/// Current working directory of the calling Linux process.
pub fn linux_cwd() -> &'static str {
    let Some(slot) = current_slot() else {
        return "/disk";
    };
    let p = unsafe { &PROCESSES[slot] };
    core::str::from_utf8(&p.cwd[..p.cwd_len]).unwrap_or("/disk")
}

/// Set the calling process's working directory.
#[allow(static_mut_refs)]
pub fn set_cwd(path: &str) -> bool {
    let Some(slot) = current_slot() else {
        return false;
    };
    let p = unsafe { &mut PROCESSES[slot] };
    let n = path.len().min(63);
    p.cwd[..n].copy_from_slice(path.as_bytes());
    p.cwd_len = n;
    true
}

/// Open a directory descriptor, remembering its resolved path for getdents64.
#[allow(static_mut_refs)]
pub fn open_dir(path: &str) -> Option<u64> {
    let slot = current_slot()?;
    unsafe {
        for i in 0..MAX_FDS {
            if !PROCESSES[slot].fds[i].used {
                PROCESSES[slot].fds[i] = Descriptor {
                    used: true,
                    kind: FD_DIR,
                    index: 0,
                    pos: 0,
                };
                let n = path.len().min(63);
                PROCESSES[slot].dir_path[i][..n].copy_from_slice(path.as_bytes());
                PROCESSES[slot].dir_len[i] = n;
                return Some(i as u64);
            }
        }
    }
    None
}

/// Resolved path of a directory descriptor.
pub fn dir_path(fd: u64) -> Option<&'static str> {
    let slot = current_slot()?;
    let i = fd as usize;
    if i >= MAX_FDS {
        return None;
    }
    let p = unsafe { &PROCESSES[slot] };
    if !p.fds[i].used || p.fds[i].kind != FD_DIR {
        return None;
    }
    core::str::from_utf8(&p.dir_path[i][..p.dir_len[i]]).ok()
}

/// Current position of a descriptor (file offset or directory entry index).
pub fn fd_pos(fd: u64) -> Option<u64> {
    let slot = current_slot()?;
    let i = fd as usize;
    if i >= MAX_FDS {
        return None;
    }
    let p = unsafe { &PROCESSES[slot] };
    p.fds[i].used.then_some(p.fds[i].pos)
}

/// Set a descriptor's position (lseek / getdents64 cursor).
#[allow(static_mut_refs)]
pub fn fd_set_pos(fd: u64, pos: u64) -> bool {
    let Some(slot) = current_slot() else {
        return false;
    };
    let i = fd as usize;
    if i >= MAX_FDS {
        return false;
    }
    let p = unsafe { &mut PROCESSES[slot] };
    if !p.fds[i].used {
        return false;
    }
    p.fds[i].pos = pos;
    true
}

/// Duplicate a descriptor onto a new (or specific) fd index.
#[allow(static_mut_refs)]
pub fn fd_dup(oldfd: u64, newfd: Option<u64>) -> Option<u64> {
    let slot = current_slot()?;
    let old = oldfd as usize;
    if old >= MAX_FDS {
        return None;
    }
    unsafe {
        let desc = PROCESSES[slot].fds[old];
        if !desc.used {
            return None;
        }
        let target = match newfd {
            Some(n) => {
                let n = n as usize;
                if n >= MAX_FDS {
                    return None;
                }
                n
            }
            None => (0..MAX_FDS).find(|&i| !PROCESSES[slot].fds[i].used)?,
        };
        PROCESSES[slot].fds[target] = desc;
        if desc.kind == FD_DIR {
            PROCESSES[slot].dir_path[target] = PROCESSES[slot].dir_path[old];
            PROCESSES[slot].dir_len[target] = PROCESSES[slot].dir_len[old];
        }
        Some(target as u64)
    }
}

pub fn sys_getpid() -> u64 {
    current_pid()
}

fn state_code(state: State) -> u64 {
    match state {
        State::Free => 0,
        State::Ready => 1,
        State::Running => 2,
        State::Blocked => 3,
        State::Zombie => 4,
    }
}

/// Fill `out` with `[pid, parent, state, exit_status]` records for every live
/// slot. Returns the number of records written.
#[allow(static_mut_refs)]
pub fn snapshot(out: &mut [u64]) -> usize {
    let mut count = 0;
    for slot in 0..MAX_PROCESSES {
        if (count + 1) * 4 > out.len() {
            break;
        }
        let process = unsafe { &PROCESSES[slot] };
        if process.state == State::Free {
            continue;
        }
        out[count * 4] = process.pid;
        out[count * 4 + 1] = process.parent;
        out[count * 4 + 2] = state_code(process.state);
        out[count * 4 + 3] = process.exit_status;
        count += 1;
    }
    count
}

/// kill(pid): mark another process a zombie. Returns 0 on success.
#[allow(static_mut_refs)]
pub fn sys_kill(pid: u64) -> u64 {
    let Some(slot) = current_slot() else {
        return SYS_ERROR;
    };
    let me = unsafe { PROCESSES[slot].pid };
    if pid == 0 || pid == me {
        return SYS_ERROR;
    }
    let Some(target) = slot_of_pid(pid) else {
        return SYS_ERROR;
    };
    let state = unsafe { PROCESSES[target].state };
    if state == State::Zombie {
        return SYS_ERROR;
    }

    let status = 137; // 128 + SIGKILL
    if READER_PID.load(Ordering::Acquire) == pid {
        READER_PID.store(0, Ordering::Release);
    }
    unsafe {
        PROCESSES[target].state = State::Zombie;
        PROCESSES[target].exit_status = status;
    }

    // Orphaned children are adopted by init.
    unsafe {
        for index in 0..MAX_PROCESSES {
            if PROCESSES[index].state != State::Free
                && PROCESSES[index].parent == pid
            {
                PROCESSES[index].parent = INIT_PID;
            }
        }
    }

    // If the parent is blocked waiting for exactly this child, hand it the
    // status now and reap the slot.
    let parent = unsafe { PROCESSES[target].parent };
    let mut reap_target = false;
    if let Some(parent_slot) = slot_of_pid(parent) {
        unsafe {
            let process = &mut PROCESSES[parent_slot];
            if process.state == State::Blocked
                && !process.read_wait
                && process.sleep_at == 0
                && (process.wait_target == 0 || process.wait_target == pid)
            {
                process.frame.x[0] = status;
                process.state = State::Ready;
                process.wait_target = 0;
                reap_target = true;
            }
        }
    }
    if reap_target {
        reap(target);
    }

    0
}

/// brk(request): query (0) or grow the user heap. Returns the break, which is
/// unchanged when the request is invalid or memory is exhausted.
#[allow(static_mut_refs)]
pub fn sys_brk(request: u64) -> u64 {
    let Some(slot) = current_slot() else {
        return 0;
    };
    let process = unsafe { &mut PROCESSES[slot] };
    let current = process.brk;
    if request == 0 {
        return current;
    }
    if request <= current || request > USER_HEAP_LIMIT {
        return current;
    }

    let new_end = (request + 0xfff) & !0xfff;
    if new_end > USER_HEAP_LIMIT {
        return current;
    }
    let pages = ((new_end - current) / 4096) as usize;

    let Some(space) = process.space.as_mut() else {
        return current;
    };
    // One spare frame for a possible new L3 table, plus a guard.
    if space.spare_frames() < pages + 2 {
        return current;
    }

    for page in 0..pages {
        let address = current + (page as u64) * 4096;
        let Some(frame) = space.alloc_frame() else {
            return current;
        };
        unsafe {
            core::ptr::write_bytes(frame as *mut u8, 0, 4096);
        }
        space.map_page(address, frame, false);
    }

    process.brk = new_end;
    new_end
}

/// Whether the current process uses the Linux syscall ABI.
pub fn current_is_linux() -> bool {
    current_slot()
        .map(|slot| unsafe { PROCESSES[slot].linux })
        .unwrap_or(false)
}

/// Linux brk(2): grow the heap from the end of the data segment.
#[allow(static_mut_refs)]
pub fn linux_brk(request: u64) -> u64 {
    let Some(slot) = current_slot() else {
        return 0;
    };
    let process = unsafe { &mut PROCESSES[slot] };
    let current = process.linux_brk;
    if request == 0 || request <= current {
        return current;
    }
    if request > LINUX_BRK_LIMIT {
        return current;
    }
    let new_end = (request + 0xfff) & !0xfff;
    let pages = ((new_end - current) / 4096) as usize;
    let Some(space) = process.space.as_mut() else {
        return current;
    };
    if space.spare_frames() < pages + 2 {
        return current;
    }
    for page in 0..pages {
        let address = current + (page as u64) * 4096;
        let Some(frame) = space.alloc_frame() else {
            return current;
        };
        unsafe {
            core::ptr::write_bytes(frame as *mut u8, 0, 4096);
        }
        space.map_page(address, frame, false);
    }
    process.linux_brk = new_end;
    new_end
}

/// Linux anonymous mmap(2): hand out pages from the mmap region.
#[allow(static_mut_refs)]
pub fn linux_mmap(length: u64, executable: bool) -> Option<u64> {
    let slot = current_slot()?;
    let process = unsafe { &mut PROCESSES[slot] };
    let length = (length + 0xfff) & !0xfff;
    let address = process.linux_mmap;
    if length == 0 || address + length > LINUX_MMAP_LIMIT {
        return None;
    }
    let pages = (length / 4096) as usize;
    let space = process.space.as_mut()?;
    if space.spare_frames() < pages + 2 {
        return None;
    }
    for page in 0..pages {
        let a = address + (page as u64) * 4096;
        let frame = space.alloc_frame()?;
        unsafe {
            core::ptr::write_bytes(frame as *mut u8, 0, 4096);
        }
        space.map_page(a, frame, executable);
    }
    process.linux_mmap = address + length;
    Some(address)
}

/// Positional read for a Linux file descriptor; advances the file position.
#[allow(static_mut_refs)]
pub fn fd_read(fd: u64, buffer: &mut [u8]) -> Option<usize> {
    let slot = current_slot()?;
    let index = fd as usize;
    if index >= MAX_FDS {
        return None;
    }
    let descriptor = unsafe { PROCESSES[slot].fds[index] };
    if !descriptor.used {
        return None;
    }
    let n = if descriptor.kind == FD_DISK {
        diskfs::read(descriptor.index, descriptor.pos, buffer)
    } else {
        let file = ramfs::file_at(descriptor.index)?;
        ramfs::RAMFS.read(file, descriptor.pos as usize, buffer)
    };
    unsafe {
        PROCESSES[slot].fds[index].pos += n as u64;
    }
    Some(n)
}

/// Positional write for a Linux file descriptor; advances the file position.
#[allow(static_mut_refs)]
pub fn fd_write(fd: u64, bytes: &[u8]) -> Option<usize> {
    let slot = current_slot()?;
    let index = fd as usize;
    if index >= MAX_FDS {
        return None;
    }
    let descriptor = unsafe { PROCESSES[slot].fds[index] };
    if !descriptor.used || descriptor.kind != FD_DISK {
        return None;
    }
    let n = diskfs::write(descriptor.index, descriptor.pos, bytes).ok()?;
    unsafe {
        PROCESSES[slot].fds[index].pos += n as u64;
    }
    Some(n)
}

/// Whether `[pointer, pointer + length)` is fully mapped in the calling
/// process. Syscalls use this before touching user memory so a bad pointer
/// kills the caller instead of faulting the kernel.
#[allow(static_mut_refs)]
pub fn user_range_ok(pointer: u64, length: u64) -> bool {
    if length == 0 {
        return true;
    }
    let Some(end) = pointer.checked_add(length) else {
        return false;
    };
    let Some(slot) = current_slot() else {
        return false;
    };
    let Some(space) = (unsafe { PROCESSES[slot].space.as_ref() }) else {
        return false;
    };
    let mut page = pointer & !0xfff;
    while page < end {
        if space.translate(page).is_none() {
            return false;
        }
        page += 0x1000;
    }
    true
}

/// Map the framebuffer into the current process and hand drawing to user
/// space by silencing the text console. Returns the fixed user address.
pub fn map_framebuffer() -> Option<u64> {
    let (physical, stride, height) = crate::fb::mapping()?;
    map_fb_at(physical, stride, height)
}

/// Map the offscreen back buffer for double-buffered GUI rendering.
pub fn map_backbuffer() -> Option<u64> {
    let (physical, stride, height) = crate::fb::back_mapping()?;
    map_fb_at(physical, stride, height)
}

/// Map the latency-instrumentation mailbox (one page) for user-space writes.
#[allow(static_mut_refs)]
pub fn map_instr() -> Option<u64> {
    let slot = current_slot()?;
    let space = unsafe { PROCESSES[slot].space.as_mut() }?;
    space.map_shared_page(USER_INSTR_VA, crate::instr::addr(), false);
    Some(USER_INSTR_VA)
}

#[allow(static_mut_refs)]
fn map_fb_at(physical: u64, stride: usize, height: usize) -> Option<u64> {
    let slot = current_slot()?;
    if unsafe { PROCESSES[slot].fb_mapped } {
        return Some(USER_FB_VA);
    }
    let bytes = (stride * height) as u64;
    let pages = (bytes + 0xfff) / 0x1000;

    let space = unsafe { PROCESSES[slot].space.as_mut() }?;
    for page in 0..pages {
        space.map_shared_page(
            USER_FB_VA + page * 0x1000,
            physical + page * 0x1000,
            false,
        );
    }
    unsafe {
        PROCESSES[slot].fb_mapped = true;
    }
    crate::fb::set_console(false);
    Some(USER_FB_VA)
}

/// Allocate a descriptor for `index` of `kind` in the current process.
#[allow(static_mut_refs)]
pub fn open_fd(kind: u8, index: usize) -> Option<u64> {
    let slot = current_slot()?;
    unsafe {
        for slot_index in 0..MAX_FDS {
            if !PROCESSES[slot].fds[slot_index].used {
                PROCESSES[slot].fds[slot_index] = Descriptor {
                    used: true,
                    kind,
                    index,
                    pos: 0,
                };
                return Some(slot_index as u64);
            }
        }
    }
    None
}

/// Returns `(kind, index)` for an open descriptor.
#[allow(static_mut_refs)]
pub fn fd_target(fd: u64) -> Option<(u8, usize)> {
    let slot = current_slot()?;
    let index = fd as usize;
    if index >= MAX_FDS {
        return None;
    }
    let descriptor = unsafe { PROCESSES[slot].fds[index] };
    if descriptor.used {
        Some((descriptor.kind, descriptor.index))
    } else {
        None
    }
}

#[allow(static_mut_refs)]
pub fn close_fd(fd: u64) -> bool {
    let Some(slot) = current_slot() else {
        return false;
    };
    let index = fd as usize;
    if index >= MAX_FDS {
        return false;
    }
    unsafe {
        if !PROCESSES[slot].fds[index].used {
            return false;
        }
        PROCESSES[slot].fds[index] = EMPTY_DESCRIPTOR;
    }
    true
}
