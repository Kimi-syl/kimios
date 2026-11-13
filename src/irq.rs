//! IRQ dispatch and the timer tick handler.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::gic;
use crate::println;
use crate::sched;
use crate::timer;

pub static TICKS: AtomicU64 = AtomicU64::new(0);

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Acquire)
}

/// Called from the IRQ vector in vectors.s with interrupts already masked.
#[no_mangle]
pub extern "C" fn irq_handler() {
    let iar = gic::acknowledge();
    let intid = iar & 0x00ff_ffff;

    if intid == gic::TIMER_INTID {
        // Re-arm before EOI so the level-sensitive line deasserts.
        timer::reload();
        let count = TICKS.fetch_add(1, Ordering::AcqRel) + 1;
        if !sched::is_ready() {
            println!("Tick! ({})", count);
        }
    } else if intid != 1023 {
        println!("Unexpected IRQ: INTID {}", intid);
    }

    gic::end_of_interrupt(iar);

    // Round-robin: may switch to another thread's context.
    sched::on_timer_tick();
}
