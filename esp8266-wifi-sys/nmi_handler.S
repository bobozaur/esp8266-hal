/*
 * NMI handler that redirects to the SDK's WiFi blob NMI processing.
 *
 * This overrides xtensa-lx-rt's default __naked_nmi_exception handler.
 * The xtensa-lx-rt NMI vector (_NMIExceptionVector) saves a0 to EXCSAVE1
 * then calls __naked_nmi_exception via call0.
 *
 * We restore a0 and call the blob's wDev_ProcessFiq with full context
 * save/restore on the NMI stack, replicating the SDK's _xt_nmi handler.
 */

#define PRI_N_STACK_SIZE 1536
/* HESF offsets (High-priority Exception Save Frame) */
#define HESF_AR0   0x00
#define HESF_AR1   0x04
#define HESF_AR2   0x08
#define HESF_AR3   0x0C
#define HESF_AR4   0x10
#define HESF_AR5   0x14
#define HESF_AR6   0x18
#define HESF_AR7   0x1C
#define HESF_AR8   0x20
#define HESF_AR9   0x24
#define HESF_AR10  0x28
#define HESF_AR11  0x2C
#define HESF_AR12  0x30
#define HESF_AR13  0x34
#define HESF_AR14  0x38
#define HESF_AR15  0x3C
#define HESF_SAR   0x40
#define HESF_EPC1  0x44
#define HESF_EXCCAUSE  0x48
#define HESF_EXCVADDR  0x4C
#define HESF_EXCSAVE1  0x50
#define HESF_EPC3  0x54
#define HESF_EPS3  0x58

    .section .rwtext, "ax"
    .global __naked_nmi_exception
    .type   __naked_nmi_exception, @function
    .align  4
    .literal_position

__naked_nmi_exception:
    /* Restore a0 that NMI vector entry saved */
    rsr     a0, EXCSAVE1

    /* Check for NMI re-entry */
    movi    a0, _chip_nmi_cnt
    l32i    a0, a0, 0
    bnez    a0, .Lnmi_reentry
    movi    a0, _chip_nmi_stk + PRI_N_STACK_SIZE
    j       .Lnmi_save
.Lnmi_reentry:
    movi    a0, _chip_nmi_stk + PRI_N_STACK_SIZE + 256 + PRI_N_STACK_SIZE

.Lnmi_save:
    /* Save registers */
    s32i    a2, a0, HESF_AR2
    s32i    a1, a0, HESF_AR1
    s32i    a3, a0, HESF_AR3

    /* xtensa-lx-rt's ESP8266 NMI vector preserves the interrupted a0 in EXCSAVE1. */
    rsr     a2, EXCSAVE1
    s32i    a4, a0, HESF_AR4
    s32i    a2, a0, HESF_AR0     /* save original a0 */

    /* Save exception state */
    rsr     a3, EPC1
    rsr     a4, EXCCAUSE
    s32i    a3, a0, HESF_EPC1
    s32i    a4, a0, HESF_EXCCAUSE
    rsr     a3, EXCVADDR
    s32i    a3, a0, HESF_EXCVADDR
    rsr     a4, EXCSAVE1
    s32i    a4, a0, HESF_EXCSAVE1
    rsr     a3, EPC3
    s32i    a3, a0, HESF_EPC3
    rsr     a4, EPS3
    s32i    a4, a0, HESF_EPS3

    /* Save remaining registers */
    s32i    a5,  a0, HESF_AR5
    s32i    a6,  a0, HESF_AR6
    s32i    a7,  a0, HESF_AR7
    s32i    a8,  a0, HESF_AR8
    s32i    a9,  a0, HESF_AR9
    s32i    a10, a0, HESF_AR10
    s32i    a11, a0, HESF_AR11
    s32i    a12, a0, HESF_AR12
    s32i    a13, a0, HESF_AR13
    s32i    a14, a0, HESF_AR14
    s32i    a15, a0, HESF_AR15

    /* Switch to NMI stack, set up C calling convention */
    mov     a1, a0
    movi    a0, 0               /* mark end of call chain */

    /* Set PS for C code: UM=1, INTLEVEL=3, EXCM=0 */
    movi    a2, 0x00023
    wsr     a2, PS
    rsync

    /* Save SAR */
    rsr     a14, SAR
    s32i    a14, a1, HESF_SAR

    /* Increment NMI count */
    movi    a2, _chip_nmi_cnt
    l32i    a3, a2, 0
    addi    a3, a3, 1
    s32i    a3, a2, 0

    /* Increment persistent NMI counter for diagnostics */
    movi    a2, g_nmi_call_count
    l32i    a3, a2, 0
    addi    a3, a3, 1
    s32i    a3, a2, 0

    /* Call the blob's NMI handler */
    call0   wDev_ProcessFiq

    /* Decrement NMI count */
    movi    a2, _chip_nmi_cnt
    l32i    a3, a2, 0
    addi    a3, a3, -1
    s32i    a3, a2, 0

    /* Determine which save area to restore from */
    beqi    a3, 1, .Lnmi_restore_reentry
    movi    a1, _chip_nmi_stk + PRI_N_STACK_SIZE
    j       .Lnmi_restore
.Lnmi_restore_reentry:
    movi    a1, _chip_nmi_stk + PRI_N_STACK_SIZE + 256 + PRI_N_STACK_SIZE

.Lnmi_restore:
    /* Restore SAR */
    l32i    a15, a1, HESF_SAR
    wsr     a15, SAR

    /* Set PS for exit: UM=1, INTLEVEL=3, EXCM=1 */
    movi    a2, 0x00033
    wsr     a2, PS
    rsync

    /* Restore exception state */
    l32i    a2, a1, HESF_EPC1
    l32i    a3, a1, HESF_EXCCAUSE
    wsr     a2, EPC1
    wsr     a3, EXCCAUSE
    l32i    a2, a1, HESF_EXCVADDR
    wsr     a2, EXCVADDR
    l32i    a3, a1, HESF_EXCSAVE1
    wsr     a3, EXCSAVE1
    l32i    a2, a1, HESF_EPC3
    wsr     a2, EPC3
    l32i    a3, a1, HESF_EPS3
    wsr     a3, EPS3

    /* Restore registers */
    l32i    a0,  a1, HESF_AR0
    l32i    a4,  a1, HESF_AR4
    l32i    a5,  a1, HESF_AR5
    l32i    a6,  a1, HESF_AR6
    l32i    a7,  a1, HESF_AR7
    l32i    a8,  a1, HESF_AR8
    l32i    a9,  a1, HESF_AR9
    l32i    a10, a1, HESF_AR10
    l32i    a11, a1, HESF_AR11
    l32i    a12, a1, HESF_AR12
    l32i    a13, a1, HESF_AR13
    l32i    a14, a1, HESF_AR14
    l32i    a15, a1, HESF_AR15

    /* Re-enable NMI: write 1 to NMI_INT_ENABLE_REG (0x3FF00000) */
    rsr     a3, SAR
    movi    a2, 0x3FF00000
    wsr     a3, SAR
    rsync
    movi    a3, 1
    s32i    a3, a2, 0

    l32i    a2, a1, HESF_AR2
    l32i    a3, a1, HESF_AR3
    l32i    a1, a1, HESF_AR1

    rfi     3

    .size   __naked_nmi_exception, . - __naked_nmi_exception

/* _chip_nmi_stk is defined in Rust code (port.rs) with proper size. */
