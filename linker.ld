ENTRY(_start)

SECTIONS
{
    /* QEMU virt: RAM starts at 0x4000_0000. 0x4008_0000 matches the
       conventional arm64 kernel load offset and is safely inside RAM. */
    . = 0x40080000;

    __kernel_start = .;

    .text : {
        KEEP(*(.text.boot))
        KEEP(*(.text.vectors))
        *(.text .text.*)
    }

    .rodata : ALIGN(8) {
        *(.rodata .rodata.*)
    }

    .data : ALIGN(8) {
        *(.data .data.*)
        *(.got .got.*)
    }

    .bss : ALIGN(8) {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        __bss_end = .;
    }

    /* 64 KiB boot stack, grows downward from __stack_top. */
    . = ALIGN(16);
    . += 0x10000;
    __stack_top = .;
    __kernel_end = .;

    /DISCARD/ : {
        *(.comment)
        *(.eh_frame .eh_frame_hdr)
    }
}
