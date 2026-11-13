// Minimal aarch64 entry point for QEMU virt (-kernel loads this ELF).
// QEMU enters this code at EL1 with:
//   x0 = physical address of the device tree blob (DTB)

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
    // x0 (DTB pointer) is still intact for kmain's first argument.
    bl      kmain

    // kmain is `-> !`, so this is unreachable; park forever just in case.
5:
    wfe
    b       5b
