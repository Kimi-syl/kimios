//! PL031 real-time clock (QEMU `virt` maps it at 0x0901_0000).
//!
//! Only the data register is used: it returns whole seconds since the Unix
//! epoch. QEMU defaults to `-rtc base=utc,clock=host`, so the value tracks
//! the host clock in UTC.

const BASE: usize = 0x0901_0000;
const RTCDR: usize = 0x00;

/// Seconds since 1970-01-01T00:00:00Z, or `None` when the clock reads zero
/// (no RTC or an unset one).
pub fn epoch_seconds() -> Option<u64> {
    let seconds = unsafe { core::ptr::read_volatile((BASE + RTCDR) as *const u32) };
    (seconds != 0).then_some(seconds as u64)
}

/// Split Unix seconds into `(year, month, day, hour, minute, second)` UTC.
pub fn civil(seconds: u64) -> (u64, u64, u64, u64, u64, u64) {
    let days = (seconds / 86_400) as i64;
    let time = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    (
        year,
        month,
        day,
        time / 3_600,
        (time % 3_600) / 60,
        time % 60,
    )
}

/// Howard Hinnant's `civil_from_days` (days since 1970-01-01).
fn civil_from_days(days: i64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = year + (month <= 2) as i64;
    (year as u64, month as u64, day as u64)
}