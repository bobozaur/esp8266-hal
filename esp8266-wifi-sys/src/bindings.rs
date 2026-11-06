// ESP8266 ABI declarations narrowed from rust-bindgen 0.70.1 output for
// SDK da7ce37be424d71874241ca9f17cf8da0e3cdb70. Scalar C bitfields use
// little-endian u32 storage; independent target-header checks live in tests/.

pub const WIFI_PROMIS_FILTER_MASK_MGMT: u32 = 1;
pub const WIFI_PROMIS_FILTER_MASK_CTRL: u32 = 2;
pub const WIFI_PROMIS_FILTER_MASK_DATA: u32 = 4;
pub const WIFI_PROMIS_FILTER_MASK_MISC: u32 = 8;
pub const WIFI_PROMIS_CTRL_FILTER_MASK_ALL: u32 = 4286578688;
pub const WIFI_PROMIS_CTRL_FILTER_MASK_WRAPPER: u32 = 8388608;
pub const WIFI_PROMIS_CTRL_FILTER_MASK_BAR: u32 = 16777216;
pub const WIFI_PROMIS_CTRL_FILTER_MASK_BA: u32 = 33554432;
pub const WIFI_PROMIS_CTRL_FILTER_MASK_PSPOLL: u32 = 67108864;
pub const WIFI_PROMIS_CTRL_FILTER_MASK_RTS: u32 = 134217728;
pub const WIFI_PROMIS_CTRL_FILTER_MASK_CTS: u32 = 268435456;
pub const WIFI_PROMIS_CTRL_FILTER_MASK_ACK: u32 = 536870912;
pub const WIFI_PROMIS_CTRL_FILTER_MASK_CFEND: u32 = 1073741824;
pub const WIFI_PROMIS_CTRL_FILTER_MASK_CFENDACK: u32 = 2147483648;
pub type esp_err_t = i32;
pub type esp_event_base_t = *const core::ffi::c_char;
pub const ESP_IF_WIFI_STA: esp_interface_t = 0;
pub const ESP_IF_WIFI_AP: esp_interface_t = 1;
pub type esp_interface_t = core::ffi::c_uint;
pub const WIFI_MODE_STA: wifi_mode_t = 1;
pub const WIFI_MODE_AP: wifi_mode_t = 2;
pub const WIFI_MODE_APSTA: wifi_mode_t = 3;
pub type wifi_mode_t = core::ffi::c_uint;
pub use self::esp_interface_t as wifi_interface_t;
pub type wifi_country_policy_t = core::ffi::c_uint;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct wifi_country_t {
    pub cc: [core::ffi::c_char; 3usize],
    pub schan: u8,
    pub nchan: u8,
    pub max_tx_power: i8,
    pub policy: wifi_country_policy_t,
}

pub const WIFI_AUTH_OPEN: wifi_auth_mode_t = 0;
pub const WIFI_AUTH_WPA2_PSK: wifi_auth_mode_t = 3;
pub type wifi_auth_mode_t = core::ffi::c_uint;
pub const WIFI_SECOND_CHAN_NONE: wifi_second_chan_t = 0;
pub type wifi_second_chan_t = core::ffi::c_uint;
pub const WIFI_SCAN_TYPE_ACTIVE: wifi_scan_type_t = 0;
pub const WIFI_SCAN_TYPE_PASSIVE: wifi_scan_type_t = 1;
pub type wifi_scan_type_t = core::ffi::c_uint;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct wifi_active_scan_time_t {
    pub min: u32,
    pub max: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub union wifi_scan_time_t {
    pub active: wifi_active_scan_time_t,
    pub passive: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct wifi_scan_config_t {
    pub ssid: *mut u8,
    pub bssid: *mut u8,
    pub channel: u8,
    pub show_hidden: bool,
    pub scan_type: wifi_scan_type_t,
    pub scan_time: wifi_scan_time_t,
}

pub type wifi_cipher_type_t = core::ffi::c_uint;
pub type wifi_ant_t = core::ffi::c_uint;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct wifi_ap_record_t {
    pub bssid: [u8; 6usize],
    pub ssid: [u8; 33usize],
    pub primary: u8,
    pub second: wifi_second_chan_t,
    pub rssi: i8,
    pub freq_offset: i16,
    pub authmode: wifi_auth_mode_t,
    pub pairwise_cipher: wifi_cipher_type_t,
    pub group_cipher: wifi_cipher_type_t,
    pub ant: wifi_ant_t,
    /// Native little-endian C bitfield storage.
    pub flags: u32,
    pub country: wifi_country_t,
}

impl wifi_ap_record_t {
    #[inline]
    pub fn wps(&self) -> u32 {
        (self.flags >> 4) & 1
    }
}

pub const WIFI_FAST_SCAN: wifi_scan_method_t = 0;
pub const WIFI_ALL_CHANNEL_SCAN: wifi_scan_method_t = 1;
pub type wifi_scan_method_t = core::ffi::c_uint;
pub const WIFI_CONNECT_AP_BY_SIGNAL: wifi_sort_method_t = 0;
pub type wifi_sort_method_t = core::ffi::c_uint;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct wifi_fast_scan_threshold_t {
    pub rssi: i8,
    pub authmode: wifi_auth_mode_t,
}

pub const WIFI_PS_NONE: wifi_ps_type_t = 0;
pub const WIFI_PS_MIN_MODEM: wifi_ps_type_t = 1;
pub const WIFI_PS_MAX_MODEM: wifi_ps_type_t = 2;
pub type wifi_ps_type_t = core::ffi::c_uint;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct wifi_pmf_config_t {
    pub capable: bool,
    pub required: bool,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct wifi_ap_config_t {
    pub ssid: [u8; 32usize],
    pub password: [u8; 64usize],
    pub ssid_len: u8,
    pub channel: u8,
    pub authmode: wifi_auth_mode_t,
    pub ssid_hidden: u8,
    pub max_connection: u8,
    pub beacon_interval: u16,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct wifi_sta_config_t {
    pub ssid: [u8; 32usize],
    pub password: [u8; 64usize],
    pub scan_method: wifi_scan_method_t,
    pub bssid_set: bool,
    pub bssid: [u8; 6usize],
    pub channel: u8,
    pub listen_interval: u16,
    pub sort_method: wifi_sort_method_t,
    pub threshold: wifi_fast_scan_threshold_t,
    pub pmf_cfg: wifi_pmf_config_t,
    /// RM/BTM occupy bits 0/1 of byte 126; the reserved word starts at 128.
    /// A u32 here would incorrectly align those flags to byte 128.
    pub flags: [u8; 6],
}

#[repr(C)]
#[derive(Copy, Clone)]
pub union wifi_config_t {
    pub ap: wifi_ap_config_t,
    pub sta: wifi_sta_config_t,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct wifi_sta_info_t {
    pub mac: [u8; 6usize],
    /// The retained driver writes this extra byte, absent from its SDK header.
    pub _driver_data: u8,
    /// libnet80211.a:esp_wifi_ap_get_sta_list writes PHY bits at byte 7.
    pub flags: u8,
    pub _reserved: u32,
}

impl wifi_sta_info_t {
    #[inline]
    pub fn phy_11b(&self) -> u32 {
        u32::from(self.flags & 1)
    }
    #[inline]
    pub fn phy_11g(&self) -> u32 {
        u32::from((self.flags >> 1) & 1)
    }
    #[inline]
    pub fn phy_11n(&self) -> u32 {
        u32::from((self.flags >> 2) & 1)
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct wifi_sta_list_t {
    pub sta: [wifi_sta_info_t; 10usize],
    pub num: core::ffi::c_int,
}

pub const WIFI_STORAGE_RAM: wifi_storage_t = 1;
pub type wifi_storage_t = core::ffi::c_uint;
pub const WIFI_PKT_MGMT: wifi_promiscuous_pkt_type_t = 0;
pub const WIFI_PKT_CTRL: wifi_promiscuous_pkt_type_t = 1;
pub const WIFI_PKT_DATA: wifi_promiscuous_pkt_type_t = 2;
pub type wifi_promiscuous_pkt_type_t = core::ffi::c_uint;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct wifi_promiscuous_filter_t {
    pub filter_mask: u32,
}

pub type wifi_event_sta_wps_fail_reason_t = core::ffi::c_uint;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct wifi_event_sta_scan_done_t {
    pub status: u32,
    pub number: u8,
    pub scan_id: u8,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct wifi_event_sta_connected_t {
    pub ssid: [u8; 32usize],
    pub ssid_len: u8,
    pub bssid: [u8; 6usize],
    pub channel: u8,
    pub authmode: wifi_auth_mode_t,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct wifi_event_sta_wps_er_pin_t {
    pub pin_code: [u8; 8usize],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct wifi_event_sta_disconnected_t {
    pub ssid: [u8; 32usize],
    pub ssid_len: u8,
    pub bssid: [u8; 6usize],
    pub reason: u8,
}

pub const ESP_MAC_WIFI_STA: esp_mac_type_t = 0;
pub const ESP_MAC_WIFI_SOFTAP: esp_mac_type_t = 1;
pub type esp_mac_type_t = core::ffi::c_uint;
pub const SYSTEM_EVENT_SCAN_DONE: system_event_id_t = 1;
pub const SYSTEM_EVENT_STA_START: system_event_id_t = 2;
pub const SYSTEM_EVENT_STA_STOP: system_event_id_t = 3;
pub const SYSTEM_EVENT_STA_CONNECTED: system_event_id_t = 4;
pub const SYSTEM_EVENT_STA_DISCONNECTED: system_event_id_t = 5;
pub const SYSTEM_EVENT_STA_GOT_IP: system_event_id_t = 8;
pub const SYSTEM_EVENT_STA_WPS_ER_SUCCESS: system_event_id_t = 10;
pub const SYSTEM_EVENT_STA_WPS_ER_FAILED: system_event_id_t = 11;
pub const SYSTEM_EVENT_STA_WPS_ER_TIMEOUT: system_event_id_t = 12;
pub const SYSTEM_EVENT_STA_WPS_ER_PIN: system_event_id_t = 13;
pub const SYSTEM_EVENT_AP_START: system_event_id_t = 14;
pub const SYSTEM_EVENT_AP_STOP: system_event_id_t = 15;
pub type system_event_id_t = core::ffi::c_uint;
pub use self::wifi_event_sta_wps_fail_reason_t as system_event_sta_wps_fail_reason_t;
pub type system_event_sta_scan_done_t = wifi_event_sta_scan_done_t;
pub type system_event_sta_connected_t = wifi_event_sta_connected_t;
pub type system_event_sta_disconnected_t = wifi_event_sta_disconnected_t;
pub type system_event_sta_wps_er_pin_t = wifi_event_sta_wps_er_pin_t;

#[repr(C)]
#[derive(Copy, Clone)]
pub union system_event_info_t {
    pub connected: system_event_sta_connected_t,
    pub disconnected: system_event_sta_disconnected_t,
    pub scan_done: system_event_sta_scan_done_t,
    pub sta_er_pin: system_event_sta_wps_er_pin_t,
    pub sta_er_fail_reason: system_event_sta_wps_fail_reason_t,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct system_event_t {
    pub event_id: system_event_id_t,
    pub event_info: system_event_info_t,
}

pub type system_event_handler_t =
    ::core::option::Option<unsafe extern "C" fn(event: *mut system_event_t) -> esp_err_t>;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct wifi_init_config_t {
    pub event_handler: system_event_handler_t,
    pub osi_funcs: *mut core::ffi::c_void,
    pub qos_enable: u8,
    pub ampdu_rx_enable: u8,
    pub rx_ba_win: u8,
    pub rx_ampdu_buf_num: u8,
    pub rx_ampdu_buf_len: u32,
    pub rx_max_single_pkt_len: u32,
    pub rx_buf_len: u32,
    pub amsdu_rx_enable: u8,
    pub rx_buf_num: u8,
    pub rx_pkt_num: u8,
    pub left_continuous_rx_buf_num: u8,
    pub tx_buf_num: u8,
    pub nvs_enable: u8,
    pub nano_enable: u8,
    pub wpa3_sae_enable: u8,
    pub magic: u32,
}

pub type wifi_promiscuous_cb_t = ::core::option::Option<
    unsafe extern "C" fn(buf: *mut core::ffi::c_void, type_: wifi_promiscuous_pkt_type_t),
>;
pub const SC_TYPE_ESPTOUCH: smartconfig_type_t = 0;
pub const SC_TYPE_AIRKISS: smartconfig_type_t = 1;
pub const SC_TYPE_ESPTOUCH_AIRKISS: smartconfig_type_t = 2;
pub const SC_TYPE_ESPTOUCH_V2: smartconfig_type_t = 3;
pub type smartconfig_type_t = core::ffi::c_uint;
pub const SC_EVENT_SCAN_DONE: smartconfig_event_t = 0;
pub const SC_EVENT_FOUND_CHANNEL: smartconfig_event_t = 1;
pub const SC_EVENT_GOT_SSID_PSWD: smartconfig_event_t = 2;
pub const SC_EVENT_SEND_ACK_DONE: smartconfig_event_t = 3;
pub type smartconfig_event_t = core::ffi::c_uint;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct smartconfig_event_got_ssid_pswd_t {
    pub ssid: [u8; 32usize],
    pub password: [u8; 64usize],
    pub bssid_set: bool,
    pub bssid: [u8; 6usize],
    pub type_: smartconfig_type_t,
    pub token: u8,
    pub cellphone_ip: [u8; 4usize],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct smartconfig_start_config_t {
    pub enable_log: bool,
    pub esp_touch_v2_enable_crypt: bool,
    pub esp_touch_v2_key: *mut core::ffi::c_char,
}

pub type wifi_rxcb_t = ::core::option::Option<
    unsafe extern "C" fn(
        buffer: *mut core::ffi::c_void,
        len: u16,
        eb: *mut core::ffi::c_void,
    ) -> esp_err_t,
>;
pub const WPS_TYPE_PBC: wps_type = 1;
pub const WPS_TYPE_PIN: wps_type = 2;
pub type wps_type = core::ffi::c_uint;
pub use self::wps_type as wps_type_t;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct wps_factory_information_t {
    pub manufacturer: [core::ffi::c_char; 65usize],
    pub model_number: [core::ffi::c_char; 33usize],
    pub model_name: [core::ffi::c_char; 33usize],
    pub device_name: [core::ffi::c_char; 33usize],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct esp_wps_config_t {
    pub wps_type: wps_type_t,
    pub factory_info: wps_factory_information_t,
}

pub const ESP_NOW_SEND_SUCCESS: esp_now_send_status_t = 0;
pub type esp_now_send_status_t = core::ffi::c_uint;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct esp_now_peer_info {
    pub peer_addr: [u8; 6usize],
    pub lmk: [u8; 16usize],
    pub channel: u8,
    pub ifidx: wifi_interface_t,
    pub encrypt: bool,
    pub priv_: *mut core::ffi::c_void,
}

pub type esp_now_peer_info_t = esp_now_peer_info;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct esp_now_peer_num {
    pub total_num: core::ffi::c_int,
    pub encrypt_num: core::ffi::c_int,
}

pub type esp_now_peer_num_t = esp_now_peer_num;
pub type esp_now_recv_cb_t = ::core::option::Option<
    unsafe extern "C" fn(mac_addr: *const u8, data: *const u8, data_len: core::ffi::c_int),
>;
pub type esp_now_send_cb_t = ::core::option::Option<
    unsafe extern "C" fn(mac_addr: *const u8, status: esp_now_send_status_t),
>;

extern "C" {
    pub fn esp_wifi_init(config: *const wifi_init_config_t) -> esp_err_t;
    pub fn esp_wifi_deinit() -> esp_err_t;
    pub fn esp_wifi_set_mode(mode: wifi_mode_t) -> esp_err_t;
    pub fn esp_wifi_start() -> esp_err_t;
    pub fn esp_wifi_stop() -> esp_err_t;
    pub fn esp_wifi_connect() -> esp_err_t;
    pub fn esp_wifi_disconnect() -> esp_err_t;
    pub fn esp_wifi_deauth_sta(aid: u16) -> esp_err_t;
    pub fn esp_wifi_scan_start(config: *const wifi_scan_config_t, block: bool) -> esp_err_t;
    pub fn esp_wifi_scan_stop() -> esp_err_t;
    pub fn esp_wifi_scan_get_ap_num(number: *mut u16) -> esp_err_t;
    pub fn esp_wifi_scan_get_ap_records(
        number: *mut u16,
        ap_records: *mut wifi_ap_record_t,
    ) -> esp_err_t;
    pub fn esp_wifi_set_ps(type_: wifi_ps_type_t) -> esp_err_t;
    pub fn esp_wifi_set_channel(primary: u8, second: wifi_second_chan_t) -> esp_err_t;
    pub fn esp_wifi_get_mac(ifx: wifi_interface_t, mac: *mut u8) -> esp_err_t;
    pub fn esp_wifi_set_promiscuous_rx_cb(cb: wifi_promiscuous_cb_t) -> esp_err_t;
    pub fn esp_wifi_set_promiscuous(en: bool) -> esp_err_t;
    pub fn esp_wifi_set_promiscuous_filter(filter: *const wifi_promiscuous_filter_t) -> esp_err_t;
    pub fn esp_wifi_set_config(interface: wifi_interface_t, conf: *mut wifi_config_t) -> esp_err_t;
    pub fn esp_wifi_set_promiscuous_ctrl_filter(
        filter: *const wifi_promiscuous_filter_t,
    ) -> esp_err_t;
    pub fn esp_wifi_ap_get_sta_list(sta: *mut wifi_sta_list_t) -> esp_err_t;
    pub fn esp_wifi_set_storage(storage: wifi_storage_t) -> esp_err_t;
    pub fn esp_esptouch_set_timeout(time_s: u8) -> esp_err_t;
    pub fn esp_smartconfig_set_type(type_: smartconfig_type_t) -> esp_err_t;
    pub fn esp_smartconfig_fast_mode(enable: bool) -> esp_err_t;
    pub fn esp_smartconfig_get_rvd_data(rvd_data: *mut u8, len: u8) -> esp_err_t;
    pub fn esp_wifi_internal_free_rx_buffer(buffer: *mut core::ffi::c_void);
    pub fn esp_wifi_internal_tx(
        wifi_if: wifi_interface_t,
        buffer: *mut core::ffi::c_void,
        len: u16,
    ) -> core::ffi::c_int;
    pub fn esp_wifi_internal_reg_rxcb(ifx: wifi_interface_t, fn_: wifi_rxcb_t) -> esp_err_t;
    pub fn esp_smartconfig_internal_start(config: *const smartconfig_start_config_t) -> esp_err_t;
    pub fn esp_smartconfig_internal_stop() -> esp_err_t;
    pub fn esp_timer_get_time() -> i64;
    pub fn esp_wifi_wps_enable(config: *const esp_wps_config_t) -> esp_err_t;
    pub fn esp_wifi_wps_disable() -> esp_err_t;
    pub fn esp_wifi_wps_start(timeout_ms: core::ffi::c_int) -> esp_err_t;
    pub fn esp_wifi_sta_wpa2_ent_enable() -> esp_err_t;
    pub fn esp_wifi_sta_wpa2_ent_disable() -> esp_err_t;
    pub fn esp_wifi_sta_wpa2_ent_set_identity(
        identity: *const core::ffi::c_uchar,
        len: core::ffi::c_int,
    ) -> esp_err_t;
    pub fn esp_wifi_sta_wpa2_ent_clear_identity();
    pub fn esp_wifi_sta_wpa2_ent_set_username(
        username: *const core::ffi::c_uchar,
        len: core::ffi::c_int,
    ) -> esp_err_t;
    pub fn esp_wifi_sta_wpa2_ent_clear_username();
    pub fn esp_wifi_sta_wpa2_ent_set_password(
        password: *const core::ffi::c_uchar,
        len: core::ffi::c_int,
    ) -> esp_err_t;
    pub fn esp_wifi_sta_wpa2_ent_clear_password();
    pub fn esp_wifi_sta_wpa2_ent_clear_new_password();
    pub fn esp_wifi_sta_wpa2_ent_set_ca_cert(
        ca_cert: *const core::ffi::c_uchar,
        ca_cert_len: core::ffi::c_int,
    ) -> esp_err_t;
    pub fn esp_wifi_sta_wpa2_ent_clear_ca_cert();
    pub fn esp_wifi_sta_wpa2_ent_set_cert_key(
        client_cert: *const core::ffi::c_uchar,
        client_cert_len: core::ffi::c_int,
        private_key: *const core::ffi::c_uchar,
        private_key_len: core::ffi::c_int,
        private_key_passwd: *const core::ffi::c_uchar,
        private_key_passwd_len: core::ffi::c_int,
    ) -> esp_err_t;
    pub fn esp_wifi_sta_wpa2_ent_clear_cert_key();
    pub fn esp_now_init() -> esp_err_t;
    pub fn esp_now_deinit() -> esp_err_t;
    pub fn esp_now_register_recv_cb(cb: esp_now_recv_cb_t) -> esp_err_t;
    pub fn esp_now_unregister_recv_cb() -> esp_err_t;
    pub fn esp_now_register_send_cb(cb: esp_now_send_cb_t) -> esp_err_t;
    pub fn esp_now_unregister_send_cb() -> esp_err_t;
    pub fn esp_now_send(peer_addr: *const u8, data: *const u8, len: usize) -> esp_err_t;
    pub fn esp_now_add_peer(peer: *const esp_now_peer_info_t) -> esp_err_t;
    pub fn esp_now_del_peer(peer_addr: *const u8) -> esp_err_t;
    pub fn esp_now_mod_peer(peer: *const esp_now_peer_info_t) -> esp_err_t;
    pub fn esp_now_is_peer_exist(peer_addr: *const u8) -> bool;
    pub fn esp_now_get_peer_num(num: *mut esp_now_peer_num_t) -> esp_err_t;
    pub fn esp_now_set_pmk(pmk: *const u8) -> esp_err_t;
}
