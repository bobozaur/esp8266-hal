//! WiFi scanning — discover nearby access points.

use esp8266_wifi_sys as sys;

use crate::wifi::error::{EspError, WifiError};
use crate::wifi::{AuthMode, SecondaryChannel};

/// Scan configuration.
#[derive(Debug, Clone)]
pub struct ScanConfig {
    /// SSID filter (scan only for this SSID). `None` = scan all.
    pub ssid: Option<heapless::Vec<u8, 32>>,
    /// BSSID filter. `None` = any.
    pub bssid: Option<[u8; 6]>,
    /// Channel to scan (0 = all channels).
    pub channel: u8,
    /// Include hidden SSIDs in results.
    pub show_hidden: bool,
    /// Scan type.
    pub scan_type: ScanType,
    /// Minimum active scan time per channel (ms).
    pub active_scan_min_ms: u32,
    /// Maximum active scan time per channel (ms, max recommended 1500).
    pub active_scan_max_ms: u32,
    /// Passive scan time per channel (ms).
    pub passive_scan_ms: u32,
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            ssid: None,
            bssid: None,
            channel: 0,
            show_hidden: false,
            scan_type: ScanType::Active,
            active_scan_min_ms: 0,
            active_scan_max_ms: 0,
            passive_scan_ms: 0,
        }
    }
}

/// Active or passive scanning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanType {
    Active,
    Passive,
}

/// Information about a discovered access point.
#[derive(Debug, Clone)]
pub struct AccessPointInfo {
    /// SSID (may be empty for hidden networks).
    pub ssid: heapless::Vec<u8, 33>,
    /// BSSID (MAC address).
    pub bssid: [u8; 6],
    /// Primary channel.
    pub channel: u8,
    /// Secondary channel position.
    pub secondary_channel: SecondaryChannel,
    /// Signal strength in dBm.
    pub rssi: i8,
    /// Authentication mode.
    pub auth_mode: AuthMode,
    /// `true` if the AP supports 802.11b.
    pub phy_11b: bool,
    /// `true` if the AP supports 802.11g.
    pub phy_11g: bool,
    /// `true` if the AP supports 802.11n.
    pub phy_11n: bool,
    /// `true` if the AP supports WPS.
    pub wps: bool,
}

impl AccessPointInfo {
    pub(crate) fn from_raw(r: &sys::wifi_ap_record_t) -> Self {
        let ssid_bytes = crate::wifi::bytes_to_str(&r.ssid);
        let mut ssid = heapless::Vec::new();
        let _ = ssid.extend_from_slice(ssid_bytes);

        Self {
            ssid,
            bssid: r.bssid,
            channel: r.primary,
            secondary_channel: SecondaryChannel::from_raw(r.second),
            rssi: r.rssi,
            auth_mode: AuthMode::from_raw(r.authmode),
            phy_11b: r.phy_11b() != 0,
            phy_11g: r.phy_11g() != 0,
            phy_11n: r.phy_11n() != 0,
            wps: r.wps() != 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Raw config builder (shared between blocking and async)
// ---------------------------------------------------------------------------

/// Temporary buffers for the raw scan config. Must live as long as the SDK
/// needs the `wifi_scan_config_t` (i.e., until the scan starts for blocking
/// mode, or until scan completes for async mode).
struct RawScanConfigBuffers {
    ssid_buf: [u8; 33],
    bssid_buf: [u8; 6],
    has_ssid: bool,
    has_bssid: bool,
    channel: u8,
    show_hidden: bool,
    scan_type: ScanType,
    active_scan_min_ms: u32,
    active_scan_max_ms: u32,
    passive_scan_ms: u32,
}

impl RawScanConfigBuffers {
    fn from_config(config: &ScanConfig) -> Self {
        let mut ssid_buf = [0u8; 33];
        let mut bssid_buf = [0u8; 6];
        let has_ssid = if let Some(s) = &config.ssid {
            let len = s.len().min(32);
            ssid_buf[..len].copy_from_slice(&s[..len]);
            true
        } else {
            false
        };
        let has_bssid = if let Some(b) = &config.bssid {
            bssid_buf = *b;
            true
        } else {
            false
        };
        Self {
            ssid_buf,
            bssid_buf,
            has_ssid,
            has_bssid,
            channel: config.channel,
            show_hidden: config.show_hidden,
            scan_type: config.scan_type,
            active_scan_min_ms: config.active_scan_min_ms,
            active_scan_max_ms: config.active_scan_max_ms,
            passive_scan_ms: config.passive_scan_ms,
        }
    }

    fn to_raw(&mut self) -> sys::wifi_scan_config_t {
        let ssid_ptr = if self.has_ssid {
            self.ssid_buf.as_mut_ptr()
        } else {
            core::ptr::null_mut()
        };
        let bssid_ptr = if self.has_bssid {
            self.bssid_buf.as_mut_ptr()
        } else {
            core::ptr::null_mut()
        };

        sys::wifi_scan_config_t {
            ssid: ssid_ptr,
            bssid: bssid_ptr,
            channel: self.channel,
            show_hidden: self.show_hidden,
            scan_type: match self.scan_type {
                ScanType::Active => sys::WIFI_SCAN_TYPE_ACTIVE,
                ScanType::Passive => sys::WIFI_SCAN_TYPE_PASSIVE,
            },
            scan_time: match self.scan_type {
                ScanType::Active => sys::wifi_scan_time_t {
                    active: sys::wifi_active_scan_time_t {
                        min: self.active_scan_min_ms,
                        max: self.active_scan_max_ms,
                    },
                },
                ScanType::Passive => sys::wifi_scan_time_t {
                    passive: self.passive_scan_ms,
                },
            },
        }
    }
}

/// Ensure scan results are freed even if the caller drops the future.
struct FreeApListOnDrop;

impl Drop for FreeApListOnDrop {
    fn drop(&mut self) {
        unsafe { sys::esp_wifi_scan_stop() };
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Start a blocking scan and return results.
///
/// This is a convenience wrapper that calls `esp_wifi_scan_start(block=true)`,
/// then retrieves results. The WiFi driver must be started in a mode that
/// includes station (STA or APSTA).
pub fn scan_blocking(config: &ScanConfig) -> Result<heapless::Vec<AccessPointInfo, 32>, WifiError> {
    let mut bufs = RawScanConfigBuffers::from_config(config);
    let raw_config = bufs.to_raw();

    EspError::check(unsafe { sys::esp_wifi_scan_start(&raw_config, true) })?;

    get_scan_results()
}

/// Start an asynchronous scan and return results when complete.
///
/// This starts the scan in non-blocking mode, awaits the `ScanDone` event,
/// then retrieves results. The SSID/BSSID filter buffers are kept alive for
/// the entire duration of the scan.
///
/// If this future is dropped before completing, `esp_wifi_scan_stop()` is
/// called to clean up.
pub async fn scan_async(
    config: &ScanConfig,
) -> Result<heapless::Vec<AccessPointInfo, 32>, WifiError> {
    crate::wifi::event::ensure_trampoline_registered()?;
    let mut sub = crate::wifi::event::event_subscriber().ok_or(WifiError::Internal(
        crate::wifi::EspError(sys::ESP_ERR_NO_MEM),
    ))?;

    // Keep buffers alive across the await point.
    let mut bufs = RawScanConfigBuffers::from_config(config);
    let raw_config = bufs.to_raw();

    EspError::check(unsafe { sys::esp_wifi_scan_start(&raw_config, false) })?;

    // Guard: stop scan if this future is dropped.
    let _guard = FreeApListOnDrop;

    // Wait for scan completion.
    loop {
        let event = sub.next_message_pure().await;
        if let crate::wifi::WifiEvent::ScanDone { success, .. } = event {
            // Disarm the guard — we'll handle cleanup via get_scan_results.
            core::mem::forget(_guard);
            if success {
                return get_scan_results();
            } else {
                return Err(WifiError::Internal(crate::wifi::EspError(sys::ESP_FAIL)));
            }
        }
    }
}

/// Start a non-blocking scan (low-level).
///
/// Use [`scan_async`] instead for a safe high-level API. This function is
/// only useful if you manage event subscription yourself.
///
/// Results can be retrieved after receiving `WifiEvent::ScanDone` by calling
/// [`get_scan_results`].
///
/// **Note:** `config.ssid` and `config.bssid` filters are ignored for
/// non-blocking scans to avoid dangling pointers. Use [`scan_async`] if
/// you need filtering.
pub fn scan_start_nonblocking(config: &ScanConfig) -> Result<(), WifiError> {
    let raw_config = sys::wifi_scan_config_t {
        ssid: core::ptr::null_mut(),
        bssid: core::ptr::null_mut(),
        channel: config.channel,
        show_hidden: config.show_hidden,
        scan_type: match config.scan_type {
            ScanType::Active => sys::WIFI_SCAN_TYPE_ACTIVE,
            ScanType::Passive => sys::WIFI_SCAN_TYPE_PASSIVE,
        },
        scan_time: match config.scan_type {
            ScanType::Active => sys::wifi_scan_time_t {
                active: sys::wifi_active_scan_time_t {
                    min: config.active_scan_min_ms,
                    max: config.active_scan_max_ms,
                },
            },
            ScanType::Passive => sys::wifi_scan_time_t {
                passive: config.passive_scan_ms,
            },
        },
    };

    EspError::check(unsafe { sys::esp_wifi_scan_start(&raw_config, false) })?;
    Ok(())
}

/// Stop an in-progress scan.
pub fn scan_stop() -> Result<(), WifiError> {
    EspError::check(unsafe { sys::esp_wifi_scan_stop() })?;
    Ok(())
}

/// Retrieve the results of the last completed scan.
///
/// Returns up to 32 results. The SDK frees internal memory once results are
/// retrieved.
pub fn get_scan_results() -> Result<heapless::Vec<AccessPointInfo, 32>, WifiError> {
    let mut count: u16 = 0;
    EspError::check(unsafe { sys::esp_wifi_scan_get_ap_num(&mut count) })?;

    let fetch = (count as usize).min(32) as u16;
    let mut records: [sys::wifi_ap_record_t; 32] = unsafe { core::mem::zeroed() };
    let mut actual = fetch;
    EspError::check(unsafe {
        sys::esp_wifi_scan_get_ap_records(&mut actual, records.as_mut_ptr())
    })?;

    let mut results = heapless::Vec::new();
    for record in records.iter().take(actual as usize) {
        let _ = results.push(AccessPointInfo::from_raw(record));
    }

    Ok(results)
}
