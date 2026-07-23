/*
 * Custom link.x for esp8266-hal — overrides xtensa-lx-rt's default.
 *
 * The only change vs upstream: .text and .rodata sections also collect
 * .text2.*, .literal2.*, and .rodata2.* which come from the ESP8266 RTOS SDK
 * WiFi blob libraries (libnet80211.a, libpp.a, etc.).
 *
 * Without this, those sections become orphans with misaligned addresses,
 * causing the ESP8266 ROM bootloader to hang during segment loading.
 */

/* before memory.x to allow override */
ENTRY(Reset)

INCLUDE memory.x

/* Wi-Fi builds replace this marker with their vendored symbol maps and Rust
   SDK entry points. Non-Wi-Fi builds leave it empty. */
/* WIFI_LINK_INPUTS */

/* after memory.x to allow override */
PROVIDE(__pre_init = DefaultPreInit);
PROVIDE(__zero_bss = default_mem_hook);
PROVIDE(__init_data = default_mem_hook);

INCLUDE exception.x

SECTIONS {
  /* IRAM code MUST come before .text so it claims sections first.
     PHY calibration code runs with flash cache disabled and must be in IRAM. */
  .rwtext : ALIGN(4)
  {
    . = ALIGN (4);
    *(.rwtext.literal .rwtext .rwtext.literal.* .rwtext.*)

    /* All IRAM sections (Rust #[ram], C IRAM_ATTR, etc.) — literals first */
    *(.iram1.literal .iram1.literal.*)
    *(.iram1 .iram1.* .iram1.text .iram1.text.*)

    /* PHY blob functions that run with flash cache disabled.
       fix_cache_bug() calls Cache_Read_Disable then phy_get_bb_evm. */
    *(.literal.fix_cache_bug .text.fix_cache_bug)
    *(.literal.phy_get_bb_evm .text.phy_get_bb_evm)
    *(.literal.phy_get_bb_freqoffset .text.phy_get_bb_freqoffset)
    *(.literal.tx_pwr_backoff_old .text.tx_pwr_backoff_old)
    *(.literal.unsign_to_sign .text.unsign_to_sign)
  } > RWTEXT

  .text : ALIGN(4)
  {
    _stext = .;
    . = ALIGN (4);
    _text_start = ABSOLUTE(.);
    . = ALIGN (4);
    *(.literal .literal.* .text .text.*)
    /* SDK WiFi blob sections that upstream xtensa-lx-rt does not collect */
    *(.literal2 .literal2.* .text2 .text2.*)

    /* Large read-only SDK crypto tables are safe to fetch from mapped flash and
       otherwise consume a disproportionate amount of ESP8266 DRAM in EAP builds.
       Keep them in the .text output section so esptool sees one IROM section.
       The wpa_supplicant internal AES tables are kept full-sized so the
       generated code uses aligned 32-bit IROM loads instead of byte loads. */
    *(.rodata.Te*)
    *(.rodata.Td*)
    *(.rodata.s_aes_*)
    *(.rodata.SB*)
    *(.rodata.SP*)
    *(.rodata.FSb*)
    *(.rodata.K)
    *(.rodata.S)
    *(.rodata.P)
    *(.rodata.small_prime)
    *(.rodata.ciphersuite_definitions)
    *(.rodata.ciphersuite_preference)
    *(.rodata.mbedtls_cipher_definitions)
    _text_end = ABSOLUTE(.);
    _etext = .;
  } > ROTEXT

  .rodata : ALIGN(4)
  {
    _rodata_start = ABSOLUTE(.);
    . = ALIGN (4);
    *(.rodata .rodata.*)
    /* SDK WiFi blob sections that upstream xtensa-lx-rt does not collect */
    *(.rodata2 .rodata2.*)
    _rodata_end = ABSOLUTE(.);
  } > RODATA

  /* C++ constructor table (SDK components may register init functions) */
  .init_array : ALIGN(4)
  {
    __init_array_start = ABSOLUTE(.);
    KEEP(*(.init_array .init_array.* .ctors))
    __init_array_end = ABSOLUTE(.);
  } > RODATA

  .data : ALIGN(4)
  {
    _data_start = ABSOLUTE(.);
    . = ALIGN (4);
    *(.dram0 .dram0.*)
    *(.data .data.*)
    _data_end = ABSOLUTE(.);
  } > RWDATA

  /* LMA of .data */
  _sidata = LOADADDR(.data);

  .bss (NOLOAD) : ALIGN(4)
  {
    _bss_start = ABSOLUTE(.);
    . = ALIGN (4);
    *(.bss .bss.* COMMON)
    _bss_end = ABSOLUTE(.);
  } > RWDATA

  .noinit (NOLOAD) : ALIGN(4)
  {
    . = ALIGN(4);
    *(.noinit .noinit.*)
  } > RWDATA

 /* must be last segment using RWTEXT */
  .text_heap_start (NOLOAD) : ALIGN(4)
  {
    . = ALIGN (4);
    _text_heap_start = ABSOLUTE(.);
  } > RWTEXT

 /* must be last segment using RWDATA */
  .heap_start (NOLOAD) : ALIGN(4)
  {
    . = ALIGN (4);
    _heap_start = ABSOLUTE(.);
  } > RWDATA
}
