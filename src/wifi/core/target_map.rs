#[cfg(feature = "wifi-promiscuous")]
use super::PromiscuousFilter;
#[cfg(all(feature = "wifi-smartconfig", target_arch = "xtensa"))]
use super::SmartConfigType;
#[cfg(all(feature = "wifi-esp-now", target_arch = "xtensa"))]
use super::{feature::EspNowInterface, EspNowPeerConfig};
#[cfg(target_arch = "xtensa")]
use super::{AccessPointConfig, ApCredential, RawMode, ScanOptions, ScanType};
use super::{Channel, ClientConfig, InitConfig, StationPowerSave};
#[cfg(all(feature = "wifi-promiscuous", target_arch = "xtensa"))]
use super::{PromiscuousControlFilter, PromiscuousPacketType};
#[cfg(all(feature = "wifi-wps", target_arch = "xtensa"))]
use super::{WpsConfig, WpsMode};

#[cfg(target_arch = "xtensa")]
extern "C" {
    fn esp_event_send(event: *mut esp8266_wifi_sys::system_event_t) -> esp8266_wifi_sys::esp_err_t;
}

#[inline(always)]
pub(super) fn raw_init_config(config: &InitConfig) -> esp8266_wifi_sys::wifi_init_config_t {
    // These values are the pinned SDK defaults. Only the three buffer counts
    // are intentionally exposed: the remaining switches are coupled to the
    // prebuilt Wi-Fi blobs and are not useful application-level knobs.
    esp8266_wifi_sys::wifi_init_config_t {
        event_handler: {
            #[cfg(target_arch = "xtensa")]
            {
                Some(esp_event_send)
            }
            #[cfg(not(target_arch = "xtensa"))]
            {
                None
            }
        },
        osi_funcs: core::ptr::null_mut(),
        qos_enable: 0,
        ampdu_rx_enable: 0,
        rx_ba_win: 0,
        rx_ampdu_buf_num: 5,
        rx_ampdu_buf_len: 256,
        rx_max_single_pkt_len: 1_076,
        rx_buf_len: 524,
        amsdu_rx_enable: 0,
        rx_buf_num: config.rx_buf_num(),
        rx_pkt_num: config.rx_pkt_num(),
        left_continuous_rx_buf_num: config.rx_buf_num().min(16),
        tx_buf_num: config.tx_buf_num(),
        // The Rust NVS shim supports PHY/MAC startup, but not the complete
        // `nvs.net80211` persistence model expected by the blob.
        nvs_enable: 0,
        nano_enable: 0,
        wpa3_sae_enable: 1,
        magic: esp8266_wifi_sys::WIFI_INIT_CONFIG_MAGIC,
    }
}

#[inline(always)]
pub(super) fn raw_client_config(config: &ClientConfig<'_>) -> esp8266_wifi_sys::wifi_sta_config_t {
    let (password, auth_threshold) = if let Some(passphrase) = config.credential().passphrase() {
        (passphrase.as_bytes(), esp8266_wifi_sys::WIFI_AUTH_WPA2_PSK)
    } else {
        (&[][..], esp8266_wifi_sys::WIFI_AUTH_OPEN)
    };

    let mut raw: esp8266_wifi_sys::wifi_sta_config_t = unsafe { core::mem::zeroed() };
    raw.ssid[..config.ssid().as_bytes().len()].copy_from_slice(config.ssid().as_bytes());
    raw.password[..password.len()].copy_from_slice(password);
    raw.scan_method = if config.channel().is_some() {
        esp8266_wifi_sys::WIFI_FAST_SCAN
    } else {
        esp8266_wifi_sys::WIFI_ALL_CHANNEL_SCAN
    };
    raw.channel = config.channel().map_or(0, Channel::get);
    raw.sort_method = esp8266_wifi_sys::WIFI_CONNECT_AP_BY_SIGNAL;
    raw.threshold = esp8266_wifi_sys::wifi_fast_scan_threshold_t {
        rssi: -127,
        authmode: auth_threshold,
    };
    raw.pmf_cfg = esp8266_wifi_sys::wifi_pmf_config_t {
        capable: false,
        required: false,
    };
    if let Some(bssid) = config.bssid() {
        raw.bssid_set = true;
        raw.bssid = bssid;
    }
    raw
}

#[inline(always)]
pub(super) const fn raw_station_power_save(
    power_save: StationPowerSave,
) -> esp8266_wifi_sys::wifi_ps_type_t {
    match power_save {
        StationPowerSave::None => esp8266_wifi_sys::WIFI_PS_NONE,
        StationPowerSave::MinModem => esp8266_wifi_sys::WIFI_PS_MIN_MODEM,
        StationPowerSave::MaxModem => esp8266_wifi_sys::WIFI_PS_MAX_MODEM,
    }
}

#[inline(always)]
#[cfg(target_arch = "xtensa")]
pub(super) fn raw_ap_config(config: &AccessPointConfig) -> esp8266_wifi_sys::wifi_ap_config_t {
    let (password, auth_mode) = match config.credential() {
        ApCredential::Open => (&[][..], esp8266_wifi_sys::WIFI_AUTH_OPEN),
        ApCredential::Wpa2(passphrase) => {
            (passphrase.as_bytes(), esp8266_wifi_sys::WIFI_AUTH_WPA2_PSK)
        }
    };

    let mut raw: esp8266_wifi_sys::wifi_ap_config_t = unsafe { core::mem::zeroed() };
    raw.ssid[..config.ssid().as_bytes().len()].copy_from_slice(config.ssid().as_bytes());
    raw.ssid_len = config.ssid().as_bytes().len() as u8;
    raw.password[..password.len()].copy_from_slice(password);
    raw.channel = config.channel().get();
    raw.authmode = auth_mode;
    raw.max_connection = config.max_clients().get();
    raw.beacon_interval = config.beacon_interval().millis();
    raw
}

#[inline(always)]
#[cfg(target_arch = "xtensa")]
pub(super) const fn raw_mode(mode: RawMode) -> esp8266_wifi_sys::wifi_mode_t {
    match mode {
        RawMode::Sta => esp8266_wifi_sys::WIFI_MODE_STA,
        RawMode::Ap => esp8266_wifi_sys::WIFI_MODE_AP,
        RawMode::ApSta => esp8266_wifi_sys::WIFI_MODE_APSTA,
    }
}

#[cfg(target_arch = "xtensa")]
pub(super) struct RawScanConfigBuffers {
    ssid: [u8; 33],
    bssid: [u8; 6],
    has_ssid: bool,
    has_bssid: bool,
}

#[cfg(target_arch = "xtensa")]
impl RawScanConfigBuffers {
    pub(super) fn from_options(options: &ScanOptions) -> Self {
        let mut buffers = Self {
            ssid: [0; 33],
            bssid: [0; 6],
            has_ssid: false,
            has_bssid: false,
        };
        if let Some(ssid) = options.target_ssid() {
            buffers.ssid[..ssid.as_bytes().len()].copy_from_slice(ssid.as_bytes());
            buffers.has_ssid = true;
        }
        if let Some(bssid) = options.target_bssid() {
            buffers.bssid = bssid;
            buffers.has_bssid = true;
        }
        buffers
    }

    pub(super) fn as_raw(&mut self, options: &ScanOptions) -> esp8266_wifi_sys::wifi_scan_config_t {
        let timings = options.scan_timings();
        esp8266_wifi_sys::wifi_scan_config_t {
            ssid: if self.has_ssid {
                self.ssid.as_mut_ptr()
            } else {
                core::ptr::null_mut()
            },
            bssid: if self.has_bssid {
                self.bssid.as_mut_ptr()
            } else {
                core::ptr::null_mut()
            },
            channel: options.target_channel().map_or(0, Channel::get),
            show_hidden: options.shows_hidden(),
            scan_type: match options.scan_kind() {
                ScanType::Active => esp8266_wifi_sys::WIFI_SCAN_TYPE_ACTIVE,
                ScanType::Passive => esp8266_wifi_sys::WIFI_SCAN_TYPE_PASSIVE,
            },
            scan_time: match options.scan_kind() {
                ScanType::Active => esp8266_wifi_sys::wifi_scan_time_t {
                    active: esp8266_wifi_sys::wifi_active_scan_time_t {
                        min: timings.active_min_ms(),
                        max: timings.active_max_ms(),
                    },
                },
                ScanType::Passive => esp8266_wifi_sys::wifi_scan_time_t {
                    passive: timings.passive_ms(),
                },
            },
        }
    }
}

#[cfg(feature = "wifi-promiscuous")]
pub(super) fn map_promiscuous_filter(filter: PromiscuousFilter) -> u32 {
    if filter.is_empty() {
        return 0;
    }
    if filter == PromiscuousFilter::DATA {
        return esp8266_wifi_sys::WIFI_PROMIS_FILTER_MASK_DATA;
    }

    // ESP8266 SDK packet-type masks are not reliable for every narrow subset.
    // Data-only capture is stable with the hardware mask; other public filters
    // use broad SDK capture and exact Rust-side filtering in the callback.
    esp8266_wifi_sys::WIFI_PROMIS_FILTER_MASK_MGMT
        | esp8266_wifi_sys::WIFI_PROMIS_FILTER_MASK_CTRL
        | esp8266_wifi_sys::WIFI_PROMIS_FILTER_MASK_DATA
        | esp8266_wifi_sys::WIFI_PROMIS_FILTER_MASK_MISC
}

#[cfg(feature = "wifi-promiscuous")]
#[cfg(target_arch = "xtensa")]
pub(super) fn map_promiscuous_control_filter(filter: PromiscuousControlFilter) -> u32 {
    if filter.contains_all(PromiscuousControlFilter::all()) {
        return esp8266_wifi_sys::WIFI_PROMIS_CTRL_FILTER_MASK_ALL;
    }

    let mut mask = 0;
    if filter.contains_all(PromiscuousControlFilter::WRAPPER) {
        mask |= esp8266_wifi_sys::WIFI_PROMIS_CTRL_FILTER_MASK_WRAPPER;
    }
    if filter.contains_all(PromiscuousControlFilter::BLOCK_ACK_REQUEST) {
        mask |= esp8266_wifi_sys::WIFI_PROMIS_CTRL_FILTER_MASK_BAR;
    }
    if filter.contains_all(PromiscuousControlFilter::BLOCK_ACK) {
        mask |= esp8266_wifi_sys::WIFI_PROMIS_CTRL_FILTER_MASK_BA;
    }
    if filter.contains_all(PromiscuousControlFilter::PS_POLL) {
        mask |= esp8266_wifi_sys::WIFI_PROMIS_CTRL_FILTER_MASK_PSPOLL;
    }
    if filter.contains_all(PromiscuousControlFilter::RTS) {
        mask |= esp8266_wifi_sys::WIFI_PROMIS_CTRL_FILTER_MASK_RTS;
    }
    if filter.contains_all(PromiscuousControlFilter::CTS) {
        mask |= esp8266_wifi_sys::WIFI_PROMIS_CTRL_FILTER_MASK_CTS;
    }
    if filter.contains_all(PromiscuousControlFilter::ACK) {
        mask |= esp8266_wifi_sys::WIFI_PROMIS_CTRL_FILTER_MASK_ACK;
    }
    if filter.contains_all(PromiscuousControlFilter::CF_END) {
        mask |= esp8266_wifi_sys::WIFI_PROMIS_CTRL_FILTER_MASK_CFEND;
    }
    if filter.contains_all(PromiscuousControlFilter::CF_END_ACK) {
        mask |= esp8266_wifi_sys::WIFI_PROMIS_CTRL_FILTER_MASK_CFENDACK;
    }
    mask
}

#[cfg(feature = "wifi-promiscuous")]
#[cfg(target_arch = "xtensa")]
pub(super) fn map_promiscuous_packet_type(
    packet_type: esp8266_wifi_sys::wifi_promiscuous_pkt_type_t,
) -> PromiscuousPacketType {
    match packet_type {
        esp8266_wifi_sys::WIFI_PKT_MGMT => PromiscuousPacketType::Management,
        esp8266_wifi_sys::WIFI_PKT_CTRL => PromiscuousPacketType::Control,
        esp8266_wifi_sys::WIFI_PKT_DATA => PromiscuousPacketType::Data,
        _ => PromiscuousPacketType::Misc,
    }
}

#[cfg(feature = "wifi-esp-now")]
#[cfg(target_arch = "xtensa")]
fn map_esp_now_interface(interface: EspNowInterface) -> esp8266_wifi_sys::wifi_interface_t {
    match interface {
        EspNowInterface::Station => esp8266_wifi_sys::WIFI_IF_STA,
        EspNowInterface::AccessPoint => esp8266_wifi_sys::WIFI_IF_AP,
    }
}

#[cfg(feature = "wifi-wps")]
#[cfg(target_arch = "xtensa")]
fn map_wps_mode(mode: WpsMode) -> esp8266_wifi_sys::wps_type_t {
    match mode {
        WpsMode::Pbc => esp8266_wifi_sys::WPS_TYPE_PBC,
        WpsMode::Pin => esp8266_wifi_sys::WPS_TYPE_PIN,
    }
}

#[cfg(feature = "wifi-wps")]
#[cfg(target_arch = "xtensa")]
pub(super) fn raw_wps_config(config: &WpsConfig<'_>) -> esp8266_wifi_sys::esp_wps_config_t {
    let mut raw: esp8266_wifi_sys::esp_wps_config_t = unsafe { core::mem::zeroed() };
    raw.wps_type = map_wps_mode(config.mode());
    copy_wps_field(
        &mut raw.factory_info.manufacturer,
        config.factory_info().manufacturer_bytes(),
    );
    copy_wps_field(
        &mut raw.factory_info.model_number,
        config.factory_info().model_number_bytes(),
    );
    copy_wps_field(
        &mut raw.factory_info.model_name,
        config.factory_info().model_name_bytes(),
    );
    copy_wps_field(
        &mut raw.factory_info.device_name,
        config.factory_info().device_name_bytes(),
    );
    raw
}

#[cfg(feature = "wifi-wps")]
#[cfg(target_arch = "xtensa")]
fn copy_wps_field(destination: &mut [core::ffi::c_char], source: &[u8]) {
    for (dst, src) in destination.iter_mut().zip(source) {
        *dst = *src as core::ffi::c_char;
    }
}

#[cfg(feature = "wifi-smartconfig")]
#[cfg(target_arch = "xtensa")]
pub(super) fn map_smartconfig_type(kind: SmartConfigType) -> esp8266_wifi_sys::smartconfig_type_t {
    match kind {
        SmartConfigType::EspTouch => esp8266_wifi_sys::SC_TYPE_ESPTOUCH,
        SmartConfigType::AirKiss => esp8266_wifi_sys::SC_TYPE_AIRKISS,
        SmartConfigType::EspTouchAirKiss => esp8266_wifi_sys::SC_TYPE_ESPTOUCH_AIRKISS,
        SmartConfigType::EspTouchV2 => esp8266_wifi_sys::SC_TYPE_ESPTOUCH_V2,
    }
}

#[cfg(feature = "wifi-smartconfig")]
#[cfg(target_arch = "xtensa")]
pub(super) fn map_smartconfig_type_from_raw(
    raw: esp8266_wifi_sys::smartconfig_type_t,
) -> SmartConfigType {
    match raw {
        esp8266_wifi_sys::SC_TYPE_AIRKISS => SmartConfigType::AirKiss,
        esp8266_wifi_sys::SC_TYPE_ESPTOUCH_AIRKISS => SmartConfigType::EspTouchAirKiss,
        esp8266_wifi_sys::SC_TYPE_ESPTOUCH_V2 => SmartConfigType::EspTouchV2,
        _ => SmartConfigType::EspTouch,
    }
}

#[cfg(feature = "wifi-smartconfig")]
#[cfg(target_arch = "xtensa")]
pub(super) fn nul_terminated_len(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len())
}

#[cfg(feature = "wifi-esp-now")]
#[cfg(target_arch = "xtensa")]
pub(super) fn raw_esp_now_peer(peer: &EspNowPeerConfig) -> esp8266_wifi_sys::esp_now_peer_info_t {
    let mut raw: esp8266_wifi_sys::esp_now_peer_info_t = unsafe { core::mem::zeroed() };
    raw.peer_addr = *peer.peer_addr();
    raw.channel = peer.channel_value();
    raw.ifidx = map_esp_now_interface(peer.interface_value());
    raw.encrypt = peer.is_encrypted();
    if let Some(lmk) = peer.lmk() {
        raw.lmk = *lmk;
    }
    raw
}

#[cfg(test)]
mod tests {
    use super::super::StationCredential;
    use super::*;

    #[cfg(feature = "wifi-promiscuous")]
    #[test]
    fn promiscuous_all_uses_only_documented_packet_bits() {
        assert_eq!(
            map_promiscuous_filter(PromiscuousFilter::all()),
            esp8266_wifi_sys::WIFI_PROMIS_FILTER_MASK_MGMT
                | esp8266_wifi_sys::WIFI_PROMIS_FILTER_MASK_CTRL
                | esp8266_wifi_sys::WIFI_PROMIS_FILTER_MASK_DATA
                | esp8266_wifi_sys::WIFI_PROMIS_FILTER_MASK_MISC
        );
    }

    #[cfg(feature = "wifi-promiscuous")]
    #[test]
    fn promiscuous_control_requests_use_sdk_safe_broad_mask() {
        assert_eq!(
            map_promiscuous_filter(PromiscuousFilter::CONTROL),
            map_promiscuous_filter(PromiscuousFilter::all())
        );
    }

    #[cfg(feature = "wifi-promiscuous")]
    #[test]
    fn promiscuous_narrow_filters_use_rust_side_filtering() {
        let broad = map_promiscuous_filter(PromiscuousFilter::all());

        assert_eq!(map_promiscuous_filter(PromiscuousFilter::MANAGEMENT), broad);
        assert_eq!(map_promiscuous_filter(PromiscuousFilter::MISC), broad);
        assert_eq!(map_promiscuous_filter(PromiscuousFilter::empty()), 0);
    }

    #[cfg(feature = "wifi-promiscuous")]
    #[test]
    fn promiscuous_data_only_uses_stable_sdk_data_mask() {
        assert_eq!(
            map_promiscuous_filter(PromiscuousFilter::DATA),
            esp8266_wifi_sys::WIFI_PROMIS_FILTER_MASK_DATA
        );
    }

    #[test]
    fn station_channel_hint_uses_fast_scan() {
        let config = ClientConfig::new(
            super::super::Ssid::try_from("esp8266-hal-sta").expect("ssid"),
            StationCredential::open(),
        )
        .with_channel(Channel::try_from(6).expect("channel"));

        let raw = raw_client_config(&config);

        assert_eq!(raw.channel, 6);
        assert_eq!(raw.scan_method, esp8266_wifi_sys::WIFI_FAST_SCAN);
    }

    #[test]
    fn station_power_save_maps_to_sdk_mode() {
        assert_eq!(
            raw_station_power_save(StationPowerSave::None),
            esp8266_wifi_sys::WIFI_PS_NONE
        );
        assert_eq!(
            raw_station_power_save(StationPowerSave::MinModem),
            esp8266_wifi_sys::WIFI_PS_MIN_MODEM
        );
        assert_eq!(
            raw_station_power_save(StationPowerSave::MaxModem),
            esp8266_wifi_sys::WIFI_PS_MAX_MODEM
        );
    }

    #[test]
    fn init_config_keeps_blob_coupled_defaults_private() {
        let config = InitConfig::new(20, 9, 8).unwrap();
        let raw = raw_init_config(&config);

        assert_eq!(raw.rx_buf_num, 20);
        assert_eq!(raw.rx_pkt_num, 9);
        assert_eq!(raw.tx_buf_num, 8);
        assert_eq!(raw.left_continuous_rx_buf_num, 16);
        assert_eq!(raw.rx_max_single_pkt_len, 1_076);
        assert_eq!(raw.magic, esp8266_wifi_sys::WIFI_INIT_CONFIG_MAGIC);
    }
}

/// SDK scan inputs remain stationary until scan completion/cancellation.
#[cfg(target_arch = "xtensa")]
pub(super) struct ScanStorage {
    buffers: RawScanConfigBuffers,
    raw: esp8266_wifi_sys::wifi_scan_config_t,
}
#[cfg(target_arch = "xtensa")]
impl ScanStorage {
    pub(super) fn new() -> Self {
        Self {
            buffers: RawScanConfigBuffers::from_options(&ScanOptions::all()),
            raw: unsafe { core::mem::zeroed() },
        }
    }
    pub(super) fn prepare(
        &mut self,
        options: &ScanOptions,
    ) -> *const esp8266_wifi_sys::wifi_scan_config_t {
        self.buffers = RawScanConfigBuffers::from_options(options);
        self.raw = self.buffers.as_raw(options);
        &self.raw
    }
}

#[cfg(all(target_arch = "xtensa", feature = "wifi-smartconfig"))]
pub(super) struct SmartConfigStorage {
    key: [i8; 17],
    raw: esp8266_wifi_sys::smartconfig_start_config_t,
}
#[cfg(all(target_arch = "xtensa", feature = "wifi-smartconfig"))]
impl SmartConfigStorage {
    pub(super) fn new() -> Self {
        Self {
            key: [0; 17],
            raw: esp8266_wifi_sys::smartconfig_start_config_t {
                enable_log: true,
                esp_touch_v2_enable_crypt: false,
                esp_touch_v2_key: core::ptr::null_mut(),
            },
        }
    }
    pub(super) fn prepare(
        &mut self,
        config: &super::SmartConfigStartConfig,
    ) -> *const esp8266_wifi_sys::smartconfig_start_config_t {
        self.key = [0; 17];
        if let Some(value) = config.esp_touch_v2_key_value() {
            for (dst, src) in self.key.iter_mut().zip(value) {
                *dst = src as i8;
            }
        }
        self.raw.esp_touch_v2_enable_crypt = config.esp_touch_v2_key_value().is_some();
        self.raw.esp_touch_v2_key = if self.raw.esp_touch_v2_enable_crypt {
            self.key.as_mut_ptr()
        } else {
            core::ptr::null_mut()
        };
        &self.raw
    }
}
