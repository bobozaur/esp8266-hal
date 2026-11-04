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

use esp8266_wifi_sys as sys;

use crate::wifi::compat::task;

const CPU_FREQUENCY_HZ: u32 = 80_000_000;
const PTHREAD_KEYS: usize = 3;
const EINVAL: i32 = 22;
const ENOMEM: i32 = 12;

static mut PTHREAD_KEYS_USED: u8 = 0;

static IP_EVENT_NAME: &[u8] = b"IP_EVENT\0";

/// Legacy event-base symbol referenced by the SDK's event objects.
#[no_mangle]
pub static mut IP_EVENT: *const c_char = IP_EVENT_NAME.as_ptr().cast();

#[no_mangle]
pub extern "C" fn tcpip_adapter_set_default_wifi_handlers() -> i32 {
    sys::ESP_OK
}

#[no_mangle]
pub extern "C" fn tcpip_adapter_clear_default_wifi_handlers() -> i32 {
    sys::ESP_OK
}

#[no_mangle]
pub extern "C" fn tcpip_adapter_get_ip_info(_interface: i32, _info: *mut c_void) -> i32 {
    sys::ESP_ERR_INVALID_STATE
}

#[no_mangle]
pub extern "C" fn tcpip_adapter_init() {}

// `esp_wifi_stop` calls this unconditionally, even in firmware that never
// initialized ESP-NOW. Avoid linking the optional ESP-NOW archive for that
// single no-op teardown path; feature builds use the real SDK implementation.
#[cfg(not(feature = "wifi-esp-now"))]
#[no_mangle]
pub extern "C" fn esp_now_deinit() -> sys::esp_err_t {
    sys::ESP_OK
}

// SmartConfig credentials are delivered through the Rust event path. The SDK
// ACK task assumes an IDF socket/netif stack that this driver deliberately
// does not own.
#[no_mangle]
pub extern "C" fn sc_send_ack_start(_kind: u32, _token: u8, _phone_ip: *mut u8) -> i32 {
    sys::ESP_OK
}

#[no_mangle]
pub extern "C" fn sc_send_ack_stop() {}

#[no_mangle]
pub extern "C" fn esp_log_timestamp() -> u32 {
    0
}

#[no_mangle]
pub extern "C" fn esp_log_early_timestamp() -> u32 {
    0
}

// C callers may pass format arguments after these fixed parameters. The
// Xtensa windowed ABI places them in caller-owned argument registers/stack,
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

#[no_mangle]
pub extern "C" fn esp_vfs_lwip_sockets_register() {}

#[no_mangle]
pub unsafe extern "C" fn prvGetExpectedIdleTime() -> u32 {
    unsafe { task::compat_prvGetExpectedIdleTime() }
}

#[no_mangle]
pub unsafe extern "C" fn pvTaskGetThreadLocalStoragePointer(
    handle: *mut c_void,
    index: i32,
) -> *mut c_void {
    unsafe { task::pvTaskGetThreadLocalStorageBufferPointer(handle, index) }
}

#[no_mangle]
pub unsafe extern "C" fn pthread_setcancelstate(_state: i32, old_state: *mut i32) -> i32 {
    if !old_state.is_null() {
        unsafe { old_state.write(0) };
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_key_create(key: *mut u32, _destructor: *mut c_void) -> i32 {
    if key.is_null() {
        return ENOMEM;
    }

    critical_section::with(|_| unsafe {
        for index in 0..PTHREAD_KEYS {
            let bit = 1 << index;
            if PTHREAD_KEYS_USED & bit == 0 {
                task::clear_task_tls_slot(index);
                PTHREAD_KEYS_USED |= bit;
                key.write((index + 1) as u32);
                return 0;
            }
        }
        ENOMEM
    })
}

#[no_mangle]
pub unsafe extern "C" fn pthread_key_delete(key: u32) -> i32 {
    let Some(index) = key.checked_sub(1).map(|value| value as usize) else {
        return EINVAL;
    };
    if index >= PTHREAD_KEYS {
        return EINVAL;
    }
    critical_section::with(|_| unsafe {
        let bit = 1 << index;
        if PTHREAD_KEYS_USED & bit == 0 {
            return EINVAL;
        }
        task::clear_task_tls_slot(index);
        PTHREAD_KEYS_USED &= !bit;
        0
    })
}

#[no_mangle]
pub unsafe extern "C" fn pthread_getspecific(key: u32) -> *mut c_void {
    let Some(index) = key.checked_sub(1).map(|value| value as usize) else {
        return ptr::null_mut();
    };
    if index >= PTHREAD_KEYS {
        return ptr::null_mut();
    }
    critical_section::with(|_| unsafe {
        if PTHREAD_KEYS_USED & (1 << index) == 0 {
            ptr::null_mut()
        } else {
            task::current_task_tls(index)
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn pthread_setspecific(key: u32, value: *const c_void) -> i32 {
    let Some(index) = key.checked_sub(1).map(|value| value as usize) else {
        return EINVAL;
    };
    if index >= PTHREAD_KEYS {
        return EINVAL;
    }
    critical_section::with(|_| unsafe {
        if PTHREAD_KEYS_USED & (1 << index) == 0 {
            EINVAL
        } else {
            task::set_current_task_tls(index, value.cast_mut());
            0
        }
    })
}

#[no_mangle]
pub extern "C" fn esp_pthread_init() -> i32 {
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
    const UART_FIFO: *mut u32 = 0x6000_0000 as *mut u32;
    loop {
        unsafe { UART_FIFO.write_volatile(b'!' as u32) };
        busy_wait(800_000);
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
