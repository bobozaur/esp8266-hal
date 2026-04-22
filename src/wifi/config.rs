//! WiFi configuration types for station and access-point modes.

use esp8266_wifi_sys as sys;

/// Station (client) configuration.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct StaConfig {
    /// SSID of the target AP (up to 32 bytes).
    pub ssid: heapless::Vec<u8, 32>,
    /// Password (up to 64 bytes). Empty for open networks.
    pub password: heapless::Vec<u8, 64>,
    /// Optional BSSID filter — only connect to this specific AP.
    pub bssid: Option<[u8; 6]>,
    /// Channel hint (0 = scan all channels).
    pub channel: u8,
    /// Scan method: fast scan or full channel scan.
    pub scan_method: ScanMethod,
    /// Sort method for selecting an AP.
    pub sort_method: SortMethod,
    /// Minimum RSSI threshold for fast scan.
    pub rssi_threshold: i8,
    /// Minimum auth mode threshold for fast scan.
    pub auth_threshold: crate::wifi::AuthMode,
    /// Listen interval for power-save mode (in beacon intervals, 0 = default 3).
    pub listen_interval: u16,
    /// Enable Protected Management Frame (802.11w) capability.
    pub pmf_capable: bool,
    /// Require Protected Management Frame.
    pub pmf_required: bool,
}

impl StaConfig {
    /// Create a builder for station configuration.
    pub fn builder<'a>(ssid: &'a str, password: &'a str) -> StaConfigBuilder<'a> {
        StaConfigBuilder {
            ssid: ssid.as_bytes(),
            password: password.as_bytes(),
            bssid: None,
            channel: 0,
            scan_method: ScanMethod::Fast,
            sort_method: SortMethod::Signal,
            rssi_threshold: -127,
            auth_threshold: crate::wifi::AuthMode::Open,
            listen_interval: 0,
            pmf_capable: true,
            pmf_required: false,
        }
    }

    pub(crate) fn to_raw(&self) -> sys::wifi_sta_config_t {
        let mut cfg: sys::wifi_sta_config_t = unsafe { core::mem::zeroed() };
        let ssid_len = self.ssid.len().min(32);
        cfg.ssid[..ssid_len].copy_from_slice(&self.ssid[..ssid_len]);
        let pwd_len = self.password.len().min(64);
        cfg.password[..pwd_len].copy_from_slice(&self.password[..pwd_len]);
        cfg.scan_method = self.scan_method.to_raw();
        cfg.sort_method = self.sort_method.to_raw();
        cfg.channel = self.channel;
        cfg.listen_interval = self.listen_interval;
        cfg.threshold = sys::wifi_fast_scan_threshold_t {
            rssi: self.rssi_threshold,
            authmode: self.auth_threshold.to_raw(),
        };
        cfg.pmf_cfg = sys::wifi_pmf_config_t {
            capable: self.pmf_capable,
            required: self.pmf_required,
        };
        if let Some(bssid) = self.bssid {
            cfg.bssid_set = true;
            cfg.bssid = bssid;
        }
        cfg
    }

    pub(crate) fn from_validated_bytes(
        ssid: &[u8],
        password: &[u8],
        auth_threshold: crate::wifi::AuthMode,
    ) -> Self {
        debug_assert!(ssid.len() <= 32);
        debug_assert!(password.len() <= 64);

        let mut ssid_vec = heapless::Vec::new();
        let _ = ssid_vec.extend_from_slice(ssid);
        let mut password_vec = heapless::Vec::new();
        let _ = password_vec.extend_from_slice(password);

        Self {
            ssid: ssid_vec,
            password: password_vec,
            bssid: None,
            channel: 0,
            scan_method: ScanMethod::Fast,
            sort_method: SortMethod::Signal,
            rssi_threshold: -127,
            auth_threshold,
            listen_interval: 0,
            pmf_capable: false,
            pmf_required: false,
        }
    }

    pub(crate) fn from_raw(raw: &sys::wifi_sta_config_t) -> Self {
        let ssid_bytes = crate::wifi::bytes_to_str(&raw.ssid);
        let pwd_bytes = crate::wifi::bytes_to_str(&raw.password);

        let mut ssid = heapless::Vec::new();
        let _ = ssid.extend_from_slice(ssid_bytes);
        let mut password = heapless::Vec::new();
        let _ = password.extend_from_slice(pwd_bytes);

        Self {
            ssid,
            password,
            bssid: if raw.bssid_set { Some(raw.bssid) } else { None },
            channel: raw.channel,
            scan_method: ScanMethod::from_raw(raw.scan_method),
            sort_method: SortMethod::from_raw(raw.sort_method),
            rssi_threshold: raw.threshold.rssi,
            auth_threshold: crate::wifi::AuthMode::from_raw(raw.threshold.authmode),
            listen_interval: raw.listen_interval,
            pmf_capable: raw.pmf_cfg.capable,
            pmf_required: raw.pmf_cfg.required,
        }
    }
}

/// Builder for [`StaConfig`].
pub struct StaConfigBuilder<'a> {
    ssid: &'a [u8],
    password: &'a [u8],
    bssid: Option<[u8; 6]>,
    channel: u8,
    scan_method: ScanMethod,
    sort_method: SortMethod,
    rssi_threshold: i8,
    auth_threshold: crate::wifi::AuthMode,
    listen_interval: u16,
    pmf_capable: bool,
    pmf_required: bool,
}

impl<'a> StaConfigBuilder<'a> {
    /// Filter by BSSID (MAC address of the target AP).
    pub fn bssid(mut self, bssid: [u8; 6]) -> Self {
        self.bssid = Some(bssid);
        self
    }

    /// Channel hint. Set to 0 to scan all channels.
    pub fn channel(mut self, ch: u8) -> Self {
        self.channel = ch;
        self
    }

    /// Use full channel scan instead of fast scan.
    pub fn full_scan(mut self) -> Self {
        self.scan_method = ScanMethod::AllChannel;
        self
    }

    /// Sort APs by security strength rather than signal strength.
    pub fn sort_by_security(mut self) -> Self {
        self.sort_method = SortMethod::Security;
        self
    }

    /// Minimum RSSI for fast scan (default -127).
    pub fn rssi_threshold(mut self, rssi: i8) -> Self {
        self.rssi_threshold = rssi;
        self
    }

    /// Minimum authentication mode for fast scan.
    pub fn auth_threshold(mut self, auth: crate::wifi::AuthMode) -> Self {
        self.auth_threshold = auth;
        self
    }

    /// Listen interval for power-save (beacon intervals).
    pub fn listen_interval(mut self, interval: u16) -> Self {
        self.listen_interval = interval;
        self
    }

    /// Configure PMF support.
    pub fn pmf(mut self, capable: bool, required: bool) -> Self {
        self.pmf_capable = capable;
        self.pmf_required = required;
        self
    }

    /// Build the station configuration.
    pub fn build(self) -> StaConfig {
        let mut ssid = heapless::Vec::new();
        let _ = ssid.extend_from_slice(&self.ssid[..self.ssid.len().min(32)]);
        let mut password = heapless::Vec::new();
        let _ = password.extend_from_slice(&self.password[..self.password.len().min(64)]);

        StaConfig {
            ssid,
            password,
            bssid: self.bssid,
            channel: self.channel,
            scan_method: self.scan_method,
            sort_method: self.sort_method,
            rssi_threshold: self.rssi_threshold,
            auth_threshold: self.auth_threshold,
            listen_interval: self.listen_interval,
            pmf_capable: self.pmf_capable,
            pmf_required: self.pmf_required,
        }
    }
}

/// Access-point (soft-AP) configuration.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ApConfig {
    /// SSID to broadcast (up to 32 bytes).
    pub ssid: heapless::Vec<u8, 32>,
    /// Password (8–64 bytes). Empty implies open auth.
    pub password: heapless::Vec<u8, 64>,
    /// WiFi channel (1–14).
    pub channel: u8,
    /// Authentication mode. `AuthMode::Wep` is not supported in AP mode.
    pub auth_mode: crate::wifi::AuthMode,
    /// Hide the SSID from beacon frames.
    pub ssid_hidden: bool,
    /// Maximum number of connected stations (1–10, SDK default 4).
    pub max_connections: u8,
    /// Beacon interval in milliseconds (100–60000, default 100).
    pub beacon_interval: u16,
}

impl ApConfig {
    /// Create a builder for AP configuration.
    pub fn builder(ssid: &str) -> ApConfigBuilder<'_> {
        ApConfigBuilder {
            ssid: ssid.as_bytes(),
            password: &[],
            channel: 1,
            auth_mode: crate::wifi::AuthMode::Open,
            ssid_hidden: false,
            max_connections: 4,
            beacon_interval: 100,
        }
    }

    pub(crate) fn to_raw(&self) -> sys::wifi_ap_config_t {
        let mut cfg: sys::wifi_ap_config_t = unsafe { core::mem::zeroed() };
        let ssid_len = self.ssid.len().min(32);
        cfg.ssid[..ssid_len].copy_from_slice(&self.ssid[..ssid_len]);
        cfg.ssid_len = ssid_len as u8;
        let pwd_len = self.password.len().min(64);
        cfg.password[..pwd_len].copy_from_slice(&self.password[..pwd_len]);
        cfg.channel = self.channel;
        cfg.authmode = self.auth_mode.to_raw();
        cfg.ssid_hidden = if self.ssid_hidden { 1 } else { 0 };
        cfg.max_connection = self.max_connections;
        cfg.beacon_interval = self.beacon_interval;
        cfg
    }

    pub(crate) fn from_validated_bytes(
        ssid: &[u8],
        password: &[u8],
        channel: u8,
        max_connections: u8,
        beacon_interval: u16,
        auth_mode: crate::wifi::AuthMode,
    ) -> Self {
        debug_assert!(ssid.len() <= 32);
        debug_assert!(password.len() <= 64);

        let mut ssid_vec = heapless::Vec::new();
        let _ = ssid_vec.extend_from_slice(ssid);
        let mut password_vec = heapless::Vec::new();
        let _ = password_vec.extend_from_slice(password);

        Self {
            ssid: ssid_vec,
            password: password_vec,
            channel,
            auth_mode,
            ssid_hidden: false,
            max_connections,
            beacon_interval,
        }
    }

    pub(crate) fn from_raw(raw: &sys::wifi_ap_config_t) -> Self {
        let ssid_len = if raw.ssid_len > 0 {
            raw.ssid_len as usize
        } else {
            crate::wifi::bytes_to_str(&raw.ssid).len()
        };
        let mut ssid = heapless::Vec::new();
        let _ = ssid.extend_from_slice(&raw.ssid[..ssid_len.min(32)]);

        let pwd_bytes = crate::wifi::bytes_to_str(&raw.password);
        let mut password = heapless::Vec::new();
        let _ = password.extend_from_slice(pwd_bytes);

        Self {
            ssid,
            password,
            channel: raw.channel,
            auth_mode: crate::wifi::AuthMode::from_raw(raw.authmode),
            ssid_hidden: raw.ssid_hidden != 0,
            max_connections: raw.max_connection,
            beacon_interval: raw.beacon_interval,
        }
    }
}

/// Builder for [`ApConfig`].
pub struct ApConfigBuilder<'a> {
    ssid: &'a [u8],
    password: &'a [u8],
    channel: u8,
    auth_mode: crate::wifi::AuthMode,
    ssid_hidden: bool,
    max_connections: u8,
    beacon_interval: u16,
}

impl<'a> ApConfigBuilder<'a> {
    /// Set the AP password and authentication mode.
    pub fn password(mut self, password: &'a str, auth: crate::wifi::AuthMode) -> Self {
        self.password = password.as_bytes();
        self.auth_mode = auth;
        self
    }

    /// Set the WiFi channel (1–14).
    pub fn channel(mut self, ch: u8) -> Self {
        self.channel = ch;
        self
    }

    /// Hide the SSID.
    pub fn hidden(mut self) -> Self {
        self.ssid_hidden = true;
        self
    }

    /// Maximum connected stations (1–10).
    pub fn max_connections(mut self, n: u8) -> Self {
        self.max_connections = n;
        self
    }

    /// Beacon interval in milliseconds.
    pub fn beacon_interval(mut self, ms: u16) -> Self {
        self.beacon_interval = ms;
        self
    }

    /// Build the access-point configuration.
    pub fn build(self) -> ApConfig {
        let mut ssid = heapless::Vec::new();
        let _ = ssid.extend_from_slice(&self.ssid[..self.ssid.len().min(32)]);
        let mut password = heapless::Vec::new();
        let _ = password.extend_from_slice(&self.password[..self.password.len().min(64)]);

        ApConfig {
            ssid,
            password,
            channel: self.channel,
            auth_mode: self.auth_mode,
            ssid_hidden: self.ssid_hidden,
            max_connections: self.max_connections,
            beacon_interval: self.beacon_interval,
        }
    }
}

/// Combined WiFi configuration.
#[derive(Debug, Clone, Default)]
pub enum Configuration {
    /// No WiFi configuration (default state).
    #[default]
    None,
    /// Station-only configuration.
    Client(StaConfig),
    /// Access-point only configuration.
    AccessPoint(ApConfig),
    /// Simultaneous station + AP configuration.
    Mixed(StaConfig, ApConfig),
}

impl Configuration {
    /// Get a reference to the station config, if present.
    pub fn as_sta(&self) -> Option<&StaConfig> {
        match self {
            Self::Client(c) | Self::Mixed(c, _) => Some(c),
            _ => None,
        }
    }

    /// Get a reference to the AP config, if present.
    pub fn as_ap(&self) -> Option<&ApConfig> {
        match self {
            Self::AccessPoint(a) | Self::Mixed(_, a) => Some(a),
            _ => None,
        }
    }
}

/// Scan method selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanMethod {
    /// Fast scan — stop after first matching AP.
    Fast,
    /// Full channel scan — scan all channels.
    AllChannel,
}

impl ScanMethod {
    pub(crate) fn to_raw(self) -> sys::wifi_scan_method_t {
        match self {
            Self::Fast => sys::WIFI_FAST_SCAN,
            Self::AllChannel => sys::WIFI_ALL_CHANNEL_SCAN,
        }
    }

    pub(crate) fn from_raw(raw: sys::wifi_scan_method_t) -> Self {
        match raw {
            sys::WIFI_ALL_CHANNEL_SCAN => Self::AllChannel,
            _ => Self::Fast,
        }
    }
}

/// AP sort method for connection selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortMethod {
    /// Sort by RSSI (signal strength).
    Signal,
    /// Sort by security mode.
    Security,
}

impl SortMethod {
    pub(crate) fn to_raw(self) -> sys::wifi_sort_method_t {
        match self {
            Self::Signal => sys::WIFI_CONNECT_AP_BY_SIGNAL,
            Self::Security => sys::WIFI_CONNECT_AP_BY_SECURITY,
        }
    }

    pub(crate) fn from_raw(raw: sys::wifi_sort_method_t) -> Self {
        match raw {
            sys::WIFI_CONNECT_AP_BY_SECURITY => Self::Security,
            _ => Self::Signal,
        }
    }
}
