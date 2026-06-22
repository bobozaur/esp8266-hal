//! WiFi driver for the ESP8266, built on the RTOS SDK.
//!
//! This module now has two layers:
//!
//! - typed driver lifecycle, configuration, events, resources, and feature
//!   sessions re-exported from `crate::wifi_core`
//! - ESP8266 target integration plus higher-level adapters for `smoltcp` and
//!   `embassy-net`
//!
//! The recommended networking path is:
//!
//! 1. build a [`WifiDriver`]
//! 2. connect with a temporary station handle from `driver.sta()`
//! 3. hand ownership to networking with `driver.sta_link()`
//!
//! # Architecture (Embassy-first)
//!
//! The embassy executor owns the main thread. The FreeRTOS compat scheduler
//! runs in the background (via tick ISR + PendSV) to service WiFi blob tasks.
//!
//! RX packet ingress, link-state tracking, wake registration, and packet
//! storage all live in caller-owned [`WifiResources`].
//!
//! # Typestate quick start
//!
//! ```rust,no_run
//! use esp8266_hal::wifi::{
//!     ClientConfig, Credential, Esp8266TargetBackend, InitConfig, Ssid, WifiDriver,
//!     WifiResources,
//! };
//!
//! let resources = WifiResources::<8, 4, 2>::new();
//! let backend = Esp8266TargetBackend::new(&resources);
//! let driver = WifiDriver::with_backend(&resources, backend)
//!     .initialize(InitConfig::default())
//!     .expect("init");
//!
//! let config = ClientConfig::new(
//!     Ssid::try_from("MyNetwork").expect("ssid"),
//!     Credential::Open,
//! );
//!
//! let mut driver = driver.configure_sta(config).expect("config").start().expect("start");
//! let mut station = driver.sta().expect("station handle");
//! let _session = station.connect_blocking(15_000).expect("connect");
//! ```
//!
//! The older controller-style implementation remains as target-internal
//! compatibility plumbing while the public driver surface is the typed core
//! plus adapters in this module.

// FFI compat layer: all `extern "C"` functions called from C/blobs.
#[allow(clippy::missing_safety_doc, clippy::needless_range_loop)]
#[doc(hidden)]
pub mod compat;
pub(crate) mod timer_driver;

#[cfg(feature = "wifi-ap")]
pub(crate) mod ap;
pub(crate) mod config;
pub(crate) mod controller;
pub(crate) mod device;
pub(crate) mod error;
#[cfg(feature = "wifi-esp-now")]
pub(crate) mod esp_now;
pub(crate) mod event;
pub(crate) mod ip;
#[cfg(feature = "wifi-promiscuous")]
pub(crate) mod promiscuous;
pub(crate) mod scan;
#[cfg(feature = "wifi-smartconfig")]
pub(crate) mod smartconfig;
#[cfg(feature = "wifi-sta")]
pub(crate) mod sta;
pub(crate) mod stack;
#[cfg(feature = "wifi-wps")]
pub(crate) mod wps;

#[doc(hidden)]
pub mod executor;
pub(crate) mod sdk;

#[cfg(feature = "wifi-embassy-net")]
pub mod embassy_net;

/// `smoltcp` adapter types over owned [`wifi_core`](crate::wifi_core) links.
pub mod smoltcp {
    pub use super::device::{ApWifiDevice, LinkDevice, WifiDevice};
}

pub use crate::wifi_core::*;
pub(crate) use config::{ApConfig, StaConfig};
pub(crate) use controller::WifiController;
pub(crate) use error::{EspError, WifiError};

/// Initialize the FreeRTOS compat scheduler in background mode.
///
/// This is called automatically by the ESP8266 target backend, but can be
/// called manually if you need the compat layer before WiFi initialization.
///
/// Safe to call multiple times (subsequent calls are no-ops).
pub fn init_compat_scheduler() {
    unsafe { compat::entry::init_compat_scheduler() };
}

/// Yield the current compat task for `ticks` scheduler ticks.
///
/// This is primarily useful in synchronous examples after their terminal
/// marker has been emitted. Async code should prefer `embassy_time::Timer`.
pub fn delay_ticks(ticks: u32) {
    unsafe { compat::task::vTaskDelay(ticks) };
}

/// Return the current compat heap free-byte estimate.
///
/// This is intended for examples and diagnostics that need a lightweight
/// health marker without reaching into the target-internal compat layer.
pub fn free_heap_bytes() -> u32 {
    unsafe { compat::memory::esp_get_free_heap_size() }
}

/// Snapshot of the most recent compat allocation failure since the last WiFi
/// initialization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AllocFailureDebug {
    pub kind: u8,
    pub requested_bytes: usize,
    pub free_bytes: usize,
    pub task_marker: u8,
    pub count: u32,
    pub eap_active_count: u32,
    pub eap_reserved: bool,
    pub eap_usable_bytes: usize,
    pub eap_in_use: bool,
    pub eap_reserve_available: bool,
}

/// Snapshot of the RSA workspace reservation used during EAP diagnostics.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EapRsaWorkspaceDebug {
    pub reserved_now: bool,
    pub active_count: u32,
    pub reserve_attempts: u32,
    pub reserved: u32,
    pub used: u32,
    pub released: u32,
    pub usable_bytes: usize,
    pub in_use: bool,
}

/// Return the most recent compat allocation failure recorded since WiFi init.
///
/// This is a diagnostics-only hook for constrained hardware bring-up. A
/// returned value indicates that some SDK/blob allocation failed; it does not
/// imply the public API call currently being handled returned allocation error.
pub fn last_alloc_failure() -> Option<AllocFailureDebug> {
    compat::memory::debug_last_alloc_failure().map(alloc_failure_from_snapshot)
}

/// Return the first compat allocation failure recorded since WiFi init.
///
/// This is a diagnostics-only hook for constrained hardware bring-up.
#[doc(hidden)]
pub fn first_alloc_failure() -> Option<AllocFailureDebug> {
    compat::memory::debug_first_alloc_failure().map(alloc_failure_from_snapshot)
}

fn alloc_failure_from_snapshot(
    snapshot: compat::memory::AllocFailureSnapshot,
) -> AllocFailureDebug {
    AllocFailureDebug {
        kind: snapshot.kind,
        requested_bytes: snapshot.requested_bytes,
        free_bytes: snapshot.free_bytes,
        task_marker: snapshot.task_marker,
        count: snapshot.count,
        eap_active_count: snapshot.eap_active_count,
        eap_reserved: snapshot.eap_reserved,
        eap_usable_bytes: snapshot.eap_usable_bytes,
        eap_in_use: snapshot.eap_in_use,
        eap_reserve_available: snapshot.eap_reserve_available,
    }
}

/// Return the current RSA workspace reservation counters.
#[doc(hidden)]
pub fn eap_rsa_workspace_debug() -> EapRsaWorkspaceDebug {
    let (active_count, reserve_attempts, reserved, used, released, usable_bytes, in_use) =
        compat::memory::debug_eap_rsa_workspace();
    EapRsaWorkspaceDebug {
        reserved_now: active_count != 0,
        active_count,
        reserve_attempts,
        reserved,
        used,
        released,
        usable_bytes,
        in_use,
    }
}

/// WiFi operating mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WifiMode {
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
}

/// Authentication mode for WiFi connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum AuthMode {
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
pub(crate) mod protocol {
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
pub(crate) enum Bandwidth {
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
pub(crate) enum PowerSave {
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
pub(crate) enum SecondaryChannel {
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
pub(crate) enum Storage {
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
