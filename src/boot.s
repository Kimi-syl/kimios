// Minimal aarch64 entry point for QEMU virt (-kernel loads this ELF).
// QEMU normally enters at EL1 with x0 = DTB physical address. With
// virtualization=on (e.g. UTM) QEMU enters at EL2 and x0 may be 0; we drop to
// EL1 below and fall back to a RAM scan for the DTB.

.section .text.boot, "ax"
.global _start

_start:
    // Only CPU 0 continues; secondaries park in WFE.
    mrs     x1, mpidr_el1
    and     x1, x1, #0xff
    cbz     x1, 1f
2:
    wfe
    b       2b

1:
    // Set up the boot stack from the linker script.
    ldr     x1, =__stack_top
    mov     sp, x1

    // x20 records whether we dropped from EL2 (persisted to BOOT_EL2 after BSS
    // is zeroed). SMP PSCI self-test is skipped on that path.
    mov     x20, #0

    // If entered at EL2 (virtualization=on), drop to EL1 so the EL1 system
    // registers and vector table used below take effect. x0 (DTB) survives eret.
    mrs     x1, CurrentEL
    lsr     x1, x1, #2
    cmp     x1, #2
    b.ne    6f
    mov     x20, #1
    mov     x1, sp
    msr     sp_el1, x1
    adr     x1, 6f
    msr     elr_el2, x1
    mov     x1, #0x3c5            // SPSR: EL1h, DAIF masked
    msr     spsr_el2, x1
    mrs     x1, hcr_el2
    orr     x1, x1, #(1 << 31)    // HCR_EL2.RW: EL1 executes AArch64
    msr     hcr_el2, x1
    isb
    eret
6:
    // Enable FP/SIMD (CPACR_EL1.FPEN = 0b11 for EL1 and EL0).
    // After reset FPEN is 0, so any compiler-generated NEON instruction
    // traps with an Undefined Instruction exception.
    mrs     x1, cpacr_el1
    orr     x1, x1, #(3 << 20)
    msr     cpacr_el1, x1
    isb

    // Zero the .bss section (it is not part of the loaded image).
    ldr     x1, =__bss_start
    ldr     x2, =__bss_end
3:
    cmp     x1, x2
    b.hs    4f
    str     xzr, [x1], #8
    b       3b

4:
    // Persist the EL2-boot flag (after BSS zeroing so it is not cleared).
    ldr     x1, =BOOT_EL2
    str     x20, [x1]

    // x0 (DTB pointer) is still intact for kmain's first argument.
    bl      kmain

    // kmain is `-> !`, so this is unreachable; park forever just in case.
5:
    wfe
    b       5b
