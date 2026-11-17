//! Latency instrumentation mailbox.
//!
//! A small page of guest RAM shared with the Android frontend (which maps the
//! whole guest RAM file). The GUI writes CNTPCT timestamps into it at each
//! stage of the touch -> display path; the kernel stamps `fb_present`. Android
//! reads the mailbox out of shared memory and correlates the guest counter with
//! its own monotonic clock (via the heartbeat field) to report per-stage latency.
//!
//! Layout (u64 slots): see the `IBOX_*` constants in libkimi.

use crate::println;

/// Slot indices (must match libkimi `IBOX_*` and the Android reader).
pub const MAGIC: usize = 0;
pub const SEQ: usize = 1;
pub const CNTFRQ: usize = 2;
pub const HB: usize = 3;
pub const INPUT: usize = 4;
pub const HANDLER: usize = 5;
pub const DRAWN: usize = 6;
pub const PRESENT: usize = 7;
pub const RECT: usize = 8;
pub const IVAL: usize = 9;
/// Cumulative dirty rect since the frontend acknowledged (union of presents).
pub const ACCUM: usize = 10;
/// Frontend writes the frame counter it has blitted; guest resets ACCUM once
/// the ack catches up, so no dirty region is ever missed between polls.
pub const ACK: usize = 11;
pub const FRAME: usize = 12;

const MAGIC_VALUE: u64 = 0x4B4D_494C; // "KMIL"

#[repr(align(4096))]
struct Mailbox([u64; 16]);

#[allow(static_mut_refs)]
static mut MB: Mailbox = Mailbox([0; 16]);

fn counter() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, cntpct_el0", out(reg) v, options(nomem, nostack)) };
    v
}

fn counter_freq() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, cntfrq_el0", out(reg) v, options(nomem, nostack)) };
    v
}

/// Guest-physical address of the mailbox (kernel is identity-mapped).
pub fn addr() -> u64 {
    core::ptr::addr_of!(MB) as u64
}

#[allow(static_mut_refs)]
pub fn init() {
    unsafe {
        MB.0[MAGIC] = MAGIC_VALUE;
        MB.0[CNTFRQ] = counter_freq();
    }
    println!(
        "instr       : mailbox at {:#x} cntfrq {} Hz",
        addr(),
        counter_freq()
    );
}

fn pack(x0: usize, y0: usize, x1: usize, y1: usize) -> u64 {
    (x0 as u64 & 0xffff)
        | ((y0 as u64 & 0xffff) << 16)
        | ((x1 as u64 & 0xffff) << 32)
        | ((y1 as u64 & 0xffff) << 48)
}

fn union(a: u64, b: u64) -> u64 {
    if a == 0 {
        return b;
    }
    if b == 0 {
        return a;
    }
    let ax0 = a & 0xffff;
    let ay0 = (a >> 16) & 0xffff;
    let ax1 = (a >> 32) & 0xffff;
    let ay1 = (a >> 48) & 0xffff;
    let bx0 = b & 0xffff;
    let by0 = (b >> 16) & 0xffff;
    let bx1 = (b >> 32) & 0xffff;
    let by1 = (b >> 48) & 0xffff;
    pack(
        ax0.min(bx0) as usize,
        ay0.min(by0) as usize,
        ax1.max(bx1) as usize,
        ay1.max(by1) as usize,
    )
}

/// Stamp the `fb_present` stage, accumulate the dirty rect for the frontend,
/// and bump the frame counter (last, so a reader seeing a new count also sees
/// the matching accumulated rect).
#[allow(static_mut_refs)]
pub fn present(x0: usize, y0: usize, x1: usize, y1: usize) {
    let rect = pack(x0, y0, x1, y1);
    unsafe {
        MB.0[PRESENT] = counter();
        MB.0[RECT] = rect;
        let frame = MB.0[FRAME];
        // Once the frontend has blitted everything up to `frame`, start a fresh
        // accumulation; otherwise grow the pending region so nothing is missed.
        if MB.0[ACK] == frame {
            MB.0[ACCUM] = rect;
        } else {
            MB.0[ACCUM] = union(MB.0[ACCUM], rect);
        }
        MB.0[FRAME] = frame.wrapping_add(1);
    }
}
