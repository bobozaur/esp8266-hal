//! Critical section and interrupt control primitives for the Xtensa LX106.
//!
//! Provides the `vPortEnterCritical`/`vPortExitCritical` and
//! `vPortETSIntrLock`/`vPortETSIntrUnlock` symbols expected by the WiFi blobs
//! and other compiled C code.

use core::arch::asm;
use core::sync::atomic::{compiler_fence, Ordering};

// ---------------------------------------------------------------------------
// Hardware register addresses (from eagle_soc.h)
// ---------------------------------------------------------------------------

const PERIPHS_DPORT_BASEADDR: u32 = 0x3ff0_0000;
const NMI_INT_ENABLE_REG: u32 = PERIPHS_DPORT_BASEADDR; // 0x3ff00000
const INT_ENA_WDEV: u32 = 0x3ff2_0c18;
const WDEV_TSF0_REACH_INT: u32 = 1 << 27;

// ---------------------------------------------------------------------------
// Globals required by the port layer and native SDK objects
// ---------------------------------------------------------------------------

/// Saved PS register for interrupt disable/enable pairs.
#[no_mangle]
pub static mut cpu_sr: u32 = 0;

/// Critical section nesting counter.
static mut CRITICAL_NESTING: u32 = 0;

/// Whether level-1 interrupts are currently disabled by the critical section.
static mut CLOSED_LV1_ISR: bool = false;

// `NMIIrqIsOn` is defined in the WiFi blob (wdev.o). Declare as extern.
extern "C" {
    /// NMI state flag — set by the NMI handler. When non-zero,
    /// critical section operations are skipped (NMI context is already non-maskable).
    pub static mut NMIIrqIsOn: u8;
}

// ---------------------------------------------------------------------------
// Inline helpers
// ---------------------------------------------------------------------------

/// Disable interrupts up to XCHAL_EXCM_LEVEL (3), saving PS into `cpu_sr`.
#[inline(always)]
unsafe fn disable_interrupts() {
    let saved: u32;
    unsafe {
        asm!(
            "rsil {0}, 3",
            out(reg) saved,
            options(nostack),
        );
        compiler_fence(Ordering::Acquire);
        cpu_sr = saved;
    }
}

/// Restore interrupts to the level saved in `cpu_sr`.
#[inline(always)]
unsafe fn enable_interrupts() {
    unsafe {
        compiler_fence(Ordering::Release);
        let val = cpu_sr;
        asm!(
            "wsr {0}, ps",
            "rsync",
            in(reg) val,
            options(nostack),
        );
    }
}

/// Read a 32-bit MMIO register.
#[inline(always)]
unsafe fn reg_read(addr: u32) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

/// Write a 32-bit MMIO register.
#[inline(always)]
unsafe fn reg_write(addr: u32, val: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, val) }
}

// ---------------------------------------------------------------------------
// Critical sections
// ---------------------------------------------------------------------------

/// Enter a critical section (disable interrupts, track nesting).
///
/// Called by C code, the heap, esp_event, etc.
/// Must be in IRAM — called from ISR paths and during flash operations.
#[no_mangle]
#[link_section = ".iram1"]
pub unsafe extern "C" fn vPortEnterCritical() {
    unsafe {
        if NMIIrqIsOn == 0 {
            if !CLOSED_LV1_ISR {
                disable_interrupts();
                CLOSED_LV1_ISR = true;
            }
            CRITICAL_NESTING += 1;
        }
        compiler_fence(Ordering::Acquire);
    }
}

/// Exit a critical section (restore interrupts when nesting reaches zero).
/// Must be in IRAM — called from ISR paths and during flash operations.
#[no_mangle]
#[link_section = ".iram1"]
pub unsafe extern "C" fn vPortExitCritical() {
    unsafe {
        compiler_fence(Ordering::Release);
        if NMIIrqIsOn == 0 {
            if CRITICAL_NESTING > 0 {
                CRITICAL_NESTING -= 1;
                if CRITICAL_NESTING == 0 && CLOSED_LV1_ISR {
                    CLOSED_LV1_ISR = false;
                    enable_interrupts();
                }
            }
        }
    }
}

/// Whether WDEV registers are safe to access (set after esp_wifi_init).
static mut WDEV_READY: bool = false;

/// Mark WDEV hardware as accessible. Call before `esp_wifi_init`.
pub unsafe fn mark_wdev_ready() {
    unsafe { WDEV_READY = true };
}

/// Mark WDEV hardware as unavailable after Wi-Fi teardown.
pub unsafe fn clear_wdev_ready() {
    unsafe { WDEV_READY = false };
}

/// Lock WiFi device interrupts (SDK-compatible).
///
/// Matches ESP8266_RTOS_SDK `port.c`: save NMI enable, enter the CPU critical
/// section, temporarily mask NMI while narrowing WDEV interest to TSF0, then
/// restore the saved NMI state before returning.
#[no_mangle]
#[link_section = ".iram1"]
pub unsafe extern "C" fn vPortETSIntrLock() {
    unsafe {
        if NMIIrqIsOn == 0 && WDEV_READY {
            let regval = reg_read(NMI_INT_ENABLE_REG);
            vPortEnterCritical();
            reg_write(NMI_INT_ENABLE_REG, 0);
            loop {
                reg_write(INT_ENA_WDEV, WDEV_TSF0_REACH_INT);
                if reg_read(INT_ENA_WDEV) == WDEV_TSF0_REACH_INT {
                    break;
                }
            }
            reg_write(NMI_INT_ENABLE_REG, regval);
        }
    }
}

/// Unlock WiFi device interrupts (SDK-compatible).
///
/// Matches ESP8266_RTOS_SDK `port.c`: temporarily mask NMI, restore WDEV
/// interest, then restore the saved NMI state before leaving the CPU critical
/// section.
#[no_mangle]
#[link_section = ".iram1"]
pub unsafe extern "C" fn vPortETSIntrUnlock() {
    unsafe {
        if NMIIrqIsOn == 0 && WDEV_READY {
            extern "C" {
                static WDEV_INTEREST_EVENT: u32;
            }
            let regval = reg_read(NMI_INT_ENABLE_REG);
            reg_write(NMI_INT_ENABLE_REG, 0);
            reg_write(INT_ENA_WDEV, WDEV_INTEREST_EVENT);
            reg_write(NMI_INT_ENABLE_REG, regval);
            vPortExitCritical();
        }
    }
}

// ---------------------------------------------------------------------------
// Interrupt query
// ---------------------------------------------------------------------------

/// Check whether CPU interrupts are currently disabled.
#[no_mangle]
pub unsafe extern "C" fn interrupt_is_disable() -> bool {
    let ps: u32;
    unsafe {
        asm!(
            "rsr {0}, ps",
            out(reg) ps,
            options(nomem, nostack),
        );
    }
    (ps & 0xF) != 0
}
