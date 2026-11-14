//! Kernel input event queue shared by the virtio keyboard and pointer.
//!
//! The virtio-input IRQ handler is the single producer; SVC handlers are the
//! single consumer. A small lock-free ring keeps IRQ latency predictable
//! (no spinning on a mutex held by a woken process).

use core::sync::atomic::{AtomicUsize, Ordering};

pub const EV_SYN: u16 = 0;
pub const EV_KEY: u16 = 1;
pub const EV_REL: u16 = 2;
pub const EV_ABS: u16 = 3;

#[derive(Clone, Copy)]
pub struct Event {
    pub kind: u16,
    pub code: u16,
    pub value: u32,
}

const CAPACITY: usize = 128;

static mut QUEUE: [Event; CAPACITY] = [Event {
    kind: 0,
    code: 0,
    value: 0,
}; CAPACITY];
static HEAD: AtomicUsize = AtomicUsize::new(0);
static TAIL: AtomicUsize = AtomicUsize::new(0);

/// Append an event; drops the oldest when the ring is full.
pub fn push(kind: u16, code: u16, value: u32) {
    let head = HEAD.load(Ordering::Relaxed);
    let tail = TAIL.load(Ordering::Acquire);
    if head - tail >= CAPACITY {
        // Overwrite the oldest event rather than stalling an IRQ.
        TAIL.store(tail + 1, Ordering::Release);
    }
    unsafe {
        (core::ptr::addr_of_mut!(QUEUE) as *mut Event)
            .add(head % CAPACITY)
            .write(Event { kind, code, value });
    }
    HEAD.store(head + 1, Ordering::Release);
}

/// Remove the oldest event, if any.
pub fn pop() -> Option<Event> {
    let tail = TAIL.load(Ordering::Relaxed);
    let head = HEAD.load(Ordering::Acquire);
    if tail == head {
        return None;
    }
    let event = unsafe {
        (core::ptr::addr_of!(QUEUE) as *const Event)
            .add(tail % CAPACITY)
            .read()
    };
    TAIL.store(tail + 1, Ordering::Release);
    Some(event)
}

pub fn pending() -> usize {
    HEAD.load(Ordering::Acquire)
        .saturating_sub(TAIL.load(Ordering::Acquire))
}

/// Pack into `[kind:16][code:16][value:32]` for a syscall return value.
pub fn pack(event: Event) -> u64 {
    ((event.kind as u64) << 48) | ((event.code as u64) << 32) | event.value as u64
}

/// Idle in the kernel until the IRQ handler queues an event. Only used when
/// there is no other runnable process.
pub fn wait_for_event() {
    crate::gic::enable_irqs();
    while pending() == 0 {
        unsafe { core::arch::asm!("wfi") };
    }
    crate::gic::disable_irqs();
}