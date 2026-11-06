//! WiFi initialization and TX path.
//!
//! Replaces `sdk_overrides/esp_wifi.c`. Provides `esp_wifi_init`,
//! `esp_wifi_deinit`, and `esp_wifi_internal_tx` which are called by
//! both Rust code and the WiFi blob.

use super::super::compat::{memory, queue, task};
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
static STATE: crate::wifi::sdk_state::SdkState = crate::wifi::sdk_state::SdkState::new();

// Native deinit retains its one-time attach flag and radio allocations. A
// later init must preserve every dimension used to size those allocations.
static mut FIRST_BUFFER_LAYOUT: Option<memory::InitBufferLayout> = None;

fn buffer_layout(config: &sys::wifi_init_config_t) -> memory::InitBufferLayout {
    memory::InitBufferLayout {
        ampdu_buffers: config.rx_ampdu_buf_num as usize,
        ampdu_buffer_len: config.rx_ampdu_buf_len as usize,
        max_packet_len: config.rx_max_single_pkt_len as usize,
        rx_buffer_len: config.rx_buf_len as usize,
        rx_buffers: config.rx_buf_num as usize,
        rx_packets: config.rx_pkt_num as usize,
        tx_buffers: config.tx_buf_num as usize,
        continuous_rx_buffers: config.left_continuous_rx_buf_num as usize,
    }
}

pub(crate) fn buffer_configuration_matches(config: &sys::wifi_init_config_t) -> bool {
    let requested = buffer_layout(config);
    critical_section::with(|_| unsafe {
        FIRST_BUFFER_LAYOUT.map_or(true, |original| original == requested)
    })
}

pub(crate) fn is_initialized() -> bool {
    STATE.is_initialized()
}
pub(crate) fn requires_quarantine() -> bool {
    STATE.requires_quarantine()
}

unsafe fn cleanup_native() -> i32 {
    let result = unsafe { esp_wifi_deinit_internal() };
    let complete = result == sys::ESP_OK || result == sys::ESP_ERR_WIFI_NOT_INIT;
    STATE.cleanup(complete);
    if complete {
        unsafe { super::phy_init::release_workspace() };
        sys::ESP_OK
    } else {
        result
    }
}
static mut TX_FALLBACK_BUF: TxFallbackBuffer = TxFallbackBuffer {
    bytes: [0; TX_FALLBACK_BYTES],
};

/// Default PPT task stack size (from SDK config).
#[no_mangle]
pub static _g_esp_wifi_ppt_task_stk_size: usize = task::PP_TASK_STACK_BYTES as usize;

/// Whether to connect to open routers when a password is set.
#[no_mangle]
pub static _g_esp_wifi_connect_open_router_when_pwd_is_set: bool = false;

// ---------------------------------------------------------------------------
// Blob function imports
// ---------------------------------------------------------------------------

extern "C" {
    fn mac_init() -> i32;
    fn esp_wifi_set_rx_pbuf_mem_type(mem_type: u32);
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
// Public API
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn esp_wifi_deinit() -> i32 {
    unsafe {
        let supplicant_ready = STATE.is_initialized();
        STATE.begin();
        if supplicant_ready {
            esp_supplicant_deinit();
        }
        cleanup_native()
    }
}

/// Initialize the WiFi subsystem.
///
/// Calls the blob's internal init, limits blob log output, initializes
/// the WPA supplicant, and leaves IP-stack policy to the application.
#[no_mangle]
pub unsafe extern "C" fn esp_wifi_init(config: *const sys::wifi_init_config_t) -> i32 {
    unsafe {
        if STATE.requires_quarantine() {
            return sys::ESP_ERR_INVALID_STATE;
        }
        let Some(config) = config.as_ref() else {
            return sys::ESP_ERR_INVALID_ARG;
        };
        if !buffer_configuration_matches(config) {
            return sys::ESP_ERR_INVALID_ARG;
        }
        let first_attach = critical_section::with(|_| {
            let original = FIRST_BUFFER_LAYOUT;
            original.is_none()
        });
        if first_attach && !task::has_free_task_slot() {
            return sys::ESP_ERR_NO_MEM;
        }
        let layout = buffer_layout(config);
        let Some(sizes) = layout.allocation_sizes(
            first_attach,
            queue::allocation_bytes(),
            task::stack_allocation_bytes(task::PP_TASK_STACK_BYTES),
        ) else {
            return sys::ESP_ERR_INVALID_ARG;
        };
        if !memory::reserve_init_allocations(sizes) {
            return sys::ESP_ERR_NO_MEM;
        }
        // Native PHY initialization is part of the permanent first attach;
        // recreation must not reserve an unused calibration workspace.
        if first_attach {
            let prepared = super::phy_init::prepare_workspace();
            if prepared != sys::ESP_OK {
                memory::release_init_allocations();
                return prepared;
            }
        }
        // Native code sets its permanent first-attach flag before attaching.
        // Even a subsequent supplicant error cannot unlock these dimensions.
        if first_attach {
            critical_section::with(|_| FIRST_BUFFER_LAYOUT = Some(layout));
        }
        STATE.begin();
        let _ = mac_init();
        esp_wifi_set_rx_pbuf_mem_type(1);

        let native_result = esp_wifi_init_internal(config);
        memory::release_init_allocations();
        let phy_result = super::phy_init::initialization_result();
        let result = if native_result != sys::ESP_OK {
            native_result
        } else {
            phy_result
        };
        if result != sys::ESP_OK {
            let _ = cleanup_native();
            return result;
        }

        // Set WiFi log level to ERROR only (minimize blob output).
        esp_wifi_internal_set_log_level(1); // WIFI_LOG_ERROR
        esp_wifi_internal_set_log_mod(0); // WIFI_LOG_SUBMODULE_NULL

        let result = esp_supplicant_init();
        if result != sys::ESP_OK {
            let _ = cleanup_native();
            return result;
        }

        // The application owns its IP stack; SDK default TCPIP handlers would
        // start lwIP interfaces and DHCP behind that stack's back.
        super::phy_init::release_workspace();
        STATE.ready();
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
/// Allocates DRAM with EP_OFFSET headroom and initialized native lookahead,
/// copies the logical payload, and calls `ieee80211_output_pbuf` without
/// changing its length. The send completion callback releases the buffer.
#[no_mangle]
pub unsafe extern "C" fn esp_wifi_internal_tx(wifi_if: u32, buffer: *mut c_void, len: u16) -> i32 {
    unsafe {
        let Some(payload_capacity) = crate::wifi::tx_buffer::payload_capacity(len as usize) else {
            return sys::ESP_ERR_INVALID_ARG;
        };
        let total = payload_capacity + EP_OFFSET;
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
        crate::wifi::tx_buffer::copy_frame(buffer.cast(), dram_buf, len as usize);

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
