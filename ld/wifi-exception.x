/* ESP8266 vectors from xtensa-lx-rt 0.12.0 exception-esp8266.x, with the
 * Wi-Fi NMI vector selected explicitly. It saves A0 in EXCSAVE3, as the pinned
 * SDK does, so an NMI cannot destroy the level-one EXCSAVE1 entry state.
 * Other vector offsets and handler fallbacks retain the runtime's contract.
 */
PROVIDE(__exception = __default_exception);
PROVIDE(__kernel_exception = __default_exception);
PROVIDE(__double_exception = __default_double_exception);
/* The runtime groups its default NMI implementation with other IRAM handlers. */
PROVIDE(__nmi_exception = __default_exception);
PROVIDE(__debug_exception = __default_exception);
PROVIDE(__alloc_exception = __default_exception);
PROVIDE(__level_1_interrupt = __default_interrupt);

PROVIDE(__naked_user_exception = __default_naked_exception);
PROVIDE(__naked_kernel_exception = __default_naked_kernel_exception);
PROVIDE(__naked_double_exception = __default_naked_double_exception);
PROVIDE(__naked_debug_exception = __default_naked_debug_exception);
PROVIDE(__naked_alloc_exception = __default_naked_alloc_exception);

EXTERN(__default_exception);
EXTERN(__default_double_exception);
EXTERN(__default_interrupt);
EXTERN(__default_naked_exception);
EXTERN(__default_naked_double_exception);
EXTERN(__default_naked_debug_exception);
EXTERN(__default_naked_alloc_exception);

/* The Wi-Fi vector and handler are a pair: the runtime's default NMI handler
 * expects EXCSAVE1 and must not be used as a fallback for this vector.
 */
EXTERN(__esp8266_hal_nmi_vector);
EXTERN(__naked_nmi_exception);

SECTIONS {
  .vectors :
  {
    . = 0x0;
    _init_start = ABSOLUTE(.);
    . = 0x10;
    KEEP(*(.DebugException.text));
    . = 0x20;
    KEEP(*(.wifi.NMIException.text));
    . = 0x40;
    KEEP(*(.KernelException.text));
    . = 0x50;
    KEEP(*(.UserException.text));
    . = 0x70;
    KEEP(*(.DoubleException.text));
    . = 0x80;
    _init_end = ABSOLUTE(.);
  } > vectors_seg

  /DISCARD/ : { *(.NMIException.text) }
}
