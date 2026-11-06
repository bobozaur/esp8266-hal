#[cfg(all(feature = "wifi-promiscuous", target_arch = "xtensa"))]
use super::PromiscuousControlFilter;
#[cfg(all(feature = "wifi-smartconfig", target_arch = "xtensa"))]
use super::SmartConfigType;
#[cfg(all(feature = "wifi-esp-now", target_arch = "xtensa"))]
use super::{feature::EspNowInterface, EspNowPeerConfig};
#[cfg(feature = "wifi-promiscuous")]
use super::{
    feature::PROMISCUOUS_MAX_PAYLOAD_PREFIX, PromiscuousFilter, PromiscuousFrameInfo,
    PromiscuousPacketType, PromiscuousRxFrame,
};
#[cfg(target_arch = "xtensa")]
use super::{AccessPointConfig, ApCredential, RawMode};
use super::{
    Channel, ClientConfig, DriverError, InitConfig, ScanOptions, ScanType, StationPowerSave,
};
#[cfg(all(feature = "wifi-wps", target_arch = "xtensa"))]
use super::{WpsConfig, WpsMode};

#[cfg(target_arch = "xtensa")]
extern "C" {
    fn esp_event_send(event: *mut esp8266_wifi_sys::system_event_t) -> esp8266_wifi_sys::esp_err_t;
}

pub(super) fn map_internal_tx_result(result: i32) -> Result<(), DriverError> {
    match result {
        0 => Ok(()),
        // SDK da7ce37be424d71874241ca9f17cf8da0e3cdb70, libnet80211.a:
        // ieee80211_output_pbuf returns descriptor exhaustion as byte-valued
        // ERR_MEM: movi a12,-1; extui a2,a12,0,8. The Rust static-buffer
        // fallback instead returns signed -1. Both require retaining the frame.
        -1 | 255 => Err(DriverError::WouldBlock),
        esp8266_wifi_sys::ESP_ERR_NO_MEM => Err(DriverError::OutOfMemory),
        code => Err(DriverError::Sdk(code)),
    }
}

/// Copy the initialized prefix of a native PP capture into an owned frame.
///
/// # Safety
/// `packet` must be the live callback pointer from the pinned SDK's
/// `ppProcessRxPktHdr`: eight bytes into an allocation from our C allocator,
/// followed by the 12-byte RX control record and the initialized capture prefix.
/// The allocation must remain live and unmodified throughout this call.
#[cfg(feature = "wifi-promiscuous")]
pub(super) unsafe fn promiscuous_frame(
    packet: *const esp8266_wifi_sys::wifi_promiscuous_pkt_t,
    packet_type: PromiscuousPacketType,
    fallback_channel: u8,
) -> Option<PromiscuousRxFrame> {
    // SAFETY: every PP capture producer initializes the RX control record.
    let rx_ctrl = unsafe { core::ptr::addr_of!((*packet).rx_ctrl).read() };
    let original_len = if rx_ctrl.sig_mode() != 0 {
        rx_ctrl.ht_length()
    } else {
        rx_ctrl.legacy_length()
    };
    let channel = if (1..=14).contains(&rx_ctrl.channel()) {
        rx_ctrl.channel()
    } else {
        fallback_channel
    };
    if !(1..=14).contains(&channel) {
        return None;
    }

    let payload_len = if packet_type == PromiscuousPacketType::Misc {
        0
    } else {
        const PP_ENVELOPE_SIZE: usize = 8;
        const RX_CONTROL_SIZE: usize = core::mem::size_of::<esp8266_wifi_sys::wifi_pkt_rx_ctrl_t>();
        // SAFETY: all four wDev_ProcessFiq event-9 producers pass allocation+8
        // to the callback. Retain the original raw pointer's allocation provenance.
        let allocation = unsafe { packet.cast::<u8>().sub(PP_ENVELOPE_SIZE) };
        let available = unsafe { crate::wifi::compat::memory::allocation_size(allocation.cast()) }
            .saturating_sub(PP_ENVELOPE_SIZE + RX_CONTROL_SIZE);
        // Nonaggregated captures allocate exactly their initialized prefix.
        // Aggregates copy only 40 payload bytes, followed by an MPDU length table.
        // Queued captures can retain SmartConfig's old 24/40-byte prefix even
        // after the current capture settings have been restored.
        let capture_limit = if rx_ctrl.aggregation() {
            40
        } else {
            PROMISCUOUS_MAX_PAYLOAD_PREFIX
        };
        usize::from(original_len).min(available).min(capture_limit)
    };
    // SAFETY: the allocation-specific bound above excludes both uncaptured
    // bytes and the aggregate trailer. The raw field pointer retains the
    // allocation provenance and remains valid until the owned copy is complete.
    let payload = unsafe {
        core::slice::from_raw_parts(
            core::ptr::addr_of!((*packet).payload).cast::<u8>(),
            payload_len,
        )
    };
    PromiscuousRxFrame::new(
        PromiscuousFrameInfo {
            rssi: rx_ctrl.rssi(),
            channel,
            packet_type,
            original_len,
        },
        payload,
    )
    .ok()
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
        // The native Wi-Fi configuration persistence model is unavailable;
        // the Rust NVS shim supports only the startup MAC record.
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

pub(super) struct RawScanConfigBuffers {
    ssid: [u8; 33],
    bssid: [u8; 6],
    has_ssid: bool,
    has_bssid: bool,
}

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
        let (scan_type, scan_time) = match options.scan_kind() {
            ScanType::Active { min_ms, max_ms } => (
                esp8266_wifi_sys::WIFI_SCAN_TYPE_ACTIVE,
                esp8266_wifi_sys::wifi_scan_time_t {
                    active: esp8266_wifi_sys::wifi_active_scan_time_t {
                        min: min_ms,
                        max: max_ms,
                    },
                },
            ),
            ScanType::Passive { dwell_ms } => (
                esp8266_wifi_sys::WIFI_SCAN_TYPE_PASSIVE,
                esp8266_wifi_sys::wifi_scan_time_t { passive: dwell_ms },
            ),
        };
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
            scan_type,
            scan_time,
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
    if let Some(lmk) = peer.lmk() {
        raw.encrypt = true;
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
    fn native_capture_uses_each_allocation_prefix_and_excludes_aggregate_trailers() {
        use crate::wifi::compat::memory;

        let _guard = memory::tests::allocator_guard();
        for (prefix, aggregated, packet_type, ht) in [
            (24, false, PromiscuousPacketType::Data, false),
            (40, false, PromiscuousPacketType::Management, true),
            (112, false, PromiscuousPacketType::Control, false),
            (200, false, PromiscuousPacketType::Data, true),
            (40, true, PromiscuousPacketType::Data, true),
            (0, false, PromiscuousPacketType::Misc, false),
        ] {
            let original_len = 1200u16;
            let mut rx_ctrl = esp8266_wifi_sys::wifi_pkt_rx_ctrl_t { _data: [0; 12] };
            rx_ctrl._data[0] = (-41i8) as u8;
            if ht {
                rx_ctrl._data[1] = 1 << 6;
                rx_ctrl._data[5..7].copy_from_slice(&original_len.to_le_bytes());
            } else {
                rx_ctrl._data[2..4].copy_from_slice(&original_len.to_le_bytes());
            }
            let allocation_len = if aggregated {
                rx_ctrl._data[7] = 1 << 3;
                rx_ctrl._data[9] = 3;
                64 + 2 * usize::from(rx_ctrl.ampdu_cnt())
            } else {
                20 + prefix
            };
            // SAFETY: this is the real C allocation boundary used by the PP
            // producers. Initialize exactly their RX control and payload prefix;
            // leave the aggregate trailer uninitialized to detect any overread.
            let allocation = unsafe { memory::malloc(allocation_len) }.cast::<u8>();
            assert!(!allocation.is_null());
            let packet =
                unsafe { allocation.add(8) }.cast::<esp8266_wifi_sys::wifi_promiscuous_pkt_t>();
            unsafe {
                core::ptr::addr_of_mut!((*packet).rx_ctrl).write(rx_ctrl);
                allocation.add(20).write_bytes(0xa5, prefix);
            }
            let frame = unsafe { promiscuous_frame(packet, packet_type, 6) }.unwrap();
            unsafe { memory::free(allocation.cast()) };

            assert_eq!(frame.info().rssi, -41);
            assert_eq!(frame.info().channel, 6);
            assert_eq!(frame.info().packet_type, packet_type);
            assert_eq!(frame.original_len(), usize::from(original_len));
            assert_eq!(frame.payload().len(), prefix.min(112));
            assert!(frame.payload().iter().all(|byte| *byte == 0xa5));
            assert!(frame.is_truncated());
        }
    }

    #[test]
    fn scan_method_selects_its_native_timing_union_and_preserves_defaults() {
        let mut buffers = RawScanConfigBuffers::from_options(&ScanOptions::all());
        for (options, expected) in [
            (ScanOptions::all(), (0, 0)),
            (
                ScanOptions::all().scan_type(ScanType::Active {
                    min_ms: 200,
                    max_ms: 500,
                }),
                (200, 500),
            ),
        ] {
            let raw = buffers.as_raw(&options);
            assert_eq!(raw.scan_type, esp8266_wifi_sys::WIFI_SCAN_TYPE_ACTIVE);
            // SAFETY: the active discriminant above selects the initialized union field.
            let active = unsafe { raw.scan_time.active };
            assert_eq!((active.min, active.max), expected);
        }
        for dwell_ms in [0, 100] {
            let raw = buffers.as_raw(&ScanOptions::all().scan_type(ScanType::Passive { dwell_ms }));
            assert_eq!(raw.scan_type, esp8266_wifi_sys::WIFI_SCAN_TYPE_PASSIVE);
            // SAFETY: the passive discriminant above selects the initialized union field.
            assert_eq!(unsafe { raw.scan_time.passive }, dwell_ms);
        }
    }

    #[test]
    fn internal_tx_preserves_native_byte_abi_backpressure() {
        assert_eq!(map_internal_tx_result(0), Ok(()));
        for result in [-1, 255] {
            assert_eq!(map_internal_tx_result(result), Err(DriverError::WouldBlock));
        }
        assert_eq!(
            map_internal_tx_result(esp8266_wifi_sys::ESP_ERR_NO_MEM),
            Err(DriverError::OutOfMemory)
        );
    }

    #[test]
    fn internal_tx_preserves_other_native_and_esp_errors() {
        // ESP_ERR_WIFI_WOULD_BLOCK is not a proven Ethernet TX result. Preserve
        // it unchanged rather than extending the separate esp_err_t mapping.
        for result in [
            -16,
            240,
            241,
            511,
            esp8266_wifi_sys::ESP_ERR_WIFI_WOULD_BLOCK,
        ] {
            assert_eq!(
                map_internal_tx_result(result),
                Err(DriverError::Sdk(result))
            );
        }
    }

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
