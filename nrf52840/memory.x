MEMORY
{
    /* NOTE 1 K = 1 KiBi = 1024 bytes */
    FLASH   : ORIGIN = 0x00010000, LENGTH = 1024K - 64K
    SCRATCH : ORIGIN = 0x20000000, LENGTH = 1K
    RAM     : ORIGIN = 0x20000400, LENGTH = 256K - 1K
}

SECTIONS
{
    .scratch (NOLOAD) : ALIGN(4)
    {
        KEEP(*(.scratch .scratch.*));
    } > SCRATCH
}
