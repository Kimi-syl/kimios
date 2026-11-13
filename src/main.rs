#![no_std]
#![no_main]

mod allocator;
mod dtb;
mod exceptions;
mod gic;
mod irq;
mod mmu;
mod pfa;
mod timer;
mod uart;

use core::alloc::Layout;
use core::arch::{asm, global_asm};
use core::panic::PanicInfo;

global_asm!(include_str!("boot.s"));
global_asm!(include_str!("vectors.s"));

/// Entry point called from `_start` in boot.s.
///
/// `dtb` is the physical address of the device tree blob that QEMU places in
/// x0 when it loads a kernel.
#[no_mangle]
pub extern "C" fn kmain(dtb: usize) -> ! {
    uart::init();

    println!();
    println!("KimiOS v0.1.0 - aarch64 bare metal");
    println!("----------------------------------");
    println!("PL011 UART online: 115200 8N1");
    println!("Device tree blob at {:#018x}", dtb);
    println!();
    println!("Hello, World from Rust #[no_std]!");

    // Phase 3: install the EL1 vector table, then deliberately trap.
    println!();
    exceptions::init();

    println!();
    println!("Triggering BRK #0x1...");
    unsafe { asm!("brk #0x1") };
    println!("Back in kmain after the breakpoint.");

    // Phase 4: identity-map RAM and turn the MMU on.
    println!();
    println!("Enabling MMU (4 KiB granule, 48-bit VA, identity map)...");
    mmu::init();
    println!("MMU enabled: SCTLR_EL1.M = {}", mmu::enabled() as u8);

    // Prove that the heap is reachable through the new mapping.
    let layout = Layout::from_size_align(256, 64).unwrap();
    let block = allocator::alloc_zeroed(layout).expect("out of heap memory");
    unsafe {
        core::ptr::write_bytes(block.as_ptr(), 0xab, layout.size());
        let readback = core::ptr::read(block.as_ptr().add(layout.size() - 1));
        println!(
            "Heap block: {:#018x}..{:#018x} (readback {:#04x})",
            block.as_ptr() as usize,
            block.as_ptr() as usize + layout.size(),
            readback
        );
    }
    println!(
        "Heap usage : {} / {} bytes",
        allocator::used(),
        allocator::capacity()
    );

    // Phase 5: device tree + physical frame allocator.
    println!();
    println!("Phase 5: device tree and physical memory");

    let boot = dtb::probe(dtb);
    if boot.memory_from_dtb {
        println!(
            "DTB         : {:#018x} ({} bytes, source: {})",
            boot.dtb_addr,
            boot.dtb_size,
            if boot.from_x0 { "x0" } else { "RAM scan" }
        );
    } else {
        println!("DTB         : not found (using fallback memory map)");
    }
    println!(
        "/memory     : {:#018x}..{:#018x} ({} MiB)",
        boot.memory.base,
        boot.memory.base + boot.memory.size,
        boot.memory.size / (1024 * 1024)
    );

    let dtb_range = if boot.dtb_addr != 0 {
        Some((boot.dtb_addr, boot.dtb_size as u64))
    } else {
        None
    };
    pfa::init(boot.memory, dtb_range);

    let (free_before, total) = pfa::stats();
    println!(
        "PFA         : {} frames ({} MiB), {} free",
        total,
        total * pfa::PAGE_SIZE / (1024 * 1024),
        free_before
    );

    let f1 = pfa::alloc_frame().expect("no free frames");
    let f2 = pfa::alloc_frame().expect("no free frames");
    let f3 = pfa::alloc_frame().expect("no free frames");
    println!("allocated   : {:#x}, {:#x}, {:#x}", f1, f2, f3);

    assert!(pfa::free_frame(f2));
    let reused = pfa::alloc_frame().expect("no free frames");
    println!(
        "freed       : {:#x}; reallocated {:#x} (reused = {})",
        f2,
        reused,
        reused == f2
    );
    println!("PFA free    : {} frames", pfa::stats().0);

    // Phase 6: GICv3 + ARM generic timer.
    println!();
    println!("Phase 6: GICv3 and timer interrupts");

    gic::init();
    let frequency = timer::frequency();
    let interval = frequency / 2; // 500 ms
    timer::start_periodic(interval);

    println!("CNTFRQ_EL0  : {} Hz", frequency);
    println!("Timer       : PPI 30, {} ticks per tick (500 ms)", interval);
    println!("Unmasking IRQs...");
    gic::enable_irqs();

    while irq::ticks() < 5 {
        unsafe { asm!("wfe") };
    }

    gic::disable_irqs();
    timer::stop();

    println!();
    println!("Received {} timer interrupts.", irq::ticks());
    println!("Phases 1-6 complete. Halting.");

    // No UART input loop yet; exit QEMU cleanly via semihosting.
    semihosting_exit(0x20026, 0);
}

/// SYS_EXIT_EXTENDED (0x20): x0 = op, x1 = pointer to [reason, status].
/// Requires: -semihosting-config enable=on,target=native
pub(crate) fn semihosting_exit(reason: u64, status: u64) -> ! {
    let block: [u64; 2] = [reason, status];
    unsafe {
        core::arch::asm!(
            "hlt #0xf000",
            in("x0") 0x20u64,
            in("x1") block.as_ptr(),
            options(noreturn)
        );
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!();
    println!("*** KERNEL PANIC ***");
    println!("{}", info);

    // ADP_Stopped_RunTimeErrorUnknown (0x20023)
    semihosting_exit(0x20023, 1);
}
