//! WiFi Protected Setup (WPS) enrollment.
//!
//! WPS allows a station to join a network by pressing a button on the router
//! (PBC mode) or entering a PIN. Requires the `wps` feature flag.
//!
//! # Requirements
//!
//! - The WiFi driver must be in station mode (not AP or sniffer mode).
//! - Call [`enable`] before [`start`], and [`disable`] to release resources
//!   when done.
//!
//! # Events
//!
//! WPS progress is reported via [`WifiEvent`](crate::wifi::WifiEvent) variants:
//! - `StaWpsSuccess` — enrollment succeeded, credentials are stored.
//! - `StaWpsFailed` — enrollment failed.
//! - `StaWpsTimeout` — the enrollment window expired.
//! - `StaWpsPin { pin }` — a PIN was generated (PIN mode only).

use esp8266_wifi_sys as sys;

use crate::wifi::error::{EspError, WifiError};

/// WPS enrollment type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WpsType {
    /// Push-Button Configuration — press the WPS button on the router.
    Pbc,
    /// PIN-based enrollment — enter the displayed PIN on the router.
    Pin,
}

impl WpsType {
    fn to_raw(self) -> sys::wps_type_t {
        match self {
            Self::Pbc => sys::WPS_TYPE_PBC,
            Self::Pin => sys::WPS_TYPE_PIN,
        }
    }
}

/// WPS device identity information reported during enrollment.
///
/// All fields are optional; empty strings cause the SDK to use its built-in
/// defaults ("ESPRESSIF", "ESP8266", "ESPRESSIF IOT", "ESP STATION").
#[derive(Debug, Clone)]
pub struct WpsFactoryInfo {
    /// Manufacturer name (max 64 chars).
    pub manufacturer: heapless::Vec<u8, 64>,
    /// Model number (max 32 chars).
    pub model_number: heapless::Vec<u8, 32>,
    /// Model name (max 32 chars).
    pub model_name: heapless::Vec<u8, 32>,
    /// Device name (max 32 chars).
    pub device_name: heapless::Vec<u8, 32>,
}

impl Default for WpsFactoryInfo {
    fn default() -> Self {
        Self {
            manufacturer: heapless::Vec::new(),
            model_number: heapless::Vec::new(),
            model_name: heapless::Vec::new(),
            device_name: heapless::Vec::new(),
        }
    }
}

/// WPS configuration.
#[derive(Debug, Clone)]
pub struct WpsConfig {
    /// Enrollment type (PBC or PIN).
    pub wps_type: WpsType,
    /// Device identity information.
    pub factory_info: WpsFactoryInfo,
}

impl WpsConfig {
    /// Create a PBC (push-button) config with default factory info.
    pub fn pbc() -> Self {
        Self {
            wps_type: WpsType::Pbc,
            factory_info: WpsFactoryInfo::default(),
        }
    }

    /// Create a PIN config with default factory info.
    pub fn pin() -> Self {
        Self {
            wps_type: WpsType::Pin,
            factory_info: WpsFactoryInfo::default(),
        }
    }

    fn to_raw(&self) -> sys::esp_wps_config_t {
        let mut cfg: sys::esp_wps_config_t = unsafe { core::mem::zeroed() };
        cfg.wps_type = self.wps_type.to_raw();

        fn copy_str(dst: &mut [core::ffi::c_char], src: &[u8]) {
            let len = src.len().min(dst.len().saturating_sub(1));
            for (i, &b) in src[..len].iter().enumerate() {
                dst[i] = b as core::ffi::c_char;
            }
        }

        copy_str(
            &mut cfg.factory_info.manufacturer,
            &self.factory_info.manufacturer,
        );
        copy_str(
            &mut cfg.factory_info.model_number,
            &self.factory_info.model_number,
        );
        copy_str(
            &mut cfg.factory_info.model_name,
            &self.factory_info.model_name,
        );
        copy_str(
            &mut cfg.factory_info.device_name,
            &self.factory_info.device_name,
        );

        cfg
    }
}

/// Enable WPS with the given configuration.
///
/// The WiFi driver must be started in station mode. After enabling, call
/// [`start`] to begin the enrollment process.
pub fn enable(config: &WpsConfig) -> Result<(), WifiError> {
    let raw = config.to_raw();
    EspError::check(unsafe { sys::esp_wifi_wps_enable(&raw) })?;
    Ok(())
}

/// Disable WPS and release associated resources.
pub fn disable() -> Result<(), WifiError> {
    EspError::check(unsafe { sys::esp_wifi_wps_disable() })?;
    Ok(())
}

/// Start the WPS enrollment process.
///
/// `timeout_ms` controls blocking behavior:
/// - `0` — non-blocking, returns immediately. Monitor progress via
///   `WifiEvent::StaWps*` events.
/// - `1..=120_000` — blocks up to the given milliseconds.
///
/// The router must be in WPS mode (PBC button pressed or PIN entered).
pub fn start(timeout_ms: i32) -> Result<(), WifiError> {
    EspError::check(unsafe { sys::esp_wifi_wps_start(timeout_ms) })?;
    Ok(())
}
