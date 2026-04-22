#[cfg(feature = "wifi-smartconfig")]
use super::SmartConfigType;
use super::{
    AccessPointConfig, Channel, ClientConfig, Credential, EspNowInterface, EspNowPeerConfig,
    InitConfig, PromiscuousControlFilter, PromiscuousFilter, PromiscuousPacketType, RawMode,
    ScanOptions, ScanType, StationPowerSave, WpsConfig, WpsMode,
};

#[inline(always)]
pub(super) fn map_init_config(config: &InitConfig) -> crate::wifi::controller::WifiInitConfig {
    let mut init = crate::wifi::controller::WifiInitConfig::default();
    init.rx_buf_num = config.rx_buf_num();
    init.rx_pkt_num = config.rx_pkt_num();
    init.tx_buf_num = config.tx_buf_num();
    init.left_continuous_rx_buf_num = config.rx_buf_num().min(init.left_continuous_rx_buf_num);
    init
}

#[inline(always)]
pub(super) fn map_client_config(config: &ClientConfig) -> crate::wifi::StaConfig {
    let (password, auth_threshold) = match config.credential() {
        Credential::Open => (&[][..], crate::wifi::AuthMode::Open),
        Credential::Wpa2(passphrase) => (passphrase.as_bytes(), crate::wifi::AuthMode::Wpa2Psk),
    };

    let mut raw = crate::wifi::StaConfig::from_validated_bytes(
        config.ssid().as_bytes(),
        password,
        auth_threshold,
    );
    raw.bssid = config.bssid();
    raw.channel = config.channel().map_or(0, Channel::get);
    raw.scan_method = if config.channel().is_some() {
        crate::wifi::config::ScanMethod::Fast
    } else {
        crate::wifi::config::ScanMethod::AllChannel
    };
    raw
}

#[inline(always)]
pub(super) const fn map_station_power_save(power_save: StationPowerSave) -> crate::wifi::PowerSave {
    match power_save {
        StationPowerSave::None => crate::wifi::PowerSave::None,
        StationPowerSave::MinModem => crate::wifi::PowerSave::MinModem,
        StationPowerSave::MaxModem => crate::wifi::PowerSave::MaxModem,
    }
}

#[inline(always)]
pub(super) fn map_ap_config(config: &AccessPointConfig) -> crate::wifi::ApConfig {
    let (password, auth_mode) = match config.credential() {
        Credential::Open => (&[][..], crate::wifi::AuthMode::Open),
        Credential::Wpa2(passphrase) => (passphrase.as_bytes(), crate::wifi::AuthMode::Wpa2Psk),
    };

    crate::wifi::ApConfig::from_validated_bytes(
        config.ssid().as_bytes(),
        password,
        config.channel().get(),
        config.max_clients().get(),
        config.beacon_interval().millis(),
        auth_mode,
    )
}

#[inline(always)]
pub(super) fn map_mode(mode: RawMode) -> crate::wifi::WifiMode {
    match mode {
        RawMode::Sta => crate::wifi::WifiMode::Sta,
        RawMode::Ap => crate::wifi::WifiMode::Ap,
        RawMode::ApSta => crate::wifi::WifiMode::ApSta,
    }
}

#[inline(always)]
pub(super) fn map_scan_options(options: &ScanOptions) -> crate::wifi::scan::ScanConfig {
    let mut ssid = None;
    if let Some(target) = options.target_ssid() {
        let mut bytes = heapless::Vec::new();
        let _ = bytes.extend_from_slice(target.as_bytes());
        ssid = Some(bytes);
    }

    let timings = options.scan_timings();
    crate::wifi::scan::ScanConfig {
        ssid,
        bssid: options.target_bssid(),
        channel: options.target_channel().map_or(0, Channel::get),
        show_hidden: options.shows_hidden(),
        scan_type: match options.scan_kind() {
            ScanType::Active => crate::wifi::scan::ScanType::Active,
            ScanType::Passive => crate::wifi::scan::ScanType::Passive,
        },
        active_scan_min_ms: timings.active_min_ms(),
        active_scan_max_ms: timings.active_max_ms(),
        passive_scan_ms: timings.passive_ms(),
    }
}

pub(super) fn map_promiscuous_filter(filter: PromiscuousFilter) -> u32 {
    if filter.contains(PromiscuousPacketType::Control) {
        return esp8266_wifi_sys::WIFI_PROMIS_FILTER_MASK_MGMT
            | esp8266_wifi_sys::WIFI_PROMIS_FILTER_MASK_CTRL
            | esp8266_wifi_sys::WIFI_PROMIS_FILTER_MASK_DATA
            | esp8266_wifi_sys::WIFI_PROMIS_FILTER_MASK_MISC;
    }

    let mut mask = 0;
    if filter.contains(PromiscuousPacketType::Management) {
        mask |= esp8266_wifi_sys::WIFI_PROMIS_FILTER_MASK_MGMT;
    }
    if filter.contains(PromiscuousPacketType::Control) {
        mask |= esp8266_wifi_sys::WIFI_PROMIS_FILTER_MASK_CTRL;
    }
    if filter.contains(PromiscuousPacketType::Data) {
        mask |= esp8266_wifi_sys::WIFI_PROMIS_FILTER_MASK_DATA;
    }
    if filter.contains(PromiscuousPacketType::Misc) {
        mask |= esp8266_wifi_sys::WIFI_PROMIS_FILTER_MASK_MISC;
    }
    mask
}

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

fn map_esp_now_interface(interface: EspNowInterface) -> esp8266_wifi_sys::wifi_interface_t {
    match interface {
        EspNowInterface::Station => esp8266_wifi_sys::WIFI_IF_STA,
        EspNowInterface::AccessPoint => esp8266_wifi_sys::WIFI_IF_AP,
    }
}

fn map_wps_mode(mode: WpsMode) -> esp8266_wifi_sys::wps_type_t {
    match mode {
        WpsMode::Pbc => esp8266_wifi_sys::WPS_TYPE_PBC,
        WpsMode::Pin => esp8266_wifi_sys::WPS_TYPE_PIN,
    }
}

pub(super) fn raw_wps_config(config: &WpsConfig) -> esp8266_wifi_sys::esp_wps_config_t {
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

fn copy_wps_field(destination: &mut [core::ffi::c_char], source: &[u8]) {
    let len = source.len().min(destination.len().saturating_sub(1));
    for (dst, src) in destination.iter_mut().zip(&source[..len]) {
        *dst = *src as core::ffi::c_char;
    }
}

#[cfg(feature = "wifi-smartconfig")]
pub(super) fn map_smartconfig_type(kind: SmartConfigType) -> esp8266_wifi_sys::smartconfig_type_t {
    match kind {
        SmartConfigType::EspTouch => esp8266_wifi_sys::SC_TYPE_ESPTOUCH,
        SmartConfigType::AirKiss => esp8266_wifi_sys::SC_TYPE_AIRKISS,
        SmartConfigType::EspTouchAirKiss => esp8266_wifi_sys::SC_TYPE_ESPTOUCH_AIRKISS,
        SmartConfigType::EspTouchV2 => esp8266_wifi_sys::SC_TYPE_ESPTOUCH_V2,
    }
}

#[cfg(feature = "wifi-smartconfig")]
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
pub(super) fn nul_terminated_len(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len())
}

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
    use super::*;

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

    #[test]
    fn promiscuous_control_requests_use_sdk_safe_broad_mask() {
        assert_eq!(
            map_promiscuous_filter(PromiscuousFilter::CONTROL),
            map_promiscuous_filter(PromiscuousFilter::all())
        );
    }

    #[test]
    fn station_channel_hint_uses_fast_scan() {
        let config = ClientConfig::new(
            super::super::Ssid::try_from("esp8266-hal-sta").expect("ssid"),
            Credential::Open,
        )
        .with_channel(Channel::try_from(6).expect("channel"));

        let raw = map_client_config(&config);

        assert_eq!(raw.channel, 6);
        assert_eq!(
            raw.scan_method,
            crate::wifi::config::ScanMethod::Fast.to_raw()
        );
    }

    #[test]
    fn station_power_save_maps_to_sdk_mode() {
        assert_eq!(
            map_station_power_save(StationPowerSave::None),
            crate::wifi::PowerSave::None
        );
        assert_eq!(
            map_station_power_save(StationPowerSave::MinModem),
            crate::wifi::PowerSave::MinModem
        );
        assert_eq!(
            map_station_power_save(StationPowerSave::MaxModem),
            crate::wifi::PowerSave::MaxModem
        );
    }
}
