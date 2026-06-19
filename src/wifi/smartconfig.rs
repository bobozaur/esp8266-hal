//! SmartConfig provisioning.
//!
//! SmartConfig allows a station to receive WiFi credentials from an
//! ESPTouch/AirKiss-compatible sender over the air without a pre-existing
//! connection. Requires the `wifi-smartconfig` feature flag.
//!
//! # Supported protocols
//!
//! - **ESPTouch** — Espressif's original protocol.
//! - **AirKiss** — WeChat-compatible protocol.
//! - **ESPTouch + AirKiss** — listen for both simultaneously.
//! - **ESPTouch V2** — enhanced protocol with optional encryption.
//!
//! # Usage
//!
//! 1. Start the WiFi driver in station mode.
//! 2. Optionally call [`set_type`] to select the protocol.
//! 3. Call [`start`] to begin listening.
//! 4. Handle `SC_EVENT_GOT_SSID_PSWD` via the event system to retrieve
//!    credentials, then connect.
//! 5. Call [`stop`] to release resources.
//!
//! Hardware validation needs a real SmartConfig sender. The repo-local hardware
//! harness currently uses an ESP32 companion ESPTouch sender; phone apps remain
//! useful for interoperability checks outside the automated matrix.

use esp8266_wifi_sys as sys;

use crate::wifi::error::{EspError, WifiError};

/// SmartConfig protocol type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmartConfigType {
    /// ESPTouch protocol.
    EspTouch,
    /// AirKiss protocol (WeChat-compatible).
    AirKiss,
    /// Listen for both ESPTouch and AirKiss.
    EspTouchAirKiss,
    /// ESPTouch V2 (supports encryption).
    EspTouchV2,
}

impl SmartConfigType {
    fn to_raw(self) -> sys::smartconfig_type_t {
        match self {
            Self::EspTouch => sys::SC_TYPE_ESPTOUCH,
            Self::AirKiss => sys::SC_TYPE_AIRKISS,
            Self::EspTouchAirKiss => sys::SC_TYPE_ESPTOUCH_AIRKISS,
            Self::EspTouchV2 => sys::SC_TYPE_ESPTOUCH_V2,
        }
    }
}

/// SmartConfig start parameters.
#[derive(Debug, Clone)]
pub struct SmartConfigStartConfig {
    /// Enable internal SmartConfig debug logging.
    pub enable_log: bool,
    /// Enable ESPTouch V2 encryption.
    pub esp_touch_v2_enable_crypt: bool,
    /// ESPTouch V2 encryption key (exactly 16 bytes). Ignored unless
    /// `esp_touch_v2_enable_crypt` is `true`.
    pub esp_touch_v2_key: Option<[u8; 16]>,
}

impl Default for SmartConfigStartConfig {
    fn default() -> Self {
        Self {
            enable_log: false,
            esp_touch_v2_enable_crypt: false,
            esp_touch_v2_key: None,
        }
    }
}

/// Start SmartConfig with the given configuration.
///
/// The WiFi driver must be in station mode. The sender broadcasts SSID and
/// password data which the device captures from the air.
///
/// Progress is reported via the event system (`SC_EVENT` base):
/// - `SC_EVENT_SCAN_DONE` — channel scanning complete.
/// - `SC_EVENT_FOUND_CHANNEL` — target AP channel found.
/// - `SC_EVENT_GOT_SSID_PSWD` — credentials received.
/// - `SC_EVENT_SEND_ACK_DONE` — acknowledgement sent to sender.
pub fn start(config: &SmartConfigStartConfig) -> Result<(), WifiError> {
    if unsafe { super::compat::timer::start_ets_timer_task() } != 1 {
        return Err(WifiError::Internal(EspError(sys::ESP_ERR_NO_MEM)));
    }

    // If a V2 key is provided, we need to keep it alive for the duration of
    // the call. The SDK copies the pointer contents internally.
    let mut key_buf: [u8; 17] = [0; 17]; // 16 bytes + null terminator
    let key_ptr = match &config.esp_touch_v2_key {
        Some(key) if config.esp_touch_v2_enable_crypt => {
            key_buf[..16].copy_from_slice(key);
            key_buf.as_ptr() as *const core::ffi::c_char
        }
        _ => core::ptr::null(),
    };

    let raw = sys::smartconfig_start_config_t {
        enable_log: config.enable_log,
        esp_touch_v2_enable_crypt: config.esp_touch_v2_enable_crypt,
        esp_touch_v2_key: key_ptr as *mut core::ffi::c_char,
    };

    super::compat::port::set_wdev_tick_poll_enabled(true);
    if let Err(error) = EspError::check(unsafe { sys::esp_smartconfig_start(&raw) }) {
        super::compat::port::set_wdev_tick_poll_enabled(false);
        return Err(error.into());
    }
    Ok(())
}

/// Stop SmartConfig and release associated resources.
///
/// Must be called after provisioning completes (whether successful or not)
/// to free memory allocated by [`start`].
pub fn stop() -> Result<(), WifiError> {
    let result = EspError::check(unsafe { sys::esp_smartconfig_stop() }).map_err(WifiError::from);
    super::compat::port::set_wdev_tick_poll_enabled(false);
    result
}

/// Set the SmartConfig protocol type.
///
/// Must be called before [`start`] if you want a type other than the default.
pub fn set_type(sc_type: SmartConfigType) -> Result<(), WifiError> {
    EspError::check(unsafe { sys::esp_smartconfig_set_type(sc_type.to_raw()) })?;
    Ok(())
}

/// Set the SmartConfig timeout.
///
/// `time_s` is in the range 15–255 seconds, with a 45-second offset (i.e.,
/// the actual timeout is `time_s + 45` seconds). SmartConfig restarts
/// automatically if the timeout expires.
pub fn set_timeout(time_s: u8) -> Result<(), WifiError> {
    EspError::check(unsafe { sys::esp_esptouch_set_timeout(time_s) })?;
    Ok(())
}

/// Enable or disable SmartConfig fast mode.
///
/// Fast mode requires a compatible sender. Both modes are interoperable.
/// Must be called before [`start`].
pub fn set_fast_mode(enable: bool) -> Result<(), WifiError> {
    EspError::check(unsafe { sys::esp_smartconfig_fast_mode(enable) })?;
    Ok(())
}

/// Get reserved data from ESPTouch V2.
///
/// Reads up to `buf.len()` bytes of application-specific reserved data
/// transmitted by the sender during ESPTouch V2 provisioning.
pub fn get_rvd_data(buf: &mut [u8]) -> Result<(), WifiError> {
    if buf.is_empty() || buf.len() > 255 {
        return Err(WifiError::InvalidArgument);
    }
    EspError::check(unsafe {
        sys::esp_smartconfig_get_rvd_data(buf.as_mut_ptr(), buf.len() as u8)
    })?;
    Ok(())
}
