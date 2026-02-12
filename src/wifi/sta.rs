//! Station (client) mode utilities.
//!
//! These functions operate on the station interface and require the WiFi
//! driver to be running in STA or APSTA mode.

use esp8266_wifi_sys as sys;

use crate::wifi::error::{EspError, WifiError};
use crate::wifi::scan::AccessPointInfo;

/// Get information about the AP the station is currently connected to.
pub fn get_ap_info() -> Result<AccessPointInfo, WifiError> {
    let mut raw: sys::wifi_ap_record_t = unsafe { core::mem::zeroed() };
    EspError::check(unsafe { sys::esp_wifi_sta_get_ap_info(&mut raw) })?;
    Ok(AccessPointInfo::from_raw(&raw))
}
