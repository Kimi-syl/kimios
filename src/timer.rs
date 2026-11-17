//! ARM generic timer, EL1 physical timer (CNTP).

use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};

static INTERVAL: AtomicU64 = AtomicU64::new(0);

/// CNTFRQ_EL0: frequency of the system counter in Hz.
pub fn frequency() -> u64 {
    let frequency: u64;
    unsafe {
        asm!("mrs {}, cntfrq_el0", out(reg) frequency, options(nomem, nostack));
    }
    frequency
}

/// Milliseconds since the system counter started (CNTPCT_EL0).
pub fn uptime_ms() -> u64 {
    let count: u64;
    unsafe {
        asm!("mrs {}, cntpct_el0", out(reg) count, options(nomem, nostack));
    }
    let ticks = count as u128 * 1000;
    (ticks / frequency().max(1) as u128) as u64
}

/// Allow EL0 (user processes) to read the system counter (CNTPCT_EL0 /
/// CNTVCT_EL0). Needed by the GUI's latency timestamps; without it the MRS
/// traps to EL1.
pub fn enable_el0_access() {
    unsafe {
        let v: u64;
        asm!("mrs {}, cntkctl_el1", out(reg) v, options(nomem, nostack));
        // EL0PCTEN (bit 0) | EL0VCTEN (bit 1)
        asm!("msr cntkctl_el1, {}", in(reg) v | 0x3, options(nomem, nostack));
    }
}

/// Start a periodic timer. `interval` is in counter ticks.
///
/// CNTP_TVAL_EL0 is a down-counter; when it reaches zero the condition is
/// asserted and PPI 30 becomes pending until TVAL is written again.
pub fn start_periodic(interval: u64) {
    INTERVAL.store(interval, Ordering::Release);
    reload();
    unsafe {
        // ENABLE = 1, IMASK = 0.
        asm!("msr cntp_ctl_el0, {}", in(reg) 1u64, options(nomem, nostack));
    }
}

/// Reload the down-counter for the next tick.
pub fn reload() {
    let interval = INTERVAL.load(Ordering::Acquire);
    unsafe {
        asm!("msr cntp_tval_el0, {}", in(reg) interval, options(nomem, nostack));
    }
}

pub fn stop() {
    unsafe {
        asm!("msr cntp_ctl_el0, {}", in(reg) 0u64, options(nomem, nostack));
    }
}
