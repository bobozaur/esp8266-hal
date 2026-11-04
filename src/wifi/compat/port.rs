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

/// Wi-Fi-specific vector selected by ld/wifi-exception.x. The runtime's
/// default vector saves A0 in EXCSAVE1, which can overwrite level-one entry
/// state before that handler has saved it. Match the SDK's separate NMI bank.
#[naked]
#[no_mangle]
#[link_section = ".wifi.NMIException.text"]
unsafe extern "C" fn __esp8266_hal_nmi_vector() -> ! {
    unsafe {
        asm!(
            "wsr a0, EXCSAVE3",
            "call0 __naked_nmi_exception",
            options(noreturn)
        );
    }
}

/// Redirect the ESP8266 NMI vector to the closed Wi-Fi runtime.
///
/// The vector enters without a usable Rust stack, so this leaf must save the
/// complete interrupted register set before calling `wDev_ProcessFiq`. A0
/// arrives in EXCSAVE3; EXCSAVE1 is preserved separately for level-one entry.
#[naked]
#[no_mangle]
#[link_section = ".rwtext"]
pub unsafe extern "C" fn __naked_nmi_exception() -> ! {
    unsafe {
        asm!(
            "rsr a0, EXCSAVE3",
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
            "rsr a2, EXCSAVE3",
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
            // Match xtensa_vectors.S:nmi_reentried/nmi_rfi in SDK
            // da7ce37be424d71874241ca9f17cf8da0e3cdb70. An NMI at the
            // final RFI must retain the original return PC/PS; saving that
            // RFI's PC and exit PS would make the return target itself.
            "movi a4, 7f",
            "beq a3, a4, 6f",
            "s32i a3, a0, 0x54",
            "rsr a4, EPS3",
            "s32i a4, a0, 0x58",
            "6:",
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
            "7:",
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

/// Context-switch request consumed after the level-one interrupt dispatch.
static mut S_SWITCH_CTX_FLAG: u32 = 0;

#[no_mangle]
pub static mut g_level1_restore_sp: u32 = 0;
static POLL_WDEV_ON_TICK: AtomicBool = AtomicBool::new(false);
static mut CLOCK: crate::wifi::runtime_clock::RuntimeClock =
    crate::wifi::runtime_clock::RuntimeClock::new(0);

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
    static mut g_esp_ticks_per_us: u32;
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

/// Program an absolute compare without leaving a past deadline armed.
/// The caller excludes ordinary interrupts; an NMI may still delay any write.
#[inline(always)]
pub(crate) unsafe fn program_compare(requested: u32) {
    unsafe {
        loop {
            let deadline = crate::wifi::runtime_clock::compare_deadline(get_ccount(), requested);
            set_ccompare(deadline);
            if (deadline.wrapping_sub(get_ccount()) as i32) > 0 {
                return;
            }
        }
    }
}

/// Monotonic microseconds for SDK callers. The native helper assumed CCOUNT was
/// reset every tick; this runtime keeps it free-running so ISR work is counted.
#[export_name = "__esp8266_hal_esp_timer_get_time"]
pub extern "C" fn esp_timer_get_time() -> i64 {
    critical_section::with(|_| unsafe {
        (&*core::ptr::addr_of!(CLOCK)).micros(get_ccount(), g_esp_ticks_per_us) as i64
    })
}

/// Settle the old CPU-clock epoch before updating the SDK's conversion globals.
pub(crate) unsafe fn update_cpu_frequency(ticks_per_us: u32) {
    critical_section::with(|_| unsafe {
        if _xt_tick_divisor != 0 {
            let deadline = (*core::ptr::addr_of_mut!(CLOCK)).change_frequency(
                get_ccount(),
                g_esp_ticks_per_us,
                ticks_per_us,
            );
            g_esp_ticks_per_us = ticks_per_us;
            _xt_tick_divisor = ticks_per_us * 1_000_000 / TICK_RATE_HZ;
            program_compare(deadline);
            #[cfg(feature = "embassy")]
            crate::wifi::timer_driver::reschedule();
        } else {
            g_esp_ticks_per_us = ticks_per_us;
        }
    });
}

/// Core ISR dispatch: read pending interrupt bits and call registered handlers
/// from ISR_TABLE or the application's static handlers. Called by the HAL's
/// level-one interrupt entry.
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

                ISR_NESTING = 1;
                if let Some(handler) = ISR_TABLE[i].handler {
                    handler(ISR_TABLE[i].arg);
                } else {
                    crate::interrupt::dispatch_app_interrupts(bit);
                }
                ISR_NESTING = 0;
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
/// Retained for the promiscuous receive path: hardware validation has not
/// qualified that path without polling. SmartConfig uses hardware NMI delivery.
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
        let elapsed = (*core::ptr::addr_of_mut!(CLOCK)).advance(get_ccount(), g_esp_ticks_per_us);
        program_compare(elapsed.next_tick);

        // Executor alarms may fire before the next SDK tick. Preserve the
        // remaining period instead of counting every compare IRQ as 10ms.
        if elapsed.ticks > 0 {
            if elapsed.ticks > 1 {
                task::vTaskStepTick(elapsed.ticks - 1);
            }
            if task::xTaskIncrementTick() != 0 {
                portYIELD_FROM_ISR();
            }
        }
        #[cfg(feature = "embassy")]
        crate::wifi::timer_driver::poll_alarms();

        if POLL_WDEV_ON_TICK.load(Ordering::Acquire) {
            // The native handler masks NMI before returning; its hardware
            // vector normally reopens that gate. Polling must preserve the
            // caller's gate and exclude a concurrent hardware invocation.
            critical_section::with(|_| wDev_ProcessFiq());
        }
    }
}

// ---------------------------------------------------------------------------
// Tick counter helpers
// ---------------------------------------------------------------------------

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

/// Allocate mandatory scheduler infrastructure before enabling any runtime ISR.
unsafe fn prepare_compat_scheduler() -> Result<(), ()> {
    unsafe {
        critical::cpu_sr = 0x20;
        task::allocate_main_task();
        if task::create_idle_task() != task::PD_PASS
            || super::timer::create_timer_daemon() != task::PD_PASS
        {
            super::timer::rollback_startup();
            task::rollback_startup();
            return Err(());
        }

        _xt_isr_attach(ETS_SOFT_INUM, SoftIsrHdl, core::ptr::null_mut());
        _xt_isr_attach(ETS_MAX_INUM, xPortSysTickHandle, core::ptr::null_mut());
        _xt_tick_divisor = xtbsp_clock_freq_hz() / TICK_RATE_HZ;
        Ok(())
    }
}

/// Start the prepared scheduler timing/ISR machinery.
unsafe fn activate_prepared_scheduler() {
    unsafe {
        // Capture the initial epoch without resetting the free-running counter.
        let counter = get_ccount();
        CLOCK = crate::wifi::runtime_clock::RuntimeClock::new(counter);
        program_compare(counter.wrapping_add(_xt_tick_divisor));
        _xt_isr_unmask(1 << ETS_MAX_INUM);

        // Retain the proven SDK software-WDT setup. This runtime uses its
        // own scheduler/idle policy rather than the SDK's startup task.
        extern "C" {
            fn pp_soft_wdt_stop();
        }
        pp_soft_wdt_stop();

        task::mark_scheduler_started();
        _xt_isr_unmask(1 << ETS_SOFT_INUM);
    }
}

/// Register the caller's permanent application stack and start preemption.
///
/// # Safety
/// Called once after allocator/newlib setup, from ordinary task context. The
/// captured stack must remain valid and excluded from the allocator forever.
pub(crate) unsafe fn start_compat_scheduler() -> Result<(), ()> {
    unsafe {
        prepare_compat_scheduler()?;
        activate_prepared_scheduler();
        Ok(())
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
        if crate::wifi::sdk::wifi_init::is_initialized() {
            esp_task_wdt_reset();
            esp_sleep_start();
        } else {
            // The explicit runtime also supports executor/timers before Wi-Fi.
            // Only the idle task sleeps directly; application tasks block.
            asm!("waiti 0", options(nomem, nostack));
        }
    }
}
