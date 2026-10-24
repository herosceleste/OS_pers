ENTRY(_start)

PHDRS
{
    text PT_LOAD FLAGS(5);
    rodata PT_LOAD FLAGS(4);
    data PT_LOAD FLAGS(6);
}

SECTIONS
{
    . = 0x00200000;

    .text ALIGN(0x1000) : {
        *(.text._start)
        *(.text .text.*)
    } :text

    .rodata ALIGN(0x1000) : {
        *(.rodata .rodata.*)
    } :rodata

    .data ALIGN(0x1000) : {
        *(.data .data.*)
        /* Keep the GOT before .bss: an orphan .got placed after .bss forces
           the linker to store the whole zero-filled .bss in KERNEL.ELF. */
        *(.got .got.*)
    } :data

    .bss ALIGN(0x1000) (NOLOAD) : {
        *(.bss .bss.*)
        *(COMMON)
    } :data

    /DISCARD/ : {
        *(.eh_frame*)
        *(.comment*)
    }
}
