//! Access-point (soft-AP) mode utilities.
//!
//! These functions operate on the soft-AP interface and require the WiFi
//! driver to be running in AP or APSTA mode.

use esp8266_wifi_sys as sys;

use crate::wifi::error::{EspError, WifiError};

/// Information about a station connected to the soft-AP.
#[derive(Debug, Clone)]
pub struct StationInfo {
    /// MAC address of the connected station.
    pub mac: [u8; 6],
    /// `true` if the station supports 802.11b.
    pub phy_11b: bool,
    /// `true` if the station supports 802.11g.
    pub phy_11g: bool,
    /// `true` if the station supports 802.11n.
    pub phy_11n: bool,
}

/// Get the list of stations currently connected to the soft-AP.
pub fn get_connected_stations() -> Result<heapless::Vec<StationInfo, 10>, WifiError> {
    let mut raw: sys::wifi_sta_list_t = unsafe { core::mem::zeroed() };
    EspError::check(unsafe { sys::esp_wifi_ap_get_sta_list(&mut raw) })?;

    let mut list = heapless::Vec::new();
    let count = (raw.num as usize).min(sys::ESP_WIFI_MAX_CONN_NUM);
    for i in 0..count {
        let sta = &raw.sta[i];
        let _ = list.push(StationInfo {
            mac: sta.mac,
            phy_11b: sta.phy_11b() != 0,
            phy_11g: sta.phy_11g() != 0,
            phy_11n: sta.phy_11n() != 0,
        });
    }

    Ok(list)
}

/// Deauthenticate a station from the soft-AP.
///
/// Pass `aid = 0` to deauthenticate all connected stations.
pub fn deauth_station(aid: u16) -> Result<(), WifiError> {
    EspError::check(unsafe { sys::esp_wifi_deauth_sta(aid) })?;
    Ok(())
}
