//! Raw FFI bindings to the ESP8266 RTOS SDK WiFi ABI.
//!
//! The generated bindings and native libraries are checked in so normal builds
//! are deterministic and offline. See `vendor/README.md` for their exact SDK
//! revision, provenance, licenses, and checksums. A small number of types and
//! constants that bindgen cannot handle cleanly (bitfield structs and
//! `#define` constants) are defined manually below the generated module.
//!
//! # Safety
//!
//! All functions in this crate are `unsafe extern "C"` and must only be called
//! from firmware linked against the ESP8266 RTOS SDK libraries. The caller is
//! responsible for satisfying every precondition documented in the SDK headers.

#![no_std]
#![allow(
    non_camel_case_types,
    non_upper_case_globals,
    non_snake_case,
    dead_code,
    clippy::upper_case_acronyms,
    clippy::missing_safety_doc,
    clippy::useless_transmute
)]

// Generated once from the pinned headers; never regenerated as a side effect
// of compiling an application.
mod bindings;
pub use bindings::*;

// ---------------------------------------------------------------------------
// Manual constants that bindgen may not emit (they originate from #define
// macros, not C declarations).
// ---------------------------------------------------------------------------

/// Magic number expected by `esp_wifi_init` in `wifi_init_config_t::magic`.
pub const WIFI_INIT_CONFIG_MAGIC: u32 = 0x1F2F_3F4F;

/// WiFi interface aliases (C: `#define WIFI_IF_STA ESP_IF_WIFI_STA`).
pub const WIFI_IF_STA: esp_interface_t = ESP_IF_WIFI_STA;
pub const WIFI_IF_AP: esp_interface_t = ESP_IF_WIFI_AP;

// Bindgen emits ESP_OK as u32 and ESP_ERR_* as u32, but esp_err_t is i32.
// Re-export them with the correct type for ergonomic use.
// The original bindgen constants are still available with their generated names.
#[allow(dead_code)]
mod _esp_err_typed {
    use super::esp_err_t;
    pub const ESP_OK: esp_err_t = 0;
    pub const ESP_FAIL: esp_err_t = -1;
    pub const ESP_ERR_NO_MEM: esp_err_t = 0x101;
    pub const ESP_ERR_INVALID_ARG: esp_err_t = 0x102;
    pub const ESP_ERR_INVALID_STATE: esp_err_t = 0x103;
    pub const ESP_ERR_INVALID_SIZE: esp_err_t = 0x104;
    pub const ESP_ERR_NOT_FOUND: esp_err_t = 0x105;
    pub const ESP_ERR_NOT_SUPPORTED: esp_err_t = 0x106;
    pub const ESP_ERR_TIMEOUT: esp_err_t = 0x107;
    pub const ESP_ERR_WIFI_BASE: esp_err_t = 0x3000;
    pub const ESP_ERR_WIFI_NOT_INIT: esp_err_t = 0x3001;
    pub const ESP_ERR_WIFI_NOT_STARTED: esp_err_t = 0x3002;
    pub const ESP_ERR_WIFI_NOT_STOPPED: esp_err_t = 0x3003;
    pub const ESP_ERR_WIFI_IF: esp_err_t = 0x3004;
    pub const ESP_ERR_WIFI_MODE: esp_err_t = 0x3005;
    pub const ESP_ERR_WIFI_STATE: esp_err_t = 0x3006;
    pub const ESP_ERR_WIFI_CONN: esp_err_t = 0x3007;
    pub const ESP_ERR_WIFI_NVS: esp_err_t = 0x3008;
    pub const ESP_ERR_WIFI_MAC: esp_err_t = 0x3009;
    pub const ESP_ERR_WIFI_SSID: esp_err_t = 0x300A;
    pub const ESP_ERR_WIFI_PASSWORD: esp_err_t = 0x300B;
    pub const ESP_ERR_WIFI_TIMEOUT: esp_err_t = 0x300C;
    pub const ESP_ERR_WIFI_WAKE_FAIL: esp_err_t = 0x300D;
    pub const ESP_ERR_WIFI_WOULD_BLOCK: esp_err_t = 0x300E;
    pub const ESP_ERR_WIFI_NOT_CONNECT: esp_err_t = 0x300F;
}
pub use _esp_err_typed::*;

/// Wildcard event ID for `esp_event_handler_register` (matches all events
/// under a given base).
pub const ESP_EVENT_ANY_ID: i32 = -1;

/// FreeRTOS maximum delay (`portMAX_DELAY`).
pub const PORT_MAX_DELAY: u32 = 0xFFFF_FFFF;

// ---------------------------------------------------------------------------
// Manual type definitions — bitfield structs that bindgen cannot
// reliably lay out. These match the C definitions in esp_wifi_types.h.
// ---------------------------------------------------------------------------

/// Promiscuous mode RX control metadata (12 bytes, four-byte C alignment).
///
/// Access individual fields through the provided accessor methods rather than
/// direct bit manipulation.
///
/// C layout (bits, LSB first within each byte):
/// ```text
///  [0]      rssi            (8)   — signed
///  [1]      rate            (4), is_group (1), reserved (1), sig_mode (2)
///  [2..3]   legacy_length   (12), damatch0 (1), damatch1 (1), bssidmatch0 (1), bssidmatch1 (1)
///  [4]      mcs             (7), cwb (1)
///  [5..6]   HT_length       (16)
///  [7]      smoothing (1), not_sounding (1), reserved (1), aggregation (1),
///           stbc (2), fec_coding (1), sgi (1)
///  [8]      rxend_state     (8)
///  [9]      ampdu_cnt       (8)
///  [10]     channel         (4), reserved (4)
///  [11]     noise_floor     (8) — signed
/// ```
// The SDK uses unsigned/signed int bitfields, whose target alignment is four.
#[repr(C, align(4))]
#[derive(Debug, Copy, Clone)]
pub struct wifi_pkt_rx_ctrl_t {
    pub _data: [u8; 12],
}

impl wifi_pkt_rx_ctrl_t {
    /// Signal strength in dBm (signed).
    #[inline]
    pub fn rssi(&self) -> i8 {
        self._data[0] as i8
    }

    /// Data rate index.
    #[inline]
    pub fn rate(&self) -> u8 {
        self._data[1] & 0x0F
    }

    /// `true` if the destination is a group address.
    #[inline]
    pub fn is_group(&self) -> bool {
        (self._data[1] >> 4) & 1 != 0
    }

    /// Signal mode: 0 = legacy (11b/g), non-zero = HT (11n).
    #[inline]
    pub fn sig_mode(&self) -> u8 {
        (self._data[1] >> 6) & 0x03
    }

    /// Legacy frame length (valid when `sig_mode() == 0`).
    #[inline]
    pub fn legacy_length(&self) -> u16 {
        let lo = self._data[2] as u16;
        let hi = (self._data[3] as u16) & 0x0F;
        (hi << 8) | lo
    }

    /// MCS index (0–76 for HT).
    #[inline]
    pub fn mcs(&self) -> u8 {
        self._data[4] & 0x7F
    }

    /// Channel bandwidth: 0 = 20 MHz, 1 = 40 MHz.
    #[inline]
    pub fn cwb(&self) -> u8 {
        (self._data[4] >> 7) & 1
    }

    /// HT frame length (valid when `sig_mode() != 0`).
    #[inline]
    pub fn ht_length(&self) -> u16 {
        u16::from_le_bytes([self._data[5], self._data[6]])
    }

    /// Aggregation flag.
    #[inline]
    pub fn aggregation(&self) -> bool {
        (self._data[7] >> 3) & 1 != 0
    }

    /// STBC field (0–3).
    #[inline]
    pub fn stbc(&self) -> u8 {
        (self._data[7] >> 4) & 0x03
    }

    /// Short guard interval flag.
    #[inline]
    pub fn sgi(&self) -> bool {
        (self._data[7] >> 7) & 1 != 0
    }

    /// RX end state (0 = no error).
    #[inline]
    pub fn rxend_state(&self) -> u8 {
        self._data[8]
    }

    /// A-MPDU sub-frame count.
    #[inline]
    pub fn ampdu_cnt(&self) -> u8 {
        self._data[9]
    }

    /// Channel number (1–14).
    #[inline]
    pub fn channel(&self) -> u8 {
        self._data[10] & 0x0F
    }

    /// Noise floor in dBm (signed).
    #[inline]
    pub fn noise_floor(&self) -> i8 {
        self._data[11] as i8
    }
}

/// Promiscuous mode received packet.
///
/// The `payload` field is a zero-length array — the actual frame data follows
/// immediately after `rx_ctrl` in memory. Use the length from `rx_ctrl`
/// (either `legacy_length()` or `ht_length()` depending on `sig_mode()`), capped
/// at the SDK's 112-byte capture limit, to determine how many bytes are available.
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct wifi_promiscuous_pkt_t {
    pub rx_ctrl: wifi_pkt_rx_ctrl_t,
    pub payload: [u8; 0],
}

/// Maximum number of stations connectable to the soft-AP.
pub const ESP_WIFI_MAX_CONN_NUM: usize = 10;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn promiscuous_rx_control_layout_matches_sdk() {
        assert_eq!(core::mem::size_of::<wifi_pkt_rx_ctrl_t>(), 12);
        assert_eq!(core::mem::align_of::<wifi_pkt_rx_ctrl_t>(), 4);
        assert_eq!(core::mem::size_of::<wifi_promiscuous_pkt_t>(), 12);
        assert_eq!(core::mem::align_of::<wifi_promiscuous_pkt_t>(), 4);
    }

    #[test]
    fn promiscuous_rx_control_accessors_decode_packed_fields() {
        let control = wifi_pkt_rx_ctrl_t {
            _data: [
                0xd6, 0xd3, 0x34, 0xa2, 0x85, 0x78, 0x56, 0xb8, 7, 3, 0x4b, 0xa1,
            ],
        };

        assert_eq!(control.rssi(), -42);
        assert_eq!(control.rate(), 3);
        assert!(control.is_group());
        assert_eq!(control.sig_mode(), 3);
        assert_eq!(control.legacy_length(), 0x234);
        assert_eq!(control.mcs(), 5);
        assert_eq!(control.cwb(), 1);
        assert_eq!(control.ht_length(), 0x5678);
        assert!(control.aggregation());
        assert_eq!(control.stbc(), 3);
        assert!(control.sgi());
        assert_eq!(control.rxend_state(), 7);
        assert_eq!(control.ampdu_cnt(), 3);
        assert_eq!(control.channel(), 11);
        assert_eq!(control.noise_floor(), -95);
    }
}
