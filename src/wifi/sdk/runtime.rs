//! Narrow Rust ABI surface used by the vendored SDK objects.
//!
//! These functions intentionally contain no policy beyond adapting the IDF
//! runtime assumptions to this crate's caller-owned, bare-metal scheduler and
//! allocator.

use core::{
    ffi::{c_char, c_void},
    ptr,
    sync::atomic::{compiler_fence, Ordering},
};

#[cfg(not(feature = "wifi-esp-now"))]
use esp8266_wifi_sys as sys;

const CPU_FREQUENCY_HZ: u32 = 80_000_000;

// `esp_wifi_stop` calls this unconditionally, even in firmware that never
// initialized ESP-NOW. Avoid linking the optional ESP-NOW archive for that
// single no-op teardown path; feature builds use the real SDK implementation.
#[cfg(not(feature = "wifi-esp-now"))]
#[no_mangle]
pub extern "C" fn esp_now_deinit() -> sys::esp_err_t {
    sys::ESP_OK
}

// Native PM uses these hooks to inhibit SDK CPU light sleep. This runtime only
// executes WAITI; radio modem sleep still uses the native PM/RTC implementation.
#[no_mangle]
pub extern "C" fn esp_sleep_lock() {}

#[no_mangle]
pub extern "C" fn esp_sleep_unlock() {}

// These are intentionally empty in the pinned SDK's task_wdt.c as well. The
// hardware watchdog is fed by the Rust idle task, independently of these hooks.
#[no_mangle]
pub extern "C" fn pp_soft_wdt_stop() {}

#[no_mangle]
pub extern "C" fn pp_soft_wdt_restart() {}

#[no_mangle]
pub extern "C" fn esp_log_timestamp() -> u32 {
    0
}

#[no_mangle]
pub extern "C" fn esp_log_early_timestamp() -> u32 {
    0
}

// C callers may pass format arguments after these fixed parameters. The
// Xtensa call0 ABI places them in caller-owned argument registers/stack,
// so ignoring them is ABI-compatible and avoids unstable Rust C variadics.
#[no_mangle]
pub extern "C" fn esp_log_write(_level: u32, _tag: *const c_char, _format: *const c_char) {}

#[no_mangle]
pub extern "C" fn esp_early_log_write(_level: u32, _tag: *const c_char, _format: *const c_char) {}

#[no_mangle]
pub extern "C" fn esp_log_level_set(_tag: *const c_char, _level: u32) {}

#[no_mangle]
pub extern "C" fn esp_log_set_putchar(_function: *mut c_void) -> *mut c_void {
    ptr::null_mut()
}

/// Suppress SDK printf calls on timing-sensitive blob task stacks.
#[no_mangle]
pub extern "C" fn ets_printf(_format: *const c_char) -> i32 {
    0
}

// Newlib stdio retains this hook; the compatibility runtime has no POSIX task
// cancellation, so report the unchanged default cancellation state.
#[no_mangle]
pub unsafe extern "C" fn pthread_setcancelstate(_state: i32, old_state: *mut i32) -> i32 {
    if !old_state.is_null() {
        unsafe { old_state.write(0) };
    }
    0
}

#[no_mangle]
pub extern "C" fn xtbsp_clock_freq_hz() -> u32 {
    CPU_FREQUENCY_HZ
}

#[inline(never)]
fn busy_wait(iterations: u32) {
    for _ in 0..iterations {
        compiler_fence(Ordering::SeqCst);
        core::hint::spin_loop();
    }
}

/// The IDF PM implementation polls RTC wake bits that are not configured by
/// this bare-metal boot path. A bounded settling delay preserves its caller's
/// timing contract without risking a permanent hang.
#[export_name = "__esp8266_hal_pm_wait4wakeup"]
pub extern "C" fn pm_wait4wakeup(_mode: i32) {
    busy_wait(800_000);
}

#[no_mangle]
pub extern "C" fn _esp_error_check_failed(
    _result: i32,
    _file: *const c_char,
    _line: i32,
    _function: *const c_char,
    _expression: *const c_char,
) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

#[no_mangle]
pub extern "C" fn panicHandler(_frame: *mut c_void, _watchdog: i32) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

// The pinned GCC runtime emits these helpers for SDK volatile accesses. The
// chip is single-core, but interrupt exclusion is still required for atomicity
// relative to ISR code.
#[no_mangle]
pub unsafe extern "C" fn __atomic_load_4(pointer: *const u32, _ordering: i32) -> u32 {
    critical_section::with(|_| unsafe { pointer.read_volatile() })
}

#[no_mangle]
pub unsafe extern "C" fn __atomic_store_1(pointer: *mut u8, value: u8, _ordering: i32) {
    critical_section::with(|_| unsafe { pointer.write_volatile(value) });
}

#[no_mangle]
pub unsafe extern "C" fn __atomic_store_4(pointer: *mut u32, value: u32, _ordering: i32) {
    critical_section::with(|_| unsafe { pointer.write_volatile(value) });
}
