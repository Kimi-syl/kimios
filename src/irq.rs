//! IRQ dispatch and the timer tick handler.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::exceptions::ExceptionFrame;
use crate::gic;
use crate::println;
use crate::process;
use crate::sched;
use crate::timer;
use crate::uart;

pub static TICKS: AtomicU64 = AtomicU64::new(0);

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Acquire)
}

/// Called from the IRQ vector in vectors.s with interrupts already masked.
#[no_mangle]
pub extern "C" fn irq_handler(frame: *mut ExceptionFrame) {
    let iar = gic::acknowledge();
    let intid = iar & 0x00ff_ffff;
    let mut timer_tick = false;
    let mut line_ready = false;

    if intid == gic::TIMER_INTID {
        // Re-arm before EOI so the level-sensitive line deasserts.
        timer::reload();
        let count = TICKS.fetch_add(1, Ordering::AcqRel) + 1;
        timer_tick = true;
        if !sched::is_ready() && !process::user_active() {
            println!("Tick! ({})", count);
        }
        // Safety net: poll the RX FIFO in case the level interrupt was masked
        // or the trigger level was not reached (single-character input).
        line_ready = uart::rx_interrupt();

    } else if intid == gic::UART_INTID {
        line_ready = uart::rx_interrupt();
    } else if intid == crate::virtio_input::interrupt_id() {
        line_ready = crate::virtio_input::interrupt();
    } else if intid != 1023 {
        println!("Unexpected IRQ: INTID {}", intid);
    }

    if line_ready {
        process::wake_reader();
    }

    gic::end_of_interrupt(iar);

    // SAFETY: vectors.s passes SP_EL1 pointing at the saved ExceptionFrame.
    let frame = unsafe { &mut *frame };
    let from_el0 = frame.spsr & 0xf == 0;

    if timer_tick {
        // Wake expired sleepers before preemption so they can be scheduled on
        // this same tick.
        process::wake_sleepers(timer::uptime_ms());
        if from_el0 {
            // Preempt the running user process (cooperative fallback stays in
            // place for spawn/wait/exit).
            process::preempt(frame);
        } else if !process::user_active() {
            // Kernel-thread round-robin during the Phase 7-8 demos.
            sched::on_timer_tick();
        }
    } else if line_ready && from_el0 {
        // A blocked reader became runnable; switch to it promptly.
        process::preempt(frame);
    }
}
