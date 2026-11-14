//! IRQ dispatch and the timer tick handler.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::exceptions::ExceptionFrame;
use crate::gic;
use crate::println;
use crate::process;
use crate::sched;
use crate::timer;

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

    if intid == gic::TIMER_INTID {
        // Re-arm before EOI so the level-sensitive line deasserts.
        timer::reload();
        let count = TICKS.fetch_add(1, Ordering::AcqRel) + 1;
        timer_tick = true;
        if !sched::is_ready() && !process::user_active() {
            println!("Tick! ({})", count);
        }
    } else if intid != 1023 {
        println!("Unexpected IRQ: INTID {}", intid);
    }

    gic::end_of_interrupt(iar);

    // SAFETY: vectors.s passes SP_EL1 pointing at the saved ExceptionFrame.
    let frame = unsafe { &mut *frame };
    let from_el0 = frame.spsr & 0xf == 0;

    if timer_tick {
        if from_el0 {
            // Preempt the running user process (cooperative fallback stays in
            // place for spawn/wait/exit).
            process::preempt(frame);
        } else if !process::user_active() {
            // Kernel-thread round-robin during the Phase 7-8 demos.
            sched::on_timer_tick();
        }
    }
}
