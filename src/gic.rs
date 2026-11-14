//! GICv3 initialization for QEMU's `virt` machine.
//!
//! Addresses on `virt`:
//!   GICD (distributor)     0x0800_0000
//!   GICR (redistributors)  0x080A_0000, 0x20000 bytes per CPU
//!
//! The CPU interface is accessed through system registers (ICC_*), which is
//! why ICC_SRE_EL1.SRE must be set before ICC_PMR_EL1 / ICC_IGRPEN1_EL1.
//!
//! Interrupt 30 is the EL1 physical timer PPI (CNTP). It is a per-CPU
//! interrupt, so it is enabled in the redistributor's SGI/PPI frame.

use core::arch::asm;

const GICD_BASE: usize = 0x0800_0000;
const GICR_BASE: usize = 0x080a_0000;
const GICR_SGI_OFFSET: usize = 0x1_0000;

const GICD_CTLR: usize = 0x0000;
const GICD_CTLR_ARE_NS: u32 = 1 << 4;
const GICD_CTLR_ENABLE_G1A: u32 = 1 << 1;
const GICD_CTLR_ENABLE_G1: u32 = 1 << 0;
const GICD_CTLR_RWP: u32 = 1 << 31;

// Distributor register blocks for shared peripheral interrupts (SPI).
const GICD_IGROUPR: usize = 0x0080;
const GICD_ISENABLER: usize = 0x0100;
const GICD_IPRIORITYR: usize = 0x0400;
const GICD_ICFGR: usize = 0x0c00;
const GICD_IROUTER: usize = 0x6000;

const GICR_CTLR: usize = 0x0000;
const GICR_WAKER: usize = 0x0014;
const GICR_WAKER_PROCESSOR_SLEEP: u32 = 1 << 1;
const GICR_WAKER_CHILDREN_ASLEEP: u32 = 1 << 2;

const GICR_IGROUPR0: usize = GICR_SGI_OFFSET + 0x0080;
const GICR_ISENABLER0: usize = GICR_SGI_OFFSET + 0x0100;
const GICR_IPRIORITYR: usize = GICR_SGI_OFFSET + 0x0400;
const GICR_ICFGR1: usize = GICR_SGI_OFFSET + 0x0c04;

pub const TIMER_INTID: u32 = 30;
/// PL011 UART0 on QEMU's `virt` is SPI 1 (INTID 32 + 1).
pub const UART_INTID: u32 = 33;
const SPURIOUS_INTID: u32 = 1023;

unsafe fn read32(address: usize) -> u32 {
    core::ptr::read_volatile(address as *const u32)
}

unsafe fn write32(address: usize, value: u32) {
    core::ptr::write_volatile(address as *mut u32, value);
}

unsafe fn write8(address: usize, value: u8) {
    core::ptr::write_volatile(address as *mut u8, value);
}

unsafe fn write64(address: usize, value: u64) {
    core::ptr::write_volatile(address as *mut u64, value);
}

fn dsb() {
    unsafe { asm!("dsb sy") }
}

fn wait_for_rwp() {
    let mut spins = 0u32;
    unsafe {
        while read32(GICD_BASE + GICD_CTLR) & GICD_CTLR_RWP != 0 && spins < 1_000_000 {
            core::hint::spin_loop();
            spins += 1;
        }
    }
}

pub fn init() {
    unsafe {
        // 1. Enable the system-register CPU interface and let all priorities
        //    through. ICC_IGRPEN1_EL1 enables non-secure Group 1.
        asm!("msr icc_sre_el1, {}", in(reg) 1u64);
        asm!("isb");
        asm!("msr icc_pmr_el1, {}", in(reg) 0xffu64);
        asm!("msr icc_bpr1_el1, {}", in(reg) 0u64);
        asm!("msr icc_igrpen1_el1, {}", in(reg) 1u64);
        asm!("isb");

        // 2. Distributor: affinity routing + Group 1 (bits 4, 1, 0).
        write32(GICD_BASE + GICD_CTLR, 0);
        wait_for_rwp();
        write32(
            GICD_BASE + GICD_CTLR,
            GICD_CTLR_ARE_NS | GICD_CTLR_ENABLE_G1A | GICD_CTLR_ENABLE_G1,
        );
        wait_for_rwp();

        // 3. Wake this CPU's redistributor (clear ProcessorSleep, then wait
        //    for ChildrenAsleep to clear).
        write32(GICR_BASE + GICR_CTLR, 0);
        let mut waker = read32(GICR_BASE + GICR_WAKER);
        waker &= !GICR_WAKER_PROCESSOR_SLEEP;
        write32(GICR_BASE + GICR_WAKER, waker);
        dsb();

        let mut spins = 0u32;
        while read32(GICR_BASE + GICR_WAKER) & GICR_WAKER_CHILDREN_ASLEEP != 0
            && spins < 1_000_000
        {
            core::hint::spin_loop();
            spins += 1;
        }

        // 4. Configure PPI 30: Group 1, highest priority, level-sensitive.
        write32(GICR_BASE + GICR_IGROUPR0, 1 << TIMER_INTID);
        write8(
            GICR_BASE + GICR_IPRIORITYR + TIMER_INTID as usize,
            0x00,
        );

        // GICR_ICFGR1 holds two bits per PPI (16..31); 0b00 = level.
        let config = read32(GICR_BASE + GICR_ICFGR1) & !(0b11 << 28);
        write32(GICR_BASE + GICR_ICFGR1, config);

        write32(GICR_BASE + GICR_ISENABLER0, 1 << TIMER_INTID);
        dsb();
    }
}

/// Enable a shared peripheral interrupt (INTID >= 32) as a level-sensitive
/// Group 1 interrupt routed to CPU 0.
pub fn enable_spi(intid: u32, priority: u8) {
    if intid < 32 {
        return;
    }
    unsafe {
        let word = (intid / 32) as usize;
        let bit = intid % 32;

        let group = GICD_BASE + GICD_IGROUPR + word * 4;
        write32(group, read32(group) | (1 << bit));

        write8(GICD_BASE + GICD_IPRIORITYR + intid as usize, priority);

        // Two configuration bits per interrupt; 0b00 = level-sensitive.
        let config = GICD_BASE + GICD_ICFGR + (intid as usize / 16) * 4;
        let shift = (intid as usize % 16) * 2;
        write32(config, read32(config) & !(0b11 << shift));

        // Route to the CPU 0 affinity (affinity 0, no 1-of-N).
        write64(GICD_BASE + GICD_IROUTER + intid as usize * 8, 0);

        let enable = GICD_BASE + GICD_ISENABLER + word * 4;
        write32(enable, 1 << bit);
        dsb();
    }
}

/// Acknowledge the highest-priority pending Group 1 interrupt.
/// Returns the raw IAR value (INTID in bits 0..23, CPUID in 24..26).
pub fn acknowledge() -> u32 {
    let iar: u64;
    unsafe {
        asm!("mrs {}, icc_iar1_el1", out(reg) iar, options(nomem, nostack));
    }
    iar as u32
}

pub fn end_of_interrupt(iar: u32) {
    if iar & 0x00ff_ffff == SPURIOUS_INTID {
        return;
    }
    unsafe {
        asm!("msr icc_eoir1_el1, {}", in(reg) iar as u64, options(nomem, nostack));
    }
}

pub fn enable_irqs() {
    unsafe { asm!("msr daifclr, #2", options(nomem, nostack)) };
}

pub fn disable_irqs() {
    unsafe { asm!("msr daifset, #2", options(nomem, nostack)) };
}
