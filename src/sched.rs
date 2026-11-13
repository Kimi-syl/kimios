//! Round-robin kernel thread scheduler.
//!
//! Threads are kernel-only for now (EL1). A thread is a stack plus a saved
//! callee-saved register context. Scheduling is driven by the timer IRQ:
//! `irq_handler` calls `on_timer_tick`, which switches to the next Ready
//! thread.
//!
//! Scheduler state is only touched with interrupts masked (the IRQ path is
//! already masked, and spawn/init run before IRQs are enabled), so a plain
//! static is sufficient; no lock is taken in interrupt context.

use core::alloc::Layout;
use core::arch::asm;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::allocator;
use crate::gic;

pub const MAX_THREADS: usize = 8;
const STACK_SIZE: usize = 64 * 1024;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Context {
    pub x19: u64,
    pub x20: u64,
    pub x21: u64,
    pub x22: u64,
    pub x23: u64,
    pub x24: u64,
    pub x25: u64,
    pub x26: u64,
    pub x27: u64,
    pub x28: u64,
    pub x29: u64,
    pub x30: u64,
    pub sp: u64,
    pub daif: u64,
}

impl Context {
    const EMPTY: Self = Self {
        x19: 0,
        x20: 0,
        x21: 0,
        x22: 0,
        x23: 0,
        x24: 0,
        x25: 0,
        x26: 0,
        x27: 0,
        x28: 0,
        x29: 0,
        x30: 0,
        sp: 0,
        daif: 0,
    };
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Free,
    Ready,
    Running,
}

#[derive(Clone, Copy)]
struct Thread {
    context: Context,
    #[allow(dead_code)]
    stack_base: u64,
    state: State,
}

const EMPTY_THREAD: Thread = Thread {
    context: Context::EMPTY,
    stack_base: 0,
    state: State::Free,
};

static mut THREADS: [Thread; MAX_THREADS] = [EMPTY_THREAD; MAX_THREADS];
static mut CURRENT: usize = 0;
static READY: AtomicBool = AtomicBool::new(false);

extern "C" {
    fn context_switch(previous: *mut Context, next: *const Context);
    fn thread_entry();
}

pub fn is_ready() -> bool {
    READY.load(Ordering::Acquire)
}

#[allow(dead_code)]
pub fn current() -> usize {
    unsafe { CURRENT }
}

/// Register the boot context as kernel thread 0 (the idle thread).
/// Must be called with interrupts masked.
#[allow(static_mut_refs)]
pub fn init() {
    unsafe {
        for thread in THREADS.iter_mut() {
            *thread = EMPTY_THREAD;
        }
        THREADS[0].state = State::Running;
        CURRENT = 0;
    }
    READY.store(true, Ordering::Release);
}

/// Spawn a kernel thread. Must be called with interrupts masked.
#[allow(static_mut_refs)]
pub fn spawn(entry: extern "C" fn(u64), argument: u64) -> Option<usize> {
    let slot = (1..MAX_THREADS).find(|&index| unsafe {
        THREADS[index].state == State::Free
    })?;

    let layout = Layout::from_size_align(STACK_SIZE, 16).ok()?;
    let stack = allocator::alloc(layout)?;
    let stack_top = stack.as_ptr() as u64 + STACK_SIZE as u64;

    let context = Context {
        x19: entry as usize as u64,
        x20: argument,
        x30: thread_entry as *const () as u64,
        sp: stack_top,
        daif: 0, // interrupts unmasked when the thread starts
        ..Context::EMPTY
    };

    unsafe {
        THREADS[slot] = Thread {
            context,
            stack_base: stack.as_ptr() as u64,
            state: State::Ready,
        };
    }

    Some(slot)
}

/// Move the current thread to Ready and switch to the next Ready thread.
/// Safe to call from IRQ context (interrupts are already masked).
#[allow(static_mut_refs)]
pub fn schedule() {
    unsafe {
        let current = CURRENT;

        let mut next = None;
        for offset in 1..=MAX_THREADS {
            let index = (current + offset) % MAX_THREADS;
            if THREADS[index].state == State::Ready {
                next = Some(index);
                break;
            }
        }

        let Some(next) = next else {
            return;
        };

        if THREADS[current].state == State::Running {
            THREADS[current].state = State::Ready;
        }
        THREADS[next].state = State::Running;
        CURRENT = next;

        let previous_context = core::ptr::addr_of_mut!(THREADS[current].context);
        let next_context = core::ptr::addr_of!(THREADS[next].context);
        context_switch(previous_context, next_context);
    }
}

/// Called from the timer IRQ after EOI.
pub fn on_timer_tick() {
    if is_ready() {
        schedule();
    }
}

/// Called by a thread trampoline when its entry function returns.
pub fn exit_current() -> ! {
    gic::disable_irqs();
    unsafe {
        THREADS[CURRENT].state = State::Free;
    }

    loop {
        schedule();
        unsafe { asm!("wfe") };
    }
}

#[no_mangle]
pub extern "C" fn thread_exit() -> ! {
    exit_current()
}
