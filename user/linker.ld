ENTRY(_start)

SECTIONS
{
    /* User virtual address base. The kernel maps this region with 4 KiB
       pages and EL0 permissions. */
    . = 0x60000000;

    .text : ALIGN(4096) {
        *(.text.boot)
        *(.text .text.*)
    }

    /* Page-align every section so each PT_LOAD segment occupies whole,
       non-overlapping pages. This keeps the ELF loader simple. */
    .rodata : ALIGN(4096) {
        *(.rodata .rodata.*)
    }

    .data : ALIGN(4096) {
        *(.data .data.*)
        *(.got .got.*)
    }

    .bss : ALIGN(4096) {
        *(.bss .bss.*)
        *(COMMON)
    }

    /DISCARD/ : {
        *(.comment)
        *(.eh_frame .eh_frame_hdr)
    }
}
