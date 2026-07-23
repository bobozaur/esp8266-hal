//! WiFi initialization and TX path.
//!
//! Replaces `sdk_overrides/esp_wifi.c`. Provides `esp_wifi_init`,
//! `esp_wifi_deinit`, and `esp_wifi_internal_tx` which are called by
//! both Rust code and the WiFi blob.

use core::ffi::c_void;
use esp8266_wifi_sys as sys;
use portable_atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const EP_OFFSET: usize = 36;
const TX_FALLBACK_PAYLOAD_BYTES: usize = 256;
const TX_FALLBACK_BYTES: usize = TX_FALLBACK_PAYLOAD_BYTES + EP_OFFSET;

#[repr(align(4))]
struct TxFallbackBuffer {
    bytes: [u8; TX_FALLBACK_BYTES],
}

static TX_FALLBACK_IN_USE: AtomicU32 = AtomicU32::new(0);
static mut TX_FALLBACK_BUF: TxFallbackBuffer = TxFallbackBuffer {
    bytes: [0; TX_FALLBACK_BYTES],
};

/// Default PPT task stack size (from SDK config).
#[no_mangle]
pub static _g_esp_wifi_ppt_task_stk_size: usize = 2816;

/// Whether to connect to open routers when a password is set.
#[no_mangle]
pub static _g_esp_wifi_connect_open_router_when_pwd_is_set: bool = false;

/// RX pbuf memory type (0 = DRAM, 1 = IRAM).
static mut RX_PBUF_MEM_TYPE: u8 = 0;

// ---------------------------------------------------------------------------
// Blob function imports
// ---------------------------------------------------------------------------

extern "C" {
    fn mac_init() -> i32;
    fn esp_wifi_init_internal(config: *const sys::wifi_init_config_t) -> i32;
    fn esp_wifi_deinit_internal() -> i32;
    fn esp_supplicant_init() -> i32;
    fn esp_supplicant_deinit();
    fn esp_wifi_internal_set_log_level(level: u32);
    fn esp_wifi_internal_set_log_mod(module: u32);

    // TX path
    fn ieee80211_output_pbuf(aio: *mut EspAio) -> i32;
}

// ---------------------------------------------------------------------------
// esp_aio_t (TX descriptor)
// ---------------------------------------------------------------------------

/// Async I/O descriptor used by the WiFi blob's TX path.
/// Layout must match `esp_aio_t` from `esp_aio.h` exactly.
#[repr(C)]
struct EspAio {
    fd: i32,                                      // file descriptor (interface)
    pbuf: *mut u8,                                // send/recv data pointer
    len: u32,                                     // buffer length in bytes (size_t)
    cb: unsafe extern "C" fn(*mut EspAio) -> i32, // callback function
    arg: *mut c_void,                             // user callback private data
    ret: i32,                                     // async operation result
}

/// TX completion callback — frees the allocated buffer.
unsafe extern "C" fn wifi_internal_send_cb(aio: *mut EspAio) -> i32 {
    unsafe {
        let arg = (*aio).arg;
        if !arg.is_null() {
            super::super::compat::memory::free(arg);
        }
        sys::ESP_OK
    }
}

/// TX completion callback for the emergency static buffer.
unsafe extern "C" fn wifi_internal_fallback_send_cb(_aio: *mut EspAio) -> i32 {
    TX_FALLBACK_IN_USE.store(0, Ordering::Release);
    sys::ESP_OK
}

// ---------------------------------------------------------------------------
// WiFi event base
// ---------------------------------------------------------------------------

/// The WIFI_EVENT event base. In C, `ESP_EVENT_DEFINE_BASE(WIFI_EVENT)`
/// expands to `const char* WIFI_EVENT = "WIFI_EVENT"`. The blob reads this
/// as a `const char*` pointer. We provide the same: a static pointer to a
/// NUL-terminated string.
///
/// `*const u8` doesn't implement `Sync`, so we wrap in a `#[repr(transparent)]`
/// newtype that does.
#[repr(transparent)]
pub(crate) struct EventBase(*const u8);
// SAFETY: the pointer is to a static string literal — immutable and valid for 'static.
unsafe impl Sync for EventBase {}

#[no_mangle]
pub static WIFI_EVENT: EventBase = EventBase(b"WIFI_EVENT\0".as_ptr());

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn esp_wifi_set_rx_pbuf_mem_type(mem_type: u32) {
    unsafe {
        if mem_type < 2 {
            RX_PBUF_MEM_TYPE = (mem_type & 1) as u8;
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn esp_wifi_get_rx_pbuf_mem_type() -> u32 {
    unsafe { RX_PBUF_MEM_TYPE as u32 }
}

#[no_mangle]
pub unsafe extern "C" fn esp_wifi_deinit() -> i32 {
    unsafe {
        esp_supplicant_deinit();
        esp_wifi_deinit_internal()
    }
}

/// Initialize the WiFi subsystem.
///
/// Calls the blob's internal init, limits blob log output, initializes
/// the WPA supplicant, and registers default TCPIP event handlers.
#[no_mangle]
pub unsafe extern "C" fn esp_wifi_init(config: *const sys::wifi_init_config_t) -> i32 {
    unsafe {
        let _ = mac_init();
        esp_wifi_set_rx_pbuf_mem_type(1);

        let result = esp_wifi_init_internal(config);
        if result != sys::ESP_OK {
            return result;
        }

        // Set WiFi log level to ERROR only (minimize blob output).
        esp_wifi_internal_set_log_level(1); // WIFI_LOG_ERROR
        esp_wifi_internal_set_log_mod(0); // WIFI_LOG_SUBMODULE_NULL

        let result = esp_supplicant_init();
        if result != sys::ESP_OK {
            let _ = esp_wifi_deinit_internal();
            return result;
        }

        // Do NOT call tcpip_adapter_set_default_wifi_handlers — we use embassy-net,
        // not lwIP. The default handlers call tcpip_adapter_start/up/dhcpc which
        // may interfere with the WiFi state machine without lwIP initialized.

        sys::ESP_OK
    }
}

/// Set RF option for deep sleep.
#[no_mangle]
pub unsafe extern "C" fn esp_deep_sleep_set_rf_option(option: u8) {
    extern "C" {
        fn phy_afterwake_set_rfoption(option: u8);
    }
    unsafe { phy_afterwake_set_rfoption(option) }
}

/// Maximum number of APs returned by scan.
#[no_mangle]
pub unsafe extern "C" fn esp_wifi_scan_get_ap_num_max() -> usize {
    32
}

/// Check whether to try high TX rate first (based on RSSI).
#[no_mangle]
#[link_section = ".iram1.text"]
pub unsafe extern "C" fn esp_wifi_try_rate_from_high() -> bool {
    false
}

/// Transmit a WiFi frame via the blob's internal TX path.
///
/// Allocates a DRAM buffer with EP_OFFSET headroom, copies the payload,
/// and calls `ieee80211_output_pbuf`. The buffer is freed by the send
/// completion callback.
#[no_mangle]
pub unsafe extern "C" fn esp_wifi_internal_tx(wifi_if: u32, buffer: *mut c_void, len: u16) -> i32 {
    unsafe {
        let total = len as usize + EP_OFFSET;
        let alloc = super::super::compat::memory::malloc(total);
        let mut used_fallback = false;
        let (dram_buf, cb, arg) = if alloc.is_null() {
            if total > TX_FALLBACK_BYTES {
                return sys::ESP_ERR_NO_MEM;
            }
            if TX_FALLBACK_IN_USE
                .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return -1;
            }
            used_fallback = true;
            let fallback = core::ptr::addr_of_mut!(TX_FALLBACK_BUF.bytes) as *mut u8;
            (
                fallback.add(EP_OFFSET),
                wifi_internal_fallback_send_cb as unsafe extern "C" fn(*mut EspAio) -> i32,
                core::ptr::null_mut(),
            )
        } else {
            (
                (alloc as *mut u8).add(EP_OFFSET),
                wifi_internal_send_cb as unsafe extern "C" fn(*mut EspAio) -> i32,
                alloc,
            )
        };
        core::ptr::copy_nonoverlapping(buffer as *const u8, dram_buf, len as usize);

        let mut aio = EspAio {
            fd: wifi_if as i32,
            pbuf: dram_buf,
            len: len as u32,
            cb,
            arg,
            ret: 0,
        };

        let ret = ieee80211_output_pbuf(&mut aio);
        if ret != 0 {
            if used_fallback {
                TX_FALLBACK_IN_USE.store(0, Ordering::Release);
            } else {
                super::super::compat::memory::free(aio.arg);
            }
        }
        ret
    }
}
