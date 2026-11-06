//! Narrow ABI declarations and pinned native inputs for the ESP8266 driver.
//!
//! The declarations and native libraries are checked in so normal builds
//! are deterministic and offline. See `vendor/README.md` for their exact SDK
//! revision, provenance, licenses, and checksums. Declarations follow only the
//! driver's required interfaces; this crate does not expose the whole SDK.
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
    clippy::missing_safety_doc
)]

// Initially generated from the pinned headers and narrowed to driver use.
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

// Native return codes are signed, matching esp_err_t.
pub const ESP_OK: esp_err_t = 0;
pub const ESP_ERR_NO_MEM: esp_err_t = 0x101;
pub const ESP_ERR_INVALID_ARG: esp_err_t = 0x102;
pub const ESP_ERR_INVALID_STATE: esp_err_t = 0x103;
pub const ESP_ERR_INVALID_SIZE: esp_err_t = 0x104;
pub const ESP_ERR_NOT_SUPPORTED: esp_err_t = 0x106;
pub const ESP_ERR_TIMEOUT: esp_err_t = 0x107;
pub const ESP_ERR_WIFI_NOT_INIT: esp_err_t = 0x3001;
pub const ESP_ERR_WIFI_NOT_STARTED: esp_err_t = 0x3002;
pub const ESP_ERR_WIFI_TIMEOUT: esp_err_t = 0x300C;
pub const ESP_ERR_WIFI_WOULD_BLOCK: esp_err_t = 0x300E;

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
/// immediately after `rx_ctrl` in memory. Its length fields describe the original
/// frame, not the allocation. Available bytes depend on the producer's allocation
/// and configured capture prefix; aggregated packets contain only 40 frame bytes
/// followed by native metadata. Consumers must establish those bounds separately.
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
    fn narrowed_native_views_preserve_target_layouts() {
        use core::mem::{align_of, offset_of, size_of};

        // tests/abi_layout.c independently checks these values against the
        // pinned target headers. These types contain no host-sized pointers.
        assert_eq!(
            (
                size_of::<wifi_ap_record_t>(),
                align_of::<wifi_ap_record_t>()
            ),
            (80, 4)
        );
        assert_eq!(offset_of!(wifi_ap_record_t, flags), 64);
        assert_eq!(offset_of!(wifi_ap_record_t, country), 68);
        assert_eq!(
            (
                size_of::<wifi_sta_config_t>(),
                align_of::<wifi_sta_config_t>()
            ),
            (132, 4)
        );
        assert_eq!(offset_of!(wifi_sta_config_t, flags), 126);
        assert_eq!(
            (size_of::<wifi_sta_info_t>(), align_of::<wifi_sta_info_t>()),
            (12, 4)
        );
        // The station-info header omits a byte present in the retained driver.
        // vendor/README.md records the independent disassembly evidence.
        assert_eq!(offset_of!(wifi_sta_info_t, _driver_data), 6);
        assert_eq!(offset_of!(wifi_sta_info_t, flags), 7);
        assert_eq!(size_of::<wifi_sta_list_t>(), 124);
        assert_eq!(offset_of!(wifi_sta_list_t, num), 120);
        assert_eq!(
            (
                size_of::<system_event_info_t>(),
                align_of::<system_event_info_t>()
            ),
            (44, 4)
        );
        assert_eq!(size_of::<system_event_t>(), 48);
        assert_eq!(offset_of!(system_event_t, event_info), 4);
    }

    #[test]
    fn scan_and_config_flags_match_target_c_images() {
        // Images emitted by tests/abi_layout.c using the pinned Xtensa GCC.
        let mut ap_image = [0_u8; 80];
        ap_image[64] = 0x10;
        // SAFETY: the complete image contains only valid integer/enum storage.
        let mut record =
            unsafe { core::ptr::read_unaligned(ap_image.as_ptr().cast::<wifi_ap_record_t>()) };
        assert_eq!(record.wps(), 1);
        record.flags = !0x10;
        assert_eq!(record.wps(), 0);

        let mut config_image = [0_u8; 132];
        config_image[126] = 3;
        // SAFETY: all bool fields are zero and every other field is an integer.
        let config =
            unsafe { core::ptr::read_unaligned(config_image.as_ptr().cast::<wifi_sta_config_t>()) };
        assert_eq!(config.flags, [3, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn station_phy_flags_decode_retained_driver_output() {
        // libnet80211.a:ieee80211_api.o:esp_wifi_ap_get_sta_list writes the
        // extra byte at instruction 0xa5 and PHY flags at 0xd9. Its 12-byte
        // record differs from the SDK header's byte-6 bitfields.
        for flags in 0..8_u8 {
            let image = [2, 3, 4, 5, 6, 7, !flags, flags, !flags, 0, 0, 0];
            // SAFETY: all twelve bytes initialize plain integer fields.
            let station =
                unsafe { core::ptr::read_unaligned(image.as_ptr().cast::<wifi_sta_info_t>()) };
            assert_eq!(station.mac, [2, 3, 4, 5, 6, 7]);
            assert_eq!(
                (station.phy_11b(), station.phy_11g(), station.phy_11n()),
                (
                    u32::from(flags & 1),
                    u32::from((flags >> 1) & 1),
                    u32::from((flags >> 2) & 1)
                )
            );
        }
    }

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
