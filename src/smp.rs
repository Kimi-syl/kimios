//! Secondary CPU bring-up through PSCI.
//!
//! QEMU's `virt` machine advertises PSCI over the HVC conduit in its device
//! tree (`/psci/method = "hvc"`); the boot CPU runs at EL1 and QEMU handles the
//! HVC directly. Each secondary starts at `_secondary_start` with the MMU and caches
//! off, its own stack, and IRQs masked; it enables the MMU, runs a bounded
//! compute workload, publishes the result and parks in `wfe`.
//!
//! This phase proves the cores and atomics work. It does **not** yet schedule
//! EL0 processes on more than one CPU: the scheduler, GIC redistributors and
//! per-CPU state still assume a single boot CPU.

use crate::println;
use core::arch::{asm, global_asm};
use core::sync::atomic::{AtomicU64, Ordering};

pub const MAX_CPUS: usize = 4;
pub const STACK_SIZE: usize = 16 * 1024;

const PSCI_VERSION: u64 = 0x8400_0000;
const PSCI_CPU_ON: u64 = 0xc400_0003;

const WORK: u64 = 300_000;

extern "C" {
    fn _secondary_start();
}

#[repr(align(16))]
#[allow(dead_code)]
pub struct Stack([u8; STACK_SIZE]);

#[no_mangle]
pub static mut SECONDARY_STACKS: [Stack; MAX_CPUS] = [const { Stack([0; STACK_SIZE]) }; MAX_CPUS];

static ONLINE: AtomicU64 = AtomicU64::new(0);
static DONE: AtomicU64 = AtomicU64::new(0);
static MPIDRS: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static RESULTS: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

global_asm!(
    r#"
    .section .text
    .global _secondary_start
_secondary_start:
    // x0 = context id (logical CPU index)
    mov     x9, #16384
    madd    x10, x0, x9, xzr
    ldr     x11, =SECONDARY_STACKS
    add     x10, x11, x10
    add     x10, x10, x9
    mov     sp, x10

    // FP/SIMD must be enabled on every core.
    mrs     x1, cpacr_el1
    orr     x1, x1, #(3 << 20)
    msr     cpacr_el1, x1
    isb

    // Give this core a vector table before running Rust.
    ldr     x1, =__vector_table
    msr     vbar_el1, x1
    isb

    bl      secondary_entry

1:  wfe
    b       1b
"#
);

fn mpidr() -> u64 {
    let value: u64;
    unsafe { asm!("mrs {}, mpidr_el1", out(reg) value, options(nomem, nostack)) };
    value
}

unsafe fn psci(fid: u64, arg0: u64, arg1: u64, arg2: u64) -> i64 {
    let result: u64;
    asm!(
        "hvc #0",
        inlateout("x0") fid => result,
        inlateout("x1") arg0 => _,
        inlateout("x2") arg1 => _,
        inlateout("x3") arg2 => _,
        lateout("x4") _, lateout("x5") _, lateout("x6") _, lateout("x7") _,
        lateout("x8") _, lateout("x9") _, lateout("x10") _, lateout("x11") _,
        lateout("x12") _, lateout("x13") _, lateout("x14") _, lateout("x15") _,
        lateout("x16") _, lateout("x17") _,
        options(nostack)
    );
    result as i64
}

/// Entry point for a secondary core after PSCI `CPU_ON`.
#[no_mangle]
pub extern "C" fn secondary_entry(context: u64) -> ! {
    crate::mmu::enable_secondary();

    let index = context as usize;
    if index < MAX_CPUS {
        MPIDRS[index].store(mpidr(), Ordering::Relaxed);
    }
    ONLINE.fetch_add(1, Ordering::Release);

    let mut sum = 0u64;
    for value in 0..WORK {
        sum = sum.wrapping_add(value ^ context);
    }
    if index < MAX_CPUS {
        RESULTS[index].store(sum, Ordering::Relaxed);
    }
    DONE.fetch_add(1, Ordering::Release);

    loop {
        unsafe { asm!("wfe") };
    }
}

fn expected_sum(index: u64) -> u64 {
    let mut sum = 0u64;
    for value in 0..WORK {
        sum = sum.wrapping_add(value ^ index);
    }
    sum
}

fn wait_for(counter: &AtomicU64, target: u64) -> bool {
    let mut spins = 0u64;
    while counter.load(Ordering::Acquire) < target {
        spins += 1;
        if spins > 500_000_000 {
            return false;
        }
        core::hint::spin_loop();
    }
    true
}

/// Bring up every secondary CPU QEMU exposes and run the parallel self-test.
pub fn init() {
    let version = unsafe { psci(PSCI_VERSION, 0, 0, 0) };
    println!(
        "PSCI        : {}.{} (HVC conduit)",
        (version >> 16) & 0xffff,
        version & 0xffff
    );

    let entry = _secondary_start as *const () as usize as u64;
    let mut started = 1u64;

    for index in 1..MAX_CPUS {
        let result = unsafe { psci(PSCI_CPU_ON, index as u64, entry, index as u64) };
        if result == 0 {
            started += 1;
            println!("CPU{}        : CPU_ON accepted (MPIDR target {})", index, index);
        } else if result == -4 {
            started += 1;
            println!("CPU{}        : already online", index);
        } else {
            println!("CPU{}        : not present (PSCI {})", index, result);
        }
    }

    if !wait_for(&ONLINE, started - 1) {
        println!("SMP         : timed out waiting for secondaries");
        return;
    }

    if !wait_for(&DONE, started - 1) {
        println!("SMP         : timed out waiting for workloads");
        return;
    }

    let mut correct = 0u64;
    for index in 1..MAX_CPUS {
        let sum = RESULTS[index].load(Ordering::Relaxed);
        if sum != 0 {
            if sum == expected_sum(index as u64) {
                correct += 1;
            }
            println!(
                "CPU{}        : MPIDR {:#x}, checksum {:#x} ({})",
                index,
                MPIDRS[index].load(Ordering::Relaxed),
                sum,
                if sum == expected_sum(index as u64) {
                    "ok"
                } else {
                    "MISMATCH"
                }
            );
        }
    }

    println!(
        "SMP         : {} CPU(s) online, {}/{} checksums ok ({} iterations each)",
        started,
        correct,
        started - 1,
        WORK
    );
}
