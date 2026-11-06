//! Narrow Rust ABI surface used by the vendored SDK objects.
//!
//! These functions intentionally contain no policy beyond adapting the IDF
//! runtime assumptions to this crate's caller-owned, bare-metal scheduler and
//! allocator.

use core::{
    ffi::{c_char, c_void},
    sync::atomic::{compiler_fence, Ordering},
};

#[cfg(not(feature = "wifi-esp-now"))]
use esp8266_wifi_sys as sys;

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
pub extern "C" fn esp_log_early_timestamp() -> u32 {
    0
}

// C callers may pass format arguments after these fixed parameters. The
// Xtensa call0 ABI places them in caller-owned argument registers/stack,
// so ignoring them is ABI-compatible and avoids unstable Rust C variadics.
#[no_mangle]
pub extern "C" fn esp_log_write(_level: u32, _tag: *const c_char, _format: *const c_char) {}

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
pub extern "C" fn panicHandler(_frame: *mut c_void, _watchdog: i32) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
