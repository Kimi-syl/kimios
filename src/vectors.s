// AArch64 exception vector table.
//
// VBAR_EL1 must point to a 2 KiB aligned table containing 16 entries,
// each 0x80 bytes apart:
//
//   offset 0x000: Current EL, SP0  - Synchronous
//   offset 0x080: Current EL, SP0  - IRQ
//   offset 0x100: Current EL, SP0  - FIQ
//   offset 0x180: Current EL, SP0  - SError
//   offset 0x200: Current EL, SPx  - Synchronous   <- KimiOS runs here
//   offset 0x280: Current EL, SPx  - IRQ
//   offset 0x300: Current EL, SPx  - FIQ
//   offset 0x380: Current EL, SPx  - SError
//   offset 0x400: Lower EL, AArch64 - Synchronous
//   ... etc.
//
// We route everything through one common save/restore path for now.

.section .text.vectors, "ax"
.balign 2048
.global __vector_table
__vector_table:

.macro vector_entry
    .balign 0x80
    b       exc_common
.endm

    vector_entry    // 0x000
    vector_entry    // 0x080
    vector_entry    // 0x100
    vector_entry    // 0x180
    vector_entry    // 0x200  Current EL SPx, Synchronous
    vector_entry    // 0x280
    vector_entry    // 0x300
    vector_entry    // 0x380
    vector_entry    // 0x400  Lower EL AArch64, Synchronous
    vector_entry    // 0x480
    vector_entry    // 0x500
    vector_entry    // 0x580
    vector_entry    // 0x600  Lower EL AArch32, Synchronous
    vector_entry    // 0x680
    vector_entry    // 0x700
    vector_entry    // 0x780

// ExceptionFrame layout (must match src/exceptions.rs):
//   [  0..248)  x0..x30
//   [248]       ELR_EL1
//   [256]       SPSR_EL1
//   total 272 bytes
.align 4
exc_common:
    sub     sp, sp, #272

    stp     x0,  x1,  [sp, #0]
    stp     x2,  x3,  [sp, #16]
    stp     x4,  x5,  [sp, #32]
    stp     x6,  x7,  [sp, #48]
    stp     x8,  x9,  [sp, #64]
    stp     x10, x11, [sp, #80]
    stp     x12, x13, [sp, #96]
    stp     x14, x15, [sp, #112]
    stp     x16, x17, [sp, #128]
    stp     x18, x19, [sp, #144]
    stp     x20, x21, [sp, #160]
    stp     x22, x23, [sp, #176]
    stp     x24, x25, [sp, #192]
    stp     x26, x27, [sp, #208]
    stp     x28, x29, [sp, #224]

    mrs     x0, elr_el1
    mrs     x1, spsr_el1
    stp     x30, x0,  [sp, #240]   // x30 @240, ELR @248
    str     x1,       [sp, #256]   // SPSR

    mov     x0, sp                 // &mut ExceptionFrame
    bl      exception_handler

    // Allow the handler to resume at a different PC (e.g. past a BRK).
    ldr     x1, [sp, #248]
    msr     elr_el1, x1
    ldr     x1, [sp, #256]
    msr     spsr_el1, x1

    ldp     x0,  x1,  [sp, #0]
    ldp     x2,  x3,  [sp, #16]
    ldp     x4,  x5,  [sp, #32]
    ldp     x6,  x7,  [sp, #48]
    ldp     x8,  x9,  [sp, #64]
    ldp     x10, x11, [sp, #80]
    ldp     x12, x13, [sp, #96]
    ldp     x14, x15, [sp, #112]
    ldp     x16, x17, [sp, #128]
    ldp     x18, x19, [sp, #144]
    ldp     x20, x21, [sp, #160]
    ldp     x22, x23, [sp, #176]
    ldp     x24, x25, [sp, #192]
    ldp     x26, x27, [sp, #208]
    ldp     x28, x29, [sp, #224]
    ldp     x30, xzr, [sp, #240]

    add     sp, sp, #272
    eret
