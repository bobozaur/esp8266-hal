/* ESP8266: 96 KiB of DRAM and 32 KiB of instruction RAM with cache enabled.
 * The ROM boot stack is inherited by xtensa-lx-rt. Reserve its upper 8 KiB
 * by default; applications may override STACK_SIZE at link time. This budget
 * does not move or reclaim the active stack.
 */
ENTRY(ESP8266Reset)

PROVIDE(STACK_SIZE = 8k);
DRAM_END = 0x40000000;
_heap_end = DRAM_END - STACK_SIZE;

MEMORY
{
  vectors_seg (RX) : ORIGIN = 0x40100000, LENGTH = 0x100
  iram_seg (RX)    : ORIGIN = 0x40100100, LENGTH = 0x7f00
  dram_seg (RW)    : ORIGIN = 0x3ffe8000, LENGTH = 0x18000 - STACK_SIZE
  irom_seg (RX)    : ORIGIN = 0x40210000, LENGTH = 0xf0000
  /* 0x60001000..0x60001200 belongs to the SDK/ROM. */
  rtc_seg (RW)     : ORIGIN = 0x60001200, LENGTH = 0x200
}

/* map generic regions to output sections */
INCLUDE "alias.x"

/* esp8266 specific regions */
SECTIONS {
  .rtc.text : {
   . = ALIGN(4);
    *(.rtc.literal .rtc.text .rtc.literal.* .rtc.text.*)
  } > rtc_seg AT > RODATA

  .rtc.data :
  {
    _rtc_data_start = ABSOLUTE(.);
    . = ALIGN(4);
    *(.rtc.data .rtc.data.*)
    _rtc_data_end = ABSOLUTE(.);
  } > rtc_seg AT > RODATA

 .rtc.bss (NOLOAD) :
  {
    _rtc_bss_start = ABSOLUTE(.);
    . = ALIGN(4);
    *(.rtc.bss .rtc.bss.*)
    _rtc_bss_end = ABSOLUTE(.);
  } > rtc_seg

 .rtc.noinit (NOLOAD) :
  {
    . = ALIGN(4);
    *(.rtc.noinit .rtc.noinit.*)
  } > rtc_seg
}

ASSERT(_heap_start <= _heap_end, "Static DRAM overlaps the reserved application stack");
ASSERT(STACK_SIZE > 0 && STACK_SIZE < 0x18000 && (STACK_SIZE & 15) == 0,
       "STACK_SIZE must be positive, 16-byte aligned and smaller than DRAM");
_text_heap_end = ORIGIN(iram_seg) + LENGTH(iram_seg);

/* Application handlers have extern C fn() ABI, separate from the runtime's
 * exception default handler that accepts an interrupt level and context. */
PROVIDE(__slc_interrupt = __esp8266_hal_default_interrupt);
PROVIDE(__spi_interrupt = __esp8266_hal_default_interrupt);
PROVIDE(__gpio_interrupt = __esp8266_hal_default_interrupt);
PROVIDE(__uart_interrupt = __esp8266_hal_default_interrupt);
PROVIDE(__ccompare_interrupt = __esp8266_hal_default_interrupt);
PROVIDE(__soft_interrupt = __esp8266_hal_default_interrupt);
PROVIDE(__wdt_interrupt = __esp8266_hal_default_interrupt);
PROVIDE(__timer1_interrupt = __esp8266_hal_default_interrupt);
