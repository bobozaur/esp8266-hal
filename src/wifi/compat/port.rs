//! Port layer: ISR dispatch, tick handler, scheduler start, PendSV.
//!
//! Replaces the FreeRTOS C port and its legacy Xtensa assembly. Ordinary
//! scheduler logic stays in Rust; the few architectural save/restore sequences
//! that cannot be expressed otherwise use local inline assembly.
//!
//! ISR-path functions are placed in `.iram1` to avoid flash cache misses
//! during interrupt handling.

use core::arch::asm;
use core::ffi::c_void;

use portable_atomic::{AtomicBool, Ordering};

use crate::ram;

use super::critical;
use super::task;

// ---------------------------------------------------------------------------
// NMI stack buffer
// ---------------------------------------------------------------------------

/// NMI stack and save-frame buffer used by [`__naked_nmi_exception`].
///
/// Layout (offsets from base):
///   +0x000 .. +0x5FF : base NMI C stack (1536 bytes, grows down)
///   +0x600 .. +0x658 : base save frame (HESF: a0-a15, SAR, EPC/EXC state)
///   +0x659 .. +0x6FF : padding
///   +0x700 .. +0xCFF : re-entry NMI C stack (1536 bytes, grows down)
///   +0xD00 .. +0xD58 : re-entry save frame
///
/// Without this allocation, the assembly-referenced `_chip_nmi_stk` resolves
/// to a zero-size symbol from the SDK, and the save frames write into the
/// heap — corrupting allocator metadata on every NMI.
/// Aligned NMI stack buffer. Must be 16-byte aligned for Xtensa call0 ABI.
#[repr(C, align(16))]
struct NmiStack([u8; 0xD80]);

#[no_mangle]
#[link_section = ".bss"]
static mut _chip_nmi_stk: NmiStack = NmiStack([0u8; 0xD80]);

#[no_mangle]
#[link_section = ".bss"]
static mut _chip_nmi_cnt: u32 = 0;

/// Redirect the ESP8266 NMI vector to the closed Wi-Fi runtime.
///
/// The vector enters without a usable Rust stack, so this leaf must save the
/// complete interrupted register set before calling `wDev_ProcessFiq`.
#[naked]
#[no_mangle]
#[link_section = ".rwtext"]
pub unsafe extern "C" fn __naked_nmi_exception() -> ! {
    unsafe {
        asm!(
            "rsr a0, EXCSAVE1",
            "movi a0, _chip_nmi_cnt",
            "l32i a0, a0, 0",
            "bnez a0, 2f",
            "movi a0, _chip_nmi_stk + 1536",
            "j 3f",
            "2:",
            "movi a0, _chip_nmi_stk + 3328",
            "3:",
            "s32i a2, a0, 0x08",
            "s32i a1, a0, 0x04",
            "s32i a3, a0, 0x0c",
            "rsr a2, EXCSAVE1",
            "s32i a4, a0, 0x10",
            "s32i a2, a0, 0x00",
            "rsr a3, EPC1",
            "rsr a4, EXCCAUSE",
            "s32i a3, a0, 0x44",
            "s32i a4, a0, 0x48",
            "rsr a3, EXCVADDR",
            "s32i a3, a0, 0x4c",
            "rsr a4, EXCSAVE1",
            "s32i a4, a0, 0x50",
            "rsr a3, EPC3",
            "s32i a3, a0, 0x54",
            "rsr a4, EPS3",
            "s32i a4, a0, 0x58",
            "s32i a5, a0, 0x14",
            "s32i a6, a0, 0x18",
            "s32i a7, a0, 0x1c",
            "s32i a8, a0, 0x20",
            "s32i a9, a0, 0x24",
            "s32i a10, a0, 0x28",
            "s32i a11, a0, 0x2c",
            "s32i a12, a0, 0x30",
            "s32i a13, a0, 0x34",
            "s32i a14, a0, 0x38",
            "s32i a15, a0, 0x3c",
            "mov a1, a0",
            "movi a0, 0",
            "movi a2, 0x23",
            "wsr a2, PS",
            "rsync",
            "rsr a14, SAR",
            "s32i a14, a1, 0x40",
            "movi a2, _chip_nmi_cnt",
            "l32i a3, a2, 0",
            "addi a3, a3, 1",
            "s32i a3, a2, 0",
            // `wDev_ProcessFiq` lives in flash, outside the direct CALL0
            // range from IRAM. This is the explicit long-call sequence.
            "movi a0, wDev_ProcessFiq",
            "callx0 a0",
            "movi a2, _chip_nmi_cnt",
            "l32i a3, a2, 0",
            "addi a3, a3, -1",
            "s32i a3, a2, 0",
            "beqi a3, 1, 4f",
            "movi a1, _chip_nmi_stk + 1536",
            "j 5f",
            "4:",
            "movi a1, _chip_nmi_stk + 3328",
            "5:",
            "l32i a15, a1, 0x40",
            "wsr a15, SAR",
            "movi a2, 0x33",
            "wsr a2, PS",
            "rsync",
            "l32i a2, a1, 0x44",
            "l32i a3, a1, 0x48",
            "wsr a2, EPC1",
            "wsr a3, EXCCAUSE",
            "l32i a2, a1, 0x4c",
            "wsr a2, EXCVADDR",
            "l32i a3, a1, 0x50",
            "wsr a3, EXCSAVE1",
            "l32i a2, a1, 0x54",
            "wsr a2, EPC3",
            "l32i a3, a1, 0x58",
            "wsr a3, EPS3",
            "l32i a0, a1, 0x00",
            "l32i a4, a1, 0x10",
            "l32i a5, a1, 0x14",
            "l32i a6, a1, 0x18",
            "l32i a7, a1, 0x1c",
            "l32i a8, a1, 0x20",
            "l32i a9, a1, 0x24",
            "l32i a10, a1, 0x28",
            "l32i a11, a1, 0x2c",
            "l32i a12, a1, 0x30",
            "l32i a13, a1, 0x34",
            "l32i a14, a1, 0x38",
            "l32i a15, a1, 0x3c",
            "rsr a3, SAR",
            "movi a2, 0x3ff00000",
            "wsr a3, SAR",
            "rsync",
            "movi a3, 1",
            "s32i a3, a2, 0",
            "l32i a2, a1, 0x08",
            "l32i a3, a1, 0x0c",
            "l32i a1, a1, 0x04",
            "rfi 3",
            options(noreturn)
        );
    }
}

/// Target ABI helper required by the vendored mbedTLS objects.
///
/// Rust's compiler builtins do not export this GCC spelling on Xtensa. Keep
/// the tiny operation here rather than vendoring an entire libgcc archive.
#[naked]
#[no_mangle]
#[link_section = ".rwtext"]
unsafe extern "C" fn __bswapsi2(_value: u32) -> u32 {
    unsafe {
        asm!(
            "extui a3, a2, 0, 8",
            "slli a3, a3, 24",
            "extui a4, a2, 8, 8",
            "slli a4, a4, 16",
            "or a3, a3, a4",
            "extui a4, a2, 16, 8",
            "slli a4, a4, 8",
            "or a3, a3, a4",
            "extui a4, a2, 24, 8",
            "or a2, a3, a4",
            "ret.n",
            options(noreturn)
        );
    }
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Tick rate in Hz (SDK default). At 100Hz, each tick is 10ms.
/// ROM timers have 5MHz resolution independent of tick rate.
const TICK_RATE_HZ: u32 = 100;
/// Tick period in milliseconds.
const TICK_PERIOD_MS: u32 = 1000 / TICK_RATE_HZ;
/// Number of ISR slots.
/// Number of ISR table slots. This is larger than `ETS_INT_MAX` (14) to
/// accommodate potential SDK-internal interrupts above the documented range.
const ISR_COUNT: usize = 16;
/// Software interrupt number.
const ETS_SOFT_INUM: u8 = 7;
/// Timer (CCOMPARE) interrupt number.
const ETS_MAX_INUM: u8 = 6;
/// Maximum hardware interrupt number.
const ETS_INT_MAX: usize = 14;

// ---------------------------------------------------------------------------
// Globals shared with interrupt entry code and native SDK objects
// ---------------------------------------------------------------------------

/// Tick divisor: CPU cycles per tick period.
#[no_mangle]
pub static mut _xt_tick_divisor: u32 = 0;

/// CCOUNT at boot (before scheduler starts).
#[no_mangle]
pub static mut g_esp_boot_ccount: u32 = 0;

/// Cumulative OS tick count (64-bit).
#[no_mangle]
pub static mut g_esp_os_ticks: u64 = 0;

/// Cumulative microseconds since scheduler start.
#[no_mangle]
pub static mut g_esp_os_us: u64 = 0;

/// Cumulative CPU clock cycles since scheduler start.
#[no_mangle]
pub static mut g_esp_os_cpu_clk: u64 = 0;

/// Context-switch request flag, checked by `_xt_isr_handler` after ISR dispatch.
static mut S_SWITCH_CTX_FLAG: u32 = 0;

#[no_mangle]
pub static mut g_level1_restore_sp: u32 = 0;
static POLL_WDEV_ON_TICK: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// ISR table
// ---------------------------------------------------------------------------

type IsrHandler = unsafe extern "C" fn(*mut c_void);

#[repr(C)]
struct IsrEntry {
    handler: Option<IsrHandler>,
    arg: *mut c_void,
}

static mut ISR_TABLE: [IsrEntry; ISR_COUNT] = {
    const EMPTY: IsrEntry = IsrEntry {
        handler: None,
        arg: core::ptr::null_mut(),
    };
    [EMPTY; ISR_COUNT]
};

/// Whether we are currently inside an ISR.
static mut ISR_NESTING: u8 = 0;

// ---------------------------------------------------------------------------
// Inline asm helpers for Xtensa special registers
// ---------------------------------------------------------------------------

#[inline(always)]
unsafe fn get_ccount() -> u32 {
    let v: u32;
    unsafe { asm!("rsr {0}, ccount", out(reg) v, options(nomem, nostack)) };
    v
}

#[inline(always)]
unsafe fn set_ccount(v: u32) {
    unsafe { asm!("wsr {0}, ccount", in(reg) v, options(nomem, nostack)) };
}

#[inline(always)]
unsafe fn set_ccompare(v: u32) {
    unsafe { asm!("wsr {0}, ccompare0", in(reg) v, options(nomem, nostack)) };
}

#[inline(always)]
unsafe fn get_interrupt_mask() -> u32 {
    let intr: u32;
    let enable: u32;
    unsafe {
        asm!("rsr {0}, interrupt", out(reg) intr, options(nomem, nostack));
        asm!("rsr {0}, intenable", out(reg) enable, options(nomem, nostack));
    };
    intr & enable
}

#[inline(always)]
unsafe fn clear_interrupt(mask: u32) {
    unsafe { asm!("wsr {0}, intclear", in(reg) mask, options(nomem, nostack)) };
}

#[inline(always)]
unsafe fn set_intset(mask: u32) {
    unsafe { asm!("wsr {0}, intset", in(reg) mask, options(nomem, nostack)) };
}

// ---------------------------------------------------------------------------
// External symbols retained in the native SDK runtime
// ---------------------------------------------------------------------------

extern "C" {
    fn MacIsrSigPostDefHdl() -> i32;
    /// Ticks per microsecond (defined in system_api.c, still compiled).
    static g_esp_ticks_per_us: u32;
    /// CPU clock frequency in Hz (defined in system_api.c).
    /// Returns the actual runtime frequency (80MHz or 160MHz).
    fn xtbsp_clock_freq_hz() -> u32;
}

// ---------------------------------------------------------------------------
// PendSV — trigger a software interrupt for deferred context switch
// ---------------------------------------------------------------------------

/// `PendSV` — request a context switch via the software interrupt.
///
/// `req == 1`: enter critical section, set flag, trigger soft ISR.
/// `req == 2`: just trigger soft ISR (already in critical section).
#[no_mangle]
#[link_section = ".iram1"]
pub unsafe extern "C" fn PendSV(req: i32) {
    unsafe {
        if req == 1 {
            critical::vPortEnterCritical();
            S_SWITCH_CTX_FLAG = 1;
            set_intset(1 << ETS_SOFT_INUM);
            critical::vPortExitCritical();
        } else if req == 2 {
            set_intset(1 << ETS_SOFT_INUM);
        }
    }
}

/// `portYIELD_FROM_ISR` — mark that a context switch is needed after ISR.
#[no_mangle]
#[link_section = ".iram1"]
pub unsafe extern "C" fn portYIELD_FROM_ISR() {
    unsafe {
        S_SWITCH_CTX_FLAG = 1;
    }
}

// ---------------------------------------------------------------------------
// ISR management
// ---------------------------------------------------------------------------

/// `_xt_isr_attach` — register an ISR handler for interrupt `i`.
#[no_mangle]
pub unsafe extern "C" fn _xt_isr_attach(
    i: u8,
    func: unsafe extern "C" fn(*mut c_void),
    arg: *mut c_void,
) {
    unsafe {
        if (i as usize) < ISR_COUNT {
            ISR_TABLE[i as usize].handler = Some(func);
            ISR_TABLE[i as usize].arg = arg;
        }
    }
}

#[no_mangle]
#[link_section = ".iram1"]
pub unsafe extern "C" fn _xt_isr_unmask(mask: u32) -> u32 {
    let previous: u32;
    let _updated: u32;
    let _saved_ps: u32;
    unsafe {
        asm!(
            "rsil {saved_ps}, 3",
            "rsr {previous}, intenable",
            "or {updated}, {previous}, {mask}",
            "wsr {updated}, intenable",
            "wsr {saved_ps}, ps",
            "rsync",
            saved_ps = out(reg) _saved_ps,
            previous = out(reg) previous,
            updated = out(reg) _updated,
            mask = in(reg) mask,
            options(nostack)
        );
    }
    previous
}

#[no_mangle]
#[link_section = ".iram1"]
pub unsafe extern "C" fn _xt_isr_mask(mask: u32) -> u32 {
    let previous: u32;
    let _updated: u32;
    let _saved_ps: u32;
    unsafe {
        asm!(
            "rsil {saved_ps}, 3",
            "rsr {previous}, intenable",
            "or {updated}, {previous}, {mask}",
            "xor {updated}, {updated}, {mask}",
            "wsr {updated}, intenable",
            "wsr {saved_ps}, ps",
            "rsync",
            saved_ps = out(reg) _saved_ps,
            previous = out(reg) previous,
            updated = out(reg) _updated,
            mask = in(reg) mask,
            options(nostack)
        );
    }
    previous
}

#[no_mangle]
#[link_section = ".iram1"]
unsafe extern "C" fn _xt_tick_timer_init() {
    unsafe {
        set_ccompare(get_ccount().wrapping_add(_xt_tick_divisor));
        _xt_isr_unmask(1 << ETS_MAX_INUM);
    }
}

/// SDK-compatible ISR dispatch entry.
///
/// Kept as an SDK-compatible dispatch symbol. The HAL's Rust level-one entry
/// calls [`isr_dispatch`] and [`apply_pending_context_switch`] directly with
/// its live frame.
#[no_mangle]
#[link_section = ".rwtext"]
pub unsafe extern "C" fn _xt_isr_handler() {
    unsafe {
        isr_dispatch();

        if S_SWITCH_CTX_FLAG != 0 {
            task::vTaskSwitchContext();
            S_SWITCH_CTX_FLAG = 0;
        }
    }
}

/// Core ISR dispatch: read pending interrupt bits and call registered handlers
/// from ISR_TABLE. Called from `__level_1_interrupt` (the xtensa-lx-rt entry
/// point) and from `_xt_isr_handler` (the SDK-compatible entry point).
///
/// Context switching is NOT handled here — the caller is responsible for
/// calling `apply_pending_context_switch` after dispatch returns.
#[inline(never)]
#[link_section = ".rwtext"]
pub unsafe fn isr_dispatch() {
    unsafe {
        loop {
            let mask = get_interrupt_mask();
            if mask == 0 {
                break;
            }

            for i in 0..ETS_INT_MAX {
                let bit = 1u32 << i;
                if (mask & bit) == 0 {
                    continue;
                }

                // Always clear the hardware interrupt bit to prevent
                // infinite re-dispatch of unhandled interrupts.
                clear_interrupt(bit);

                if let Some(handler) = ISR_TABLE[i].handler {
                    ISR_NESTING = 1;
                    handler(ISR_TABLE[i].arg);
                    ISR_NESTING = 0;
                }
            }
        }
    }
}

/// `xPortInIsrContext` — returns non-zero if currently in an ISR.
#[no_mangle]
pub unsafe extern "C" fn xPortInIsrContext() -> i32 {
    unsafe { ISR_NESTING as i32 }
}

/// Enable polling of the WiFi FIQ processor from the tick ISR.
///
/// Hardware validation shows this is required for closed-library paths such as
/// SmartConfig to drain promiscuous RX traffic at the expected rate.
pub fn set_wdev_tick_poll_enabled(enabled: bool) {
    POLL_WDEV_ON_TICK.store(enabled, Ordering::Release);
}

/// Apply a pending task switch using the live xtensa-lx-rt interrupt frame.
///
/// xtensa-lx-rt restores CPU state from the current live level-1 interrupt
/// frame on return, so a compat task switch must overwrite that live frame
/// with the selected task's saved frame before control returns to the outer
/// xtensa-lx-rt restore sequence.
#[ram]
pub unsafe fn apply_pending_context_switch(frame: *mut u8) {
    unsafe {
        if S_SWITCH_CTX_FLAG == 0 {
            return;
        }

        if task::scheduler_is_suspended() {
            task::vTaskSwitchContext();
            S_SWITCH_CTX_FLAG = 0;
            return;
        }

        task::save_current_frame(frame);
        task::vTaskSwitchContext();
        g_level1_restore_sp = task::prepare_current_frame_for_restore() as u32;
        S_SWITCH_CTX_FLAG = 0;
    }
}

// ---------------------------------------------------------------------------
// Soft ISR handler
// ---------------------------------------------------------------------------

/// `SoftIsrHdl` — software interrupt handler (ETS_SOFT_INUM).
///
/// Match SDK `port.c`: only the WiFi/MAC deferred handler decides whether
/// the soft interrupt should request a context switch.
#[no_mangle]
#[link_section = ".iram1"]
pub unsafe extern "C" fn SoftIsrHdl(_arg: *mut c_void) {
    unsafe {
        if MacIsrSigPostDefHdl() != 0 {
            portYIELD_FROM_ISR();
        }
    }
}

// ---------------------------------------------------------------------------
// Tick handler
// ---------------------------------------------------------------------------

extern "C" {
    /// WiFi blob's NMI handler — process received frames and wDev events.
    /// Called from NMI normally; we also poll it from the tick ISR.
    fn wDev_ProcessFiq();
}

/// `xPortSysTickHandle` — timer interrupt handler (ETS_MAX_INUM / CCOMPARE).
#[no_mangle]
#[link_section = ".iram1"]
pub unsafe extern "C" fn xPortSysTickHandle(_p: *mut c_void) {
    unsafe {
        // Match SDK port.c: reset CCOUNT, compute elapsed microseconds,
        // catch up missed ticks via vTaskStepTick.
        let ccount = get_ccount();
        let us = ccount / g_esp_ticks_per_us;
        g_esp_os_us += us as u64;
        g_esp_os_cpu_clk += ccount as u64;
        set_ccount(0);
        set_ccompare(_xt_tick_divisor);

        // SDK port.c: if more than one tick period elapsed (ISR was delayed
        // by NMI processing or context switch), advance the tick counter to
        // catch up. Without this, FreeRTOS timers and task delays drift.
        let ticks = us / 1000 / TICK_PERIOD_MS;
        if ticks > 1 {
            task::vTaskStepTick(ticks - 1);
        }

        g_esp_os_ticks += 1;

        // NOTE: SDK does NOT call pp_post_from_isr here. ROM timers are
        // processed by ets_timer_task (via ets_timer_proc) and by FRC1
        // interrupt directly. Posting 0x10 every tick floods ppTask and
        // prevents it from processing auth/EAPOL frames promptly.

        if POLL_WDEV_ON_TICK.load(Ordering::Acquire) {
            wDev_ProcessFiq();
        }

        if task::xTaskIncrementTick() != 0 {
            portYIELD_FROM_ISR();
        }
    }
}

// ---------------------------------------------------------------------------
// Tick counter helpers
// ---------------------------------------------------------------------------

/// `esp_increase_tick_cnt` — add to the OS tick counter.
#[no_mangle]
pub unsafe extern "C" fn esp_increase_tick_cnt(ticks: u32) {
    unsafe {
        g_esp_os_ticks += ticks as u64;
    }
}

/// `esp_clk_cpu_freq` — return current CPU clock frequency.
#[no_mangle]
pub unsafe extern "C" fn esp_clk_cpu_freq() -> i32 {
    unsafe { (_xt_tick_divisor * TICK_RATE_HZ) as i32 }
}

/// `xPortGetTickRateHz` — return the tick rate.
#[no_mangle]
pub unsafe extern "C" fn xPortGetTickRateHz() -> u32 {
    TICK_RATE_HZ
}

// ---------------------------------------------------------------------------
// Scheduler start
// ---------------------------------------------------------------------------

/// Prepare the compat scheduler infrastructure.
///
/// This sets up idle/timer tasks and ISR handlers, but does not yet start the
/// tick timer or mark the scheduler as running.
unsafe fn prepare_compat_scheduler(register_current_task: bool) {
    unsafe {
        // Initialize cpu_sr (see port.c TAG 1.2.3 comment).
        critical::cpu_sr = 0x20;

        if register_current_task {
            // Background mode: treat the caller's current execution context as
            // a schedulable task so compat APIs can yield back into it later.
            task::allocate_main_task();
        }

        // Create the idle task (priority 0, feeds WDT, cleans deleted tasks).
        task::create_idle_task();

        // Create the timer daemon task.
        super::timer::create_timer_daemon();

        // Attach software interrupt handler (MAC deferred processing).
        _xt_isr_attach(ETS_SOFT_INUM, SoftIsrHdl, core::ptr::null_mut());
        _xt_isr_unmask(1 << ETS_SOFT_INUM);

        // Attach tick timer ISR.
        _xt_isr_attach(ETS_MAX_INUM, xPortSysTickHandle, core::ptr::null_mut());

        // Compute tick divisor from actual CPU frequency (80MHz or 160MHz).
        _xt_tick_divisor = xtbsp_clock_freq_hz() / TICK_RATE_HZ;
    }
}

/// Start the prepared scheduler timing/ISR machinery.
unsafe fn activate_prepared_scheduler() {
    unsafe {
        // Record boot CCOUNT, then reset for first tick.
        g_esp_boot_ccount = get_ccount();
        set_ccount(0);

        // Start the tick timer (assembly: sets CCOMPARE = _xt_tick_divisor).
        _xt_tick_timer_init();

        // Disable software WDT. The sleeping executor prevents the idle
        // task from running, so the WDT must be disabled here.
        extern "C" {
            fn pp_soft_wdt_stop();
        }
        pp_soft_wdt_stop();

        // Mark scheduler as started so compat API calls work.
        task::mark_scheduler_started();
    }
}

/// Initialize the compat scheduler infrastructure (ISRs, tick timer, idle
/// and timer daemon tasks) and return control to the caller.
///
/// This is the embassy-first entry path: the caller (user code) owns the
/// main thread and runs an embassy executor. WiFi blob tasks created later
/// will run via ISR-driven preemptive context switches.
///
/// # Safety
///
/// Must be called exactly once, after hardware init.
pub(crate) unsafe fn start_compat_scheduler() {
    unsafe {
        prepare_scheduler_with_boot_task();
        activate_prepared_scheduler_only();
    }
}

/// Prepare the background scheduler path by registering the current execution
/// context as a temporary schedulable task.
pub(crate) unsafe fn prepare_scheduler_with_boot_task() {
    unsafe {
        prepare_compat_scheduler(true);
    }
}

/// Activate a previously prepared scheduler without forcing an immediate jump
/// into the first task.
pub(crate) unsafe fn activate_prepared_scheduler_only() {
    unsafe {
        activate_prepared_scheduler();
    }
}

/// Restore the first selected task using the compat xtensa-lx-rt frame layout.
///
/// This mirrors xtensa-lx-rt's `.RestoreContext` path rather than the SDK
/// `_xt_enter_first_task`, because compat-created tasks carry 0x100-byte
/// exception frames with `PC/PS/A0/A1` at the xtensa-lx-rt offsets.
#[naked]
#[link_section = ".rwtext"]
unsafe extern "C" fn compat_enter_first_task() -> ! {
    unsafe {
        asm!(
            "movi a2, pxCurrentTCB",
            "l32i a2, a2, 0",
            "l32i a1, a2, 0",
            "addi a3, a1, 0x100",
            "s32i a3, a2, 0",
            "call0 restore_context",
            "l32i a0, a1, 0x04",
            "wsr.ps a0",
            "l32i a0, a1, 0x00",
            "wsr.epc1 a0",
            "l32i a0, a1, 0x08",
            "l32i a1, a1, 0x0c",
            "rsync",
            ".byte 0x00, 0x30, 0x00",
            options(noreturn)
        );
    }
}

/// Finish scheduler startup by selecting and entering the first task, matching
/// the SDK `xPortStartScheduler()` handoff after the scheduler has already been
/// prepared and activated.
pub(crate) unsafe fn enter_first_prepared_task() -> ! {
    unsafe {
        task::vTaskSwitchContext();
        compat_enter_first_task()
    }
}

/// `xPortStartScheduler` — FreeRTOS-compatible entry that starts the scheduler
/// and jumps to the first task (never returns).
///
/// This is kept as a `#[no_mangle]` symbol in case any blob code calls it
/// directly. In the embassy-first architecture, prefer
/// [`start_compat_scheduler`] which returns control to the caller.
#[no_mangle]
pub unsafe extern "C" fn xPortStartScheduler() -> i32 {
    unsafe {
        start_compat_scheduler();

        // Select the first task to run.
        task::vTaskSwitchContext();

        // Jump to the first task — this never returns.
        compat_enter_first_task()
    }
}

// ---------------------------------------------------------------------------
// Stack initialisation (single implementation, used by task.rs and C code)
// ---------------------------------------------------------------------------

/// `__cpu_init_stk` — build an initial interrupt frame on a stack.
///
/// Called from `xTaskCreate` (via `task::init_stack`) and from C code.
#[no_mangle]
pub unsafe extern "C" fn __cpu_init_stk(
    stack_top: *mut u8,
    entry: unsafe extern "C" fn(*mut c_void),
    param: *mut c_void,
    _exit: unsafe extern "C" fn(),
) -> *mut u8 {
    unsafe { task::init_stack(stack_top, entry, param) }
}

/// `pxPortInitialiseStack` — FreeRTOS-style stack init wrapper.
#[no_mangle]
pub unsafe extern "C" fn pxPortInitialiseStack(
    top_of_stack: *mut u32,
    task_code: unsafe extern "C" fn(*mut c_void),
    parameters: *mut c_void,
) -> *mut u32 {
    unsafe { task::init_stack(top_of_stack as *mut u8, task_code, parameters) as *mut u32 }
}

/// `vPortInitContextFromOldStack` — copy context to a new stack.
///
/// Used by `vTaskAllocateNewStackAndDeleteOldStack` in some SDK paths.
#[no_mangle]
pub unsafe extern "C" fn vPortInitContextFromOldStack(
    new_top: *mut u32,
    old_top: *mut u32,
    stack_size: u32,
) {
    unsafe {
        core::ptr::copy_nonoverlapping(
            old_top as *const u8,
            new_top as *mut u8,
            stack_size as usize,
        );
        // Fix up A1 (stack pointer) in the new frame — must point into the
        // new stack, not the old one.
        let sp = new_top;
        sp.add(task::XT_STK_A1 >> 2)
            .write(new_top as u32 + task::XT_STK_FRMSZ as u32);
    }
}

// ---------------------------------------------------------------------------
// Hooks (weak-like defaults — C code may override via linker)
// ---------------------------------------------------------------------------

/// `vApplicationStackOverflowHook` — called on stack overflow detection.
#[no_mangle]
pub unsafe extern "C" fn vApplicationStackOverflowHook(_task: *mut c_void, _task_name: *const u8) {
    loop {}
}

/// `vApplicationIdleHook` — called from the idle task (placeholder).
#[no_mangle]
pub unsafe extern "C" fn vApplicationIdleHook() {}

/// `vApplicationTickHook` — called on each tick (placeholder).
#[no_mangle]
pub unsafe extern "C" fn vApplicationTickHook() {}

extern "C" {
    fn esp_task_wdt_reset();
    fn esp_sleep_start();
}

/// `esp_internal_idle_hook` — SDK path: feed WDT, then enter light sleep.
#[no_mangle]
pub unsafe extern "C" fn esp_internal_idle_hook() {
    unsafe {
        esp_task_wdt_reset();
        esp_sleep_start();
    }
}
