MEMORY
{
    /* NOTE 1 K = 1 KiBi = 1024 bytes */
    FLASH : ORIGIN = 0x00000000, LENGTH = 128K
    APP   : ORIGIN = 0x00020000, LENGTH = 1024K - 128K
    RAM   : ORIGIN = 0x20000000, LENGTH = 256K
}

SECTIONS
{
    .app (NOLOAD) : ALIGN(4)
    {
        KEEP(*(.app .app.*));
        . = ALIGN(4);
    } > APP
}
