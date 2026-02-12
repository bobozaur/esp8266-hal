//! WiFi driver for the ESP8266, built on the RTOS SDK.
//!
//! This module wraps the ESP8266 RTOS SDK WiFi C APIs with safe Rust
//! abstractions modeled after the `esp-wifi` crate for ESP32.
//!
//! # Architecture (Embassy-first)
//!
//! The embassy executor owns the main thread. The FreeRTOS compat scheduler
//! runs in the background (via tick ISR + PendSV) to service WiFi blob tasks.
//!
//! # Quick start
//!
//! ```rust,no_run
//! use esp8266_hal::wifi::{WifiController, WifiMode, StaConfig};
//! use esp8266_hal::wifi::controller::WifiInitConfig;
//!
//! let mut ctrl = WifiController::init(WifiInitConfig::default())
//!     .expect("WiFi init failed");
//!
//! ctrl.set_mode(WifiMode::Sta).expect("set mode");
//! ctrl.set_sta_config(
//!     &StaConfig::builder("MyNetwork", "hunter2").build()
//! ).expect("set config");
//!
//! ctrl.start().expect("start");
//! ```

// FFI compat layer: all `extern "C"` functions called from C/blobs, not Rust API.
#[allow(clippy::missing_safety_doc, clippy::needless_range_loop)]
pub mod compat;
pub mod phy;
pub mod timer_driver;

#[cfg(feature = "wifi-ap")]
pub mod ap;
pub mod config;
pub mod controller;
pub mod device;
pub mod error;
#[cfg(feature = "wifi-esp-now")]
pub mod esp_now;
pub mod event;
pub mod ip;
#[cfg(feature = "wifi-promiscuous")]
pub mod promiscuous;
pub mod scan;
#[cfg(feature = "wifi-smartconfig")]
pub mod smartconfig;
#[cfg(feature = "wifi-sta")]
pub mod sta;
pub mod stack;
#[cfg(feature = "wifi-wps")]
pub mod wps;

pub mod executor;
pub mod sdk;

#[cfg(feature = "wifi-embassy-net")]
pub mod embassy_net;

pub use config::{ApConfig, Configuration, StaConfig};
pub use controller::{StaConnectionState, WifiController};
pub use device::WifiDevice;
pub use error::{EspError, WifiError};
pub use event::{DisconnectReason, EventSubscriber, WifiEvent, WifiEventKind};
pub use ip::{Ipv4Addr, Ipv4Info};
pub use scan::{AccessPointInfo, ScanConfig};

/// Initialize the FreeRTOS compat scheduler in background mode.
///
/// This is called automatically by [`WifiController::init`], but can be
/// called manually if you need the compat layer before WiFi initialization.
///
/// Safe to call multiple times (subsequent calls are no-ops).
pub fn init_compat_scheduler() {
    unsafe { compat::entry::init_compat_scheduler() };
}

/// WiFi operating mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiMode {
    /// Station mode — connects to an external access point.
    Sta,
    /// Soft-AP mode — acts as an access point for other stations.
    Ap,
    /// Station + Soft-AP mode.
    ApSta,
}

impl WifiMode {
    pub(crate) fn to_raw(self) -> esp8266_wifi_sys::wifi_mode_t {
        match self {
            Self::Sta => esp8266_wifi_sys::WIFI_MODE_STA,
            Self::Ap => esp8266_wifi_sys::WIFI_MODE_AP,
            Self::ApSta => esp8266_wifi_sys::WIFI_MODE_APSTA,
        }
    }

    pub(crate) fn from_raw(raw: esp8266_wifi_sys::wifi_mode_t) -> Option<Self> {
        match raw {
            esp8266_wifi_sys::WIFI_MODE_STA => Some(Self::Sta),
            esp8266_wifi_sys::WIFI_MODE_AP => Some(Self::Ap),
            esp8266_wifi_sys::WIFI_MODE_APSTA => Some(Self::ApSta),
            _ => None,
        }
    }

    /// Returns `true` if this mode includes station functionality.
    pub fn is_sta(self) -> bool {
        matches!(self, Self::Sta | Self::ApSta)
    }

    /// Returns `true` if this mode includes soft-AP functionality.
    pub fn is_ap(self) -> bool {
        matches!(self, Self::Ap | Self::ApSta)
    }
}

/// Authentication mode for WiFi connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AuthMode {
    Open,
    Wep,
    WpaPsk,
    Wpa2Psk,
    WpaWpa2Psk,
    Wpa2Enterprise,
    Wpa3Psk,
    Wpa2Wpa3Psk,
}

impl AuthMode {
    pub(crate) fn to_raw(self) -> esp8266_wifi_sys::wifi_auth_mode_t {
        match self {
            Self::Open => esp8266_wifi_sys::WIFI_AUTH_OPEN,
            Self::Wep => esp8266_wifi_sys::WIFI_AUTH_WEP,
            Self::WpaPsk => esp8266_wifi_sys::WIFI_AUTH_WPA_PSK,
            Self::Wpa2Psk => esp8266_wifi_sys::WIFI_AUTH_WPA2_PSK,
            Self::WpaWpa2Psk => esp8266_wifi_sys::WIFI_AUTH_WPA_WPA2_PSK,
            Self::Wpa2Enterprise => esp8266_wifi_sys::WIFI_AUTH_WPA2_ENTERPRISE,
            Self::Wpa3Psk => esp8266_wifi_sys::WIFI_AUTH_WPA3_PSK,
            Self::Wpa2Wpa3Psk => esp8266_wifi_sys::WIFI_AUTH_WPA2_WPA3_PSK,
        }
    }

    pub(crate) fn from_raw(raw: esp8266_wifi_sys::wifi_auth_mode_t) -> Self {
        match raw {
            esp8266_wifi_sys::WIFI_AUTH_OPEN => Self::Open,
            esp8266_wifi_sys::WIFI_AUTH_WEP => Self::Wep,
            esp8266_wifi_sys::WIFI_AUTH_WPA_PSK => Self::WpaPsk,
            esp8266_wifi_sys::WIFI_AUTH_WPA2_PSK => Self::Wpa2Psk,
            esp8266_wifi_sys::WIFI_AUTH_WPA_WPA2_PSK => Self::WpaWpa2Psk,
            esp8266_wifi_sys::WIFI_AUTH_WPA2_ENTERPRISE => Self::Wpa2Enterprise,
            esp8266_wifi_sys::WIFI_AUTH_WPA3_PSK => Self::Wpa3Psk,
            esp8266_wifi_sys::WIFI_AUTH_WPA2_WPA3_PSK => Self::Wpa2Wpa3Psk,
            _ => Self::Open,
        }
    }
}

/// WiFi protocol flags.
pub mod protocol {
    bitflags::bitflags! {
        /// 802.11 protocol support flags.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct Protocol: u8 {
            /// 802.11b (DSSS, 1-11 Mbps).
            const B = esp8266_wifi_sys::WIFI_PROTOCOL_11B as u8;
            /// 802.11g (OFDM, 6-54 Mbps).
            const G = esp8266_wifi_sys::WIFI_PROTOCOL_11G as u8;
            /// 802.11n (HT, up to 72.2 Mbps on ESP8266).
            const N = esp8266_wifi_sys::WIFI_PROTOCOL_11N as u8;
            /// Espressif Long Range mode.
            const LR = esp8266_wifi_sys::WIFI_PROTOCOL_LR as u8;
        }
    }
}

/// Channel bandwidth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bandwidth {
    /// 20 MHz channel.
    Ht20,
    /// 40 MHz channel (requires 802.11n).
    Ht40,
}

impl Bandwidth {
    pub(crate) fn to_raw(self) -> esp8266_wifi_sys::wifi_bandwidth_t {
        match self {
            Self::Ht20 => esp8266_wifi_sys::WIFI_BW_HT20,
            Self::Ht40 => esp8266_wifi_sys::WIFI_BW_HT40,
        }
    }

    pub(crate) fn from_raw(raw: esp8266_wifi_sys::wifi_bandwidth_t) -> Self {
        match raw {
            esp8266_wifi_sys::WIFI_BW_HT40 => Self::Ht40,
            _ => Self::Ht20,
        }
    }
}

/// Power-save mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerSave {
    /// No power save — maximum performance.
    None,
    /// Minimum modem sleep — wake at every DTIM beacon.
    MinModem,
    /// Maximum modem sleep — wake interval controlled by `listen_interval`.
    MaxModem,
}

impl PowerSave {
    pub(crate) fn to_raw(self) -> esp8266_wifi_sys::wifi_ps_type_t {
        match self {
            Self::None => esp8266_wifi_sys::WIFI_PS_NONE,
            Self::MinModem => esp8266_wifi_sys::WIFI_PS_MIN_MODEM,
            Self::MaxModem => esp8266_wifi_sys::WIFI_PS_MAX_MODEM,
        }
    }

    pub(crate) fn from_raw(raw: esp8266_wifi_sys::wifi_ps_type_t) -> Self {
        match raw {
            esp8266_wifi_sys::WIFI_PS_MIN_MODEM => Self::MinModem,
            esp8266_wifi_sys::WIFI_PS_MAX_MODEM => Self::MaxModem,
            _ => Self::None,
        }
    }
}

/// Secondary channel position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecondaryChannel {
    None,
    Above,
    Below,
}

impl SecondaryChannel {
    pub(crate) fn to_raw(self) -> esp8266_wifi_sys::wifi_second_chan_t {
        match self {
            Self::None => esp8266_wifi_sys::WIFI_SECOND_CHAN_NONE,
            Self::Above => esp8266_wifi_sys::WIFI_SECOND_CHAN_ABOVE,
            Self::Below => esp8266_wifi_sys::WIFI_SECOND_CHAN_BELOW,
        }
    }

    pub(crate) fn from_raw(raw: esp8266_wifi_sys::wifi_second_chan_t) -> Self {
        match raw {
            esp8266_wifi_sys::WIFI_SECOND_CHAN_ABOVE => Self::Above,
            esp8266_wifi_sys::WIFI_SECOND_CHAN_BELOW => Self::Below,
            _ => Self::None,
        }
    }
}

/// WiFi configuration storage destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Storage {
    /// Persist to NVS flash and RAM.
    Flash,
    /// RAM only — lost on reboot.
    Ram,
}

impl Storage {
    pub(crate) fn to_raw(self) -> esp8266_wifi_sys::wifi_storage_t {
        match self {
            Self::Flash => esp8266_wifi_sys::WIFI_STORAGE_FLASH,
            Self::Ram => esp8266_wifi_sys::WIFI_STORAGE_RAM,
        }
    }
}

/// Extract a null-terminated or length-bounded string from a byte buffer.
pub(crate) fn bytes_to_str(buf: &[u8]) -> &[u8] {
    match buf.iter().position(|&b| b == 0) {
        Some(pos) => &buf[..pos],
        None => buf,
    }
}
