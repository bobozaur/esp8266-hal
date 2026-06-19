#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_rx_trace))]
use portable_atomic::AtomicPtr;
use portable_atomic::{AtomicU32, Ordering};

use super::target_callback::CallbackRegistration;
use super::target_map::*;
use super::{
    AccessPointConfig, AccessPointRecord, ApStationInfo, Channel, ClientConfig, ConnectError,
    Credential, Deadline, DisconnectError, DriverError, EspNowPeerConfig, EspNowRxFrame,
    EspNowSendStatus, EspNowTxStatus, InitConfig, InitError, PromiscuousControlFilter,
    PromiscuousFilter, PromiscuousFrameInfo, PromiscuousPacketType, PromiscuousRxFrame, RawMode,
    RawRxRecord, ScanOptions, StationPowerSave, WifiApBackend, WifiEvent, WifiFeatureBackend,
    WifiLifecycleBackend, WifiPacketBackend, WifiResources, WifiRuntimeBackend, WifiScanBackend,
    WifiStaBackend, Wpa2EnterpriseCredentials, WpsConfig, WpsPin, PROMISCUOUS_MAX_PAYLOAD_PREFIX,
};
#[cfg(feature = "wifi-debug-trace")]
use super::{DeliverDataDebugSnapshot, RxCallbackDebugSnapshot};
#[cfg(feature = "wifi-smartconfig")]
use super::{
    Ipv4Addr, SmartConfigCredentials, SmartConfigStartConfig, SmartConfigTimeout, SmartConfigType,
};

extern "C" {
    fn esp8266_hal_wps_set_pin(pin: *const u8);
    fn esp8266_hal_wps_set_target_ssid(ssid: *const u8, len: u32);
}

#[cfg(feature = "wifi-debug-trace")]
#[no_mangle]
static g_sta_rx_cb_calls: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "wifi-debug-trace")]
#[no_mangle]
static g_sta_rx_cb_enqueued: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "wifi-debug-trace")]
#[no_mangle]
static g_sta_rx_cb_rejected: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "wifi-debug-trace")]
#[no_mangle]
static g_sta_rx_cb_null_resources: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "wifi-debug-trace")]
#[no_mangle]
static g_sta_rx_cb_last_len: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "wifi-debug-trace")]
#[no_mangle]
static g_sta_rx_attach_calls: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "wifi-debug-trace")]
#[no_mangle]
static g_sta_rx_refresh_calls: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "wifi-debug-trace")]
#[no_mangle]
static g_sta_rx_detach_calls: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "wifi-debug-trace")]
#[no_mangle]
static g_sta_rx_register_calls: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "wifi-debug-trace")]
#[no_mangle]
static g_sta_rx_unregister_calls: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "wifi-debug-trace")]
#[no_mangle]
static g_sta_rx_last_register_ret: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "wifi-debug-trace")]
#[no_mangle]
static g_sta_rx_last_unregister_ret: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "wifi-debug-trace")]
#[no_mangle]
static g_sta_rx_expected_slot: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "wifi-debug-trace")]
#[no_mangle]
static g_ap_rx_cb_calls: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "wifi-debug-trace")]
#[no_mangle]
static g_ap_rx_cb_enqueued: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "wifi-debug-trace")]
#[no_mangle]
static g_ap_rx_cb_rejected: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "wifi-debug-trace")]
#[no_mangle]
static g_ap_rx_cb_null_resources: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "wifi-debug-trace")]
#[no_mangle]
static g_ap_rx_cb_last_len: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "wifi-debug-trace")]
#[no_mangle]
static g_ap_rx_attach_calls: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "wifi-debug-trace")]
#[no_mangle]
static g_ap_rx_refresh_calls: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "wifi-debug-trace")]
#[no_mangle]
static g_ap_rx_detach_calls: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "wifi-debug-trace")]
#[no_mangle]
static g_ap_rx_register_calls: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "wifi-debug-trace")]
#[no_mangle]
static g_ap_rx_unregister_calls: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "wifi-debug-trace")]
#[no_mangle]
static g_ap_rx_last_register_ret: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "wifi-debug-trace")]
#[no_mangle]
static g_ap_rx_last_unregister_ret: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "wifi-debug-trace")]
#[no_mangle]
static g_ap_rx_expected_slot: AtomicU32 = AtomicU32::new(0);
static PROMISCUOUS_FILTER_BITS: AtomicU32 = AtomicU32::new(0);
static PROMISCUOUS_CHANNEL: AtomicU32 = AtomicU32::new(0);
static CORE_STA_START_SEQ: AtomicU32 = AtomicU32::new(0);

#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_sniffer_trace))]
#[no_mangle]
pub static mut g_smartconfig_sniffer_calls: u32 = 0;
#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_sniffer_trace))]
#[no_mangle]
pub static mut g_smartconfig_sniffer_last_type: u32 = 0;
#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_sniffer_trace))]
#[no_mangle]
pub static mut g_smartconfig_sniffer_last_len: u32 = 0;
#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_sniffer_trace))]
#[no_mangle]
pub static mut g_smartconfig_sniffer_last_channel: u32 = 0;
#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_sniffer_trace))]
#[no_mangle]
pub static mut g_smartconfig_sniffer_last_rssi: u32 = 0;

#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_sniffer_trace))]
extern "C" {
    fn __real_sc_sniffer_cb(
        buffer: *mut core::ffi::c_void,
        packet_type: esp8266_wifi_sys::wifi_promiscuous_pkt_type_t,
    );
}

#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_sniffer_trace))]
#[no_mangle]
unsafe extern "C" fn __wrap_sc_sniffer_cb(
    buffer: *mut core::ffi::c_void,
    packet_type: esp8266_wifi_sys::wifi_promiscuous_pkt_type_t,
) {
    unsafe {
        g_smartconfig_sniffer_calls = g_smartconfig_sniffer_calls.wrapping_add(1);
        g_smartconfig_sniffer_last_type = packet_type;
        if !buffer.is_null() {
            let packet = &*(buffer as *const esp8266_wifi_sys::wifi_promiscuous_pkt_t);
            g_smartconfig_sniffer_last_len = if packet.rx_ctrl.sig_mode() != 0 {
                packet.rx_ctrl.ht_length() as u32
            } else {
                packet.rx_ctrl.legacy_length() as u32
            };
            g_smartconfig_sniffer_last_channel = packet.rx_ctrl.channel() as u32;
            g_smartconfig_sniffer_last_rssi = packet.rx_ctrl.rssi() as i32 as u32;
        }
        __real_sc_sniffer_cb(buffer, packet_type);
    }
}

#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_wifi_trace))]
static SC_WIFI_TRACE_CHANNEL_CALLS: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_wifi_trace))]
static SC_WIFI_TRACE_LAST_CHANNEL: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_wifi_trace))]
static SC_WIFI_TRACE_LAST_CHANNEL_RET: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_wifi_trace))]
static SC_WIFI_TRACE_FILTER_CALLS: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_wifi_trace))]
static SC_WIFI_TRACE_LAST_FILTER: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_wifi_trace))]
static SC_WIFI_TRACE_LAST_FILTER_RET: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_wifi_trace))]
static SC_WIFI_TRACE_RX_CB_CALLS: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_wifi_trace))]
static SC_WIFI_TRACE_LAST_RX_CB: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_wifi_trace))]
static SC_WIFI_TRACE_LAST_RX_CB_RET: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_wifi_trace))]
static SC_WIFI_TRACE_PROMISC_CALLS: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_wifi_trace))]
static SC_WIFI_TRACE_LAST_PROMISC_ENABLE: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_wifi_trace))]
static SC_WIFI_TRACE_LAST_PROMISC_RET: AtomicU32 = AtomicU32::new(0);

#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_rx_trace))]
static SC_RX_TRACE_SET_CALLS: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_rx_trace))]
static SC_RX_TRACE_LAST_SET_CB: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_rx_trace))]
static SC_RX_TRACE_LAST_SET_RET: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_rx_trace))]
static SC_RX_TRACE_CALLBACK_CALLS: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_rx_trace))]
static SC_RX_TRACE_LAST_TYPE: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_rx_trace))]
static SC_RX_TRACE_LAST_LEN: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_rx_trace))]
static SC_RX_TRACE_LAST_CHANNEL: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_rx_trace))]
static SC_RX_TRACE_LAST_RSSI: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_rx_trace))]
static SC_RX_TRACE_ORIGINAL_CB: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_event_trace))]
static SC_EVENT_TRACE_CALLS: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_event_trace))]
static SC_EVENT_TRACE_NULL_RESOURCES: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_event_trace))]
static SC_EVENT_TRACE_LAST_ID: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_event_trace))]
static SC_EVENT_TRACE_LAST_DATA: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_event_trace))]
static SC_EVENT_TRACE_PUBLISHED: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_event_trace))]
static SC_EVENT_TRACE_GOT_CALLS: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_event_trace))]
static SC_EVENT_TRACE_GOT_ACCEPTED: AtomicU32 = AtomicU32::new(0);
#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_event_trace))]
static SC_EVENT_TRACE_GOT_REJECTED: AtomicU32 = AtomicU32::new(0);

#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_wifi_trace))]
extern "C" {
    fn __real_esp_wifi_set_channel(
        primary: u8,
        second: esp8266_wifi_sys::wifi_second_chan_t,
    ) -> esp8266_wifi_sys::esp_err_t;
    fn __real_esp_wifi_set_promiscuous_filter(
        filter: *const esp8266_wifi_sys::wifi_promiscuous_filter_t,
    ) -> esp8266_wifi_sys::esp_err_t;
    fn __real_esp_wifi_set_promiscuous_rx_cb(
        cb: esp8266_wifi_sys::wifi_promiscuous_cb_t,
    ) -> esp8266_wifi_sys::esp_err_t;
    fn __real_esp_wifi_set_promiscuous(en: bool) -> esp8266_wifi_sys::esp_err_t;
}

#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_rx_trace))]
extern "C" {
    fn __real_esp_wifi_set_promiscuous_rx_cb(
        cb: esp8266_wifi_sys::wifi_promiscuous_cb_t,
    ) -> esp8266_wifi_sys::esp_err_t;
}

#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_wifi_trace))]
#[no_mangle]
pub unsafe extern "C" fn esp8266_hal_sc_wifi_trace_snapshot(out: *mut u32, len: usize) {
    if out.is_null() {
        return;
    }

    let values = [
        SC_WIFI_TRACE_CHANNEL_CALLS.load(Ordering::Acquire),
        SC_WIFI_TRACE_LAST_CHANNEL.load(Ordering::Acquire),
        SC_WIFI_TRACE_LAST_CHANNEL_RET.load(Ordering::Acquire),
        SC_WIFI_TRACE_FILTER_CALLS.load(Ordering::Acquire),
        SC_WIFI_TRACE_LAST_FILTER.load(Ordering::Acquire),
        SC_WIFI_TRACE_LAST_FILTER_RET.load(Ordering::Acquire),
        SC_WIFI_TRACE_RX_CB_CALLS.load(Ordering::Acquire),
        SC_WIFI_TRACE_LAST_RX_CB.load(Ordering::Acquire),
        SC_WIFI_TRACE_LAST_RX_CB_RET.load(Ordering::Acquire),
        SC_WIFI_TRACE_PROMISC_CALLS.load(Ordering::Acquire),
        SC_WIFI_TRACE_LAST_PROMISC_ENABLE.load(Ordering::Acquire),
        SC_WIFI_TRACE_LAST_PROMISC_RET.load(Ordering::Acquire),
    ];

    for (idx, value) in values.iter().copied().enumerate().take(len) {
        unsafe {
            out.add(idx).write_volatile(value);
        }
    }
}

#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_rx_trace))]
#[no_mangle]
pub unsafe extern "C" fn esp8266_hal_sc_rx_trace_snapshot(out: *mut u32, len: usize) {
    if out.is_null() {
        return;
    }

    let values = [
        SC_RX_TRACE_SET_CALLS.load(Ordering::Acquire),
        SC_RX_TRACE_LAST_SET_CB.load(Ordering::Acquire),
        SC_RX_TRACE_LAST_SET_RET.load(Ordering::Acquire),
        SC_RX_TRACE_CALLBACK_CALLS.load(Ordering::Acquire),
        SC_RX_TRACE_LAST_TYPE.load(Ordering::Acquire),
        SC_RX_TRACE_LAST_LEN.load(Ordering::Acquire),
        SC_RX_TRACE_LAST_CHANNEL.load(Ordering::Acquire),
        SC_RX_TRACE_LAST_RSSI.load(Ordering::Acquire),
    ];

    for (idx, value) in values.iter().copied().enumerate().take(len) {
        unsafe {
            out.add(idx).write_volatile(value);
        }
    }
}

#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_event_trace))]
#[no_mangle]
pub unsafe extern "C" fn esp8266_hal_sc_event_trace_snapshot(out: *mut u32, len: usize) {
    if out.is_null() {
        return;
    }

    let values = [
        SC_EVENT_TRACE_CALLS.load(Ordering::Acquire),
        SC_EVENT_TRACE_NULL_RESOURCES.load(Ordering::Acquire),
        SC_EVENT_TRACE_LAST_ID.load(Ordering::Acquire),
        SC_EVENT_TRACE_LAST_DATA.load(Ordering::Acquire),
        SC_EVENT_TRACE_PUBLISHED.load(Ordering::Acquire),
        SC_EVENT_TRACE_GOT_CALLS.load(Ordering::Acquire),
        SC_EVENT_TRACE_GOT_ACCEPTED.load(Ordering::Acquire),
        SC_EVENT_TRACE_GOT_REJECTED.load(Ordering::Acquire),
    ];

    for (idx, value) in values.iter().copied().enumerate().take(len) {
        unsafe {
            out.add(idx).write_volatile(value);
        }
    }
}

#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_wifi_trace))]
#[no_mangle]
unsafe extern "C" fn __wrap_esp_wifi_set_channel(
    primary: u8,
    second: esp8266_wifi_sys::wifi_second_chan_t,
) -> esp8266_wifi_sys::esp_err_t {
    SC_WIFI_TRACE_CHANNEL_CALLS.fetch_add(1, Ordering::AcqRel);
    SC_WIFI_TRACE_LAST_CHANNEL.store(primary as u32, Ordering::Release);
    let ret = unsafe { __real_esp_wifi_set_channel(primary, second) };
    SC_WIFI_TRACE_LAST_CHANNEL_RET.store(ret as u32, Ordering::Release);
    ret
}

#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_wifi_trace))]
#[no_mangle]
unsafe extern "C" fn __wrap_esp_wifi_set_promiscuous_filter(
    filter: *const esp8266_wifi_sys::wifi_promiscuous_filter_t,
) -> esp8266_wifi_sys::esp_err_t {
    SC_WIFI_TRACE_FILTER_CALLS.fetch_add(1, Ordering::AcqRel);
    let mask = if filter.is_null() {
        0
    } else {
        unsafe { (*filter).filter_mask }
    };
    SC_WIFI_TRACE_LAST_FILTER.store(mask, Ordering::Release);
    let ret = unsafe { __real_esp_wifi_set_promiscuous_filter(filter) };
    SC_WIFI_TRACE_LAST_FILTER_RET.store(ret as u32, Ordering::Release);
    ret
}

#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_wifi_trace))]
#[no_mangle]
unsafe extern "C" fn __wrap_esp_wifi_set_promiscuous_rx_cb(
    cb: esp8266_wifi_sys::wifi_promiscuous_cb_t,
) -> esp8266_wifi_sys::esp_err_t {
    SC_WIFI_TRACE_RX_CB_CALLS.fetch_add(1, Ordering::AcqRel);
    SC_WIFI_TRACE_LAST_RX_CB.store(cb.map_or(0, |f| f as usize as u32), Ordering::Release);
    let ret = unsafe { __real_esp_wifi_set_promiscuous_rx_cb(cb) };
    SC_WIFI_TRACE_LAST_RX_CB_RET.store(ret as u32, Ordering::Release);
    ret
}

#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_rx_trace))]
unsafe extern "C" fn sc_rx_trace_cb(
    buffer: *mut core::ffi::c_void,
    packet_type: esp8266_wifi_sys::wifi_promiscuous_pkt_type_t,
) {
    SC_RX_TRACE_CALLBACK_CALLS.fetch_add(1, Ordering::AcqRel);
    SC_RX_TRACE_LAST_TYPE.store(packet_type, Ordering::Release);

    if !buffer.is_null() {
        let packet = unsafe { &*(buffer as *const esp8266_wifi_sys::wifi_promiscuous_pkt_t) };
        let len = if packet.rx_ctrl.sig_mode() != 0 {
            packet.rx_ctrl.ht_length() as u32
        } else {
            packet.rx_ctrl.legacy_length() as u32
        };
        SC_RX_TRACE_LAST_LEN.store(len, Ordering::Release);
        SC_RX_TRACE_LAST_CHANNEL.store(packet.rx_ctrl.channel() as u32, Ordering::Release);
        SC_RX_TRACE_LAST_RSSI.store(packet.rx_ctrl.rssi() as i32 as u32, Ordering::Release);
    }

    let original = SC_RX_TRACE_ORIGINAL_CB.load(Ordering::Acquire);
    if !original.is_null() {
        let original: unsafe extern "C" fn(
            *mut core::ffi::c_void,
            esp8266_wifi_sys::wifi_promiscuous_pkt_type_t,
        ) = unsafe { core::mem::transmute(original) };
        unsafe {
            original(buffer, packet_type);
        }
    }
}

#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_rx_trace))]
#[no_mangle]
unsafe extern "C" fn __wrap_esp_wifi_set_promiscuous_rx_cb(
    cb: esp8266_wifi_sys::wifi_promiscuous_cb_t,
) -> esp8266_wifi_sys::esp_err_t {
    SC_RX_TRACE_SET_CALLS.fetch_add(1, Ordering::AcqRel);
    SC_RX_TRACE_LAST_SET_CB.store(cb.map_or(0, |f| f as usize as u32), Ordering::Release);

    let traced_cb: esp8266_wifi_sys::wifi_promiscuous_cb_t = match cb {
        Some(callback) => {
            SC_RX_TRACE_ORIGINAL_CB.store(callback as *mut (), Ordering::Release);
            Some(sc_rx_trace_cb)
        }
        None => {
            SC_RX_TRACE_ORIGINAL_CB.store(core::ptr::null_mut(), Ordering::Release);
            None
        }
    };

    let ret = unsafe { __real_esp_wifi_set_promiscuous_rx_cb(traced_cb) };
    SC_RX_TRACE_LAST_SET_RET.store(ret as u32, Ordering::Release);
    ret
}

#[cfg(all(feature = "wifi-smartconfig", esp8266_hal_smartconfig_wifi_trace))]
#[no_mangle]
unsafe extern "C" fn __wrap_esp_wifi_set_promiscuous(en: bool) -> esp8266_wifi_sys::esp_err_t {
    SC_WIFI_TRACE_PROMISC_CALLS.fetch_add(1, Ordering::AcqRel);
    SC_WIFI_TRACE_LAST_PROMISC_ENABLE.store(en as u32, Ordering::Release);
    let ret = unsafe { __real_esp_wifi_set_promiscuous(en) };
    SC_WIFI_TRACE_LAST_PROMISC_RET.store(ret as u32, Ordering::Release);
    ret
}

#[cfg(feature = "wifi-debug-trace")]
macro_rules! rx_debug {
    ($($statement:expr);* $(;)?) => {
        $(let _ = $statement;)*
    };
}

#[cfg(not(feature = "wifi-debug-trace"))]
macro_rules! rx_debug {
    ($($statement:expr);* $(;)?) => {};
}

// STA_START normally arrives on the first scheduler turn after
// `esp_wifi_start()`. Keep a wide 1s margin so `start()` fails explicitly
// instead of letting callers race an SDK station state that is not ready.
const STA_START_EVENT_TIMEOUT_MS: u32 = 1_000;

#[cfg(feature = "wifi-debug-trace")]
extern "C" {
    static mut sta_rxcb: [u32; 2];
}

pub struct Esp8266TargetBackend<
    'd,
    const EVENTS: usize,
    const RX: usize,
    const SUBS: usize,
    const ESP_NOW_PEERS: usize = 0,
    const ESP_NOW_RX: usize = 0,
    const PROMISCUOUS_RX: usize = 0,
    const AP_RX_SLOTS: usize = 0,
    const AP_RX_BYTES: usize = 0,
> {
    controller: Option<crate::wifi::WifiController>,
    resources: &'d WifiResources<
        EVENTS,
        RX,
        SUBS,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >,
    registration: Option<
        CallbackRegistration<
            'd,
            EVENTS,
            RX,
            SUBS,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
    >,
    event_handler_registered: bool,
    #[cfg(feature = "wifi-smartconfig")]
    smartconfig_handler_registered: bool,
    sta_rx_registered: bool,
    ap_rx_registered: bool,
    ap_netif_started: bool,
    wpa2_enterprise_enabled: bool,
    station_power_save: StationPowerSave,
}

impl<
        'd,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    >
    Esp8266TargetBackend<
        'd,
        EVENTS,
        RX,
        SUBS,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
{
    pub fn new(
        resources: &'d WifiResources<
            EVENTS,
            RX,
            SUBS,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
    ) -> Self {
        Self {
            controller: None,
            resources,
            registration: None,
            event_handler_registered: false,
            #[cfg(feature = "wifi-smartconfig")]
            smartconfig_handler_registered: false,
            sta_rx_registered: false,
            ap_rx_registered: false,
            ap_netif_started: false,
            wpa2_enterprise_enabled: false,
            station_power_save: StationPowerSave::None,
        }
    }

    fn controller(&self) -> Result<&crate::wifi::WifiController, DriverError> {
        self.controller.as_ref().ok_or(DriverError::BackendRejected)
    }

    fn controller_mut(&mut self) -> Result<&mut crate::wifi::WifiController, DriverError> {
        self.controller.as_mut().ok_or(DriverError::BackendRejected)
    }

    fn apply_station_power_save_for_mode(&mut self) -> Result<(), DriverError> {
        let mode = self
            .controller()?
            .get_mode()
            .map_err(map_wifi_driver_error)?;
        if matches!(
            mode,
            crate::wifi::WifiMode::Sta | crate::wifi::WifiMode::ApSta
        ) {
            let power_save = map_station_power_save(self.station_power_save);
            self.controller_mut()?
                .set_power_save(power_save)
                .map_err(map_wifi_driver_error)?;
        }
        Ok(())
    }

    fn configure_wpa2_enterprise_for_station(
        &mut self,
        config: &ClientConfig,
    ) -> Result<(), DriverError> {
        match config.credential() {
            Credential::Wpa2Enterprise(credentials) => self.enable_wpa2_enterprise(credentials),
            Credential::Open | Credential::Wpa2(_) => self.disable_wpa2_enterprise_if_enabled(),
        }
    }

    fn enable_wpa2_enterprise(
        &mut self,
        credentials: &Wpa2EnterpriseCredentials,
    ) -> Result<(), DriverError> {
        self.clear_wpa2_enterprise_credentials();
        let result = self.enable_wpa2_enterprise_inner(credentials);
        if result.is_err() {
            self.clear_wpa2_enterprise_credentials();
            self.wpa2_enterprise_enabled = false;
        }
        result
    }

    fn enable_wpa2_enterprise_inner(
        &mut self,
        credentials: &Wpa2EnterpriseCredentials,
    ) -> Result<(), DriverError> {
        let identity = credentials.identity().as_bytes();
        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_wifi_sta_wpa2_ent_set_identity(
                identity.as_ptr(),
                identity.len() as i32,
            )
        })
        .map_err(map_enterprise_esp_error)?;

        let username = credentials.username().as_bytes();
        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_wifi_sta_wpa2_ent_set_username(
                username.as_ptr(),
                username.len() as i32,
            )
        })
        .map_err(map_enterprise_esp_error)?;

        let password = credentials.password().as_bytes();
        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_wifi_sta_wpa2_ent_set_password(
                password.as_ptr(),
                password.len() as i32,
            )
        })
        .map_err(map_enterprise_esp_error)?;

        if let Some(ca_cert) = credentials.ca_cert() {
            crate::wifi::EspError::check(unsafe {
                esp8266_wifi_sys::esp_wifi_sta_wpa2_ent_set_ca_cert(
                    ca_cert.as_ptr(),
                    ca_cert.len() as i32,
                )
            })
            .map_err(map_enterprise_esp_error)?;
        }

        if let Some(certificate) = credentials.client_certificate() {
            let password = certificate.private_key_password();
            let password_ptr = password.map_or(core::ptr::null(), <[u8]>::as_ptr);
            let password_len = password.map_or(0, |password| password.len() as i32);
            crate::wifi::EspError::check(unsafe {
                esp8266_wifi_sys::esp_wifi_sta_wpa2_ent_set_cert_key(
                    certificate.client_cert().as_ptr(),
                    certificate.client_cert().len() as i32,
                    certificate.private_key().as_ptr(),
                    certificate.private_key().len() as i32,
                    password_ptr,
                    password_len,
                )
            })
            .map_err(map_enterprise_esp_error)?;
        }

        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_wifi_sta_wpa2_ent_set_disable_time_check(
                credentials.disable_time_check(),
            )
        })
        .map_err(map_enterprise_esp_error)?;

        crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_wifi_sta_wpa2_ent_enable() })
            .map_err(map_enterprise_esp_error)?;

        self.wpa2_enterprise_enabled = true;
        Ok(())
    }

    fn disable_wpa2_enterprise_if_enabled(&mut self) -> Result<(), DriverError> {
        if !self.wpa2_enterprise_enabled {
            return Ok(());
        }

        let result = crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_wifi_sta_wpa2_ent_disable()
        })
        .map_err(map_enterprise_esp_error);
        self.clear_wpa2_enterprise_credentials();
        if result.is_ok() {
            self.wpa2_enterprise_enabled = false;
            crate::wifi::compat::memory::release_eap_rsa_workspace();
        }
        result
    }

    fn clear_wpa2_enterprise_credentials(&self) {
        unsafe {
            esp8266_wifi_sys::esp_wifi_sta_wpa2_ent_clear_identity();
            esp8266_wifi_sys::esp_wifi_sta_wpa2_ent_clear_username();
            esp8266_wifi_sys::esp_wifi_sta_wpa2_ent_clear_password();
            esp8266_wifi_sys::esp_wifi_sta_wpa2_ent_clear_new_password();
            esp8266_wifi_sys::esp_wifi_sta_wpa2_ent_clear_ca_cert();
            esp8266_wifi_sys::esp_wifi_sta_wpa2_ent_clear_cert_key();
        }
    }

    fn connect_blocking_inner(&mut self, timeout_ms: u32) -> Result<(), DriverError> {
        crate::wifi::event::enable_sdk_sta_auto_connect(u32::MAX).map_err(map_wifi_driver_error)?;
        let result = self.connect_blocking_with_event_retries(timeout_ms);
        crate::wifi::event::disable_sdk_sta_auto_connect();
        if matches!(
            result.as_ref().err(),
            Some(error) if matches!(error, DriverError::Timeout | DriverError::Disconnected)
        ) && crate::wifi::controller::current_sta_state()
            != crate::wifi::controller::StaConnectionState::Connected
        {
            let _ = self
                .controller_mut()
                .and_then(|controller| controller.disconnect().map_err(map_wifi_driver_error));
        }
        result
    }

    fn connect_blocking_with_event_retries(&mut self, timeout_ms: u32) -> Result<(), DriverError> {
        use crate::wifi::controller::{current_sta_state, StaConnectionState};

        let deadline = Deadline::new(self.now_ms(), timeout_ms);
        self.controller_mut()
            .and_then(|controller| controller.connect().map_err(map_wifi_driver_error))?;

        loop {
            match current_sta_state() {
                StaConnectionState::Connected => {
                    self.apply_station_power_save_for_mode()?;
                    return Ok(());
                }
                StaConnectionState::Disconnected | StaConnectionState::Connecting => {}
            }

            if deadline.is_expired(self.now_ms()) {
                return Err(DriverError::Timeout);
            }

            self.spin_once();
        }
    }

    #[inline(never)]
    fn register_event_handler(&mut self) -> Result<(), DriverError> {
        if self.event_handler_registered {
            return Ok(());
        }

        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_event_handler_register(
                esp8266_wifi_sys::WIFI_EVENT,
                -1,
                Some(
                    wifi_core_event_trampoline::<
                        EVENTS,
                        RX,
                        SUBS,
                        ESP_NOW_PEERS,
                        ESP_NOW_RX,
                        PROMISCUOUS_RX,
                        AP_RX_SLOTS,
                        AP_RX_BYTES,
                    >,
                ),
                core::ptr::null_mut(),
            )
        })
        .map_err(|_| DriverError::BackendRejected)?;
        self.event_handler_registered = true;
        Ok(())
    }

    fn unregister_event_handler(&mut self) {
        if !self.event_handler_registered {
            return;
        }

        let _ = unsafe {
            esp8266_wifi_sys::esp_event_handler_unregister(
                esp8266_wifi_sys::WIFI_EVENT,
                -1,
                Some(
                    wifi_core_event_trampoline::<
                        EVENTS,
                        RX,
                        SUBS,
                        ESP_NOW_PEERS,
                        ESP_NOW_RX,
                        PROMISCUOUS_RX,
                        AP_RX_SLOTS,
                        AP_RX_BYTES,
                    >,
                ),
            )
        };
        self.event_handler_registered = false;
    }

    #[cfg(feature = "wifi-smartconfig")]
    fn register_smartconfig_event_handler(&mut self) -> Result<(), DriverError> {
        if self.smartconfig_handler_registered {
            return Ok(());
        }

        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_event_handler_register(
                esp8266_wifi_sys::SC_EVENT,
                -1,
                Some(
                    smartconfig_event_trampoline::<
                        EVENTS,
                        RX,
                        SUBS,
                        ESP_NOW_PEERS,
                        ESP_NOW_RX,
                        PROMISCUOUS_RX,
                        AP_RX_SLOTS,
                        AP_RX_BYTES,
                    >,
                ),
                core::ptr::null_mut(),
            )
        })
        .map_err(|_| DriverError::BackendRejected)?;
        self.smartconfig_handler_registered = true;
        Ok(())
    }

    #[cfg(feature = "wifi-smartconfig")]
    fn unregister_smartconfig_event_handler(&mut self) {
        if !self.smartconfig_handler_registered {
            return;
        }

        let _ = unsafe {
            esp8266_wifi_sys::esp_event_handler_unregister(
                esp8266_wifi_sys::SC_EVENT,
                -1,
                Some(
                    smartconfig_event_trampoline::<
                        EVENTS,
                        RX,
                        SUBS,
                        ESP_NOW_PEERS,
                        ESP_NOW_RX,
                        PROMISCUOUS_RX,
                        AP_RX_SLOTS,
                        AP_RX_BYTES,
                    >,
                ),
            )
        };
        self.smartconfig_handler_registered = false;
    }

    #[inline(never)]
    fn register_sta_rx_callback(&mut self) -> Result<(), DriverError> {
        let ret = unsafe {
            esp8266_wifi_sys::esp_wifi_internal_reg_rxcb(
                crate::wifi::controller::WifiInterface::Sta.to_raw(),
                Some(
                    rx_callback_sta::<
                        EVENTS,
                        RX,
                        SUBS,
                        ESP_NOW_PEERS,
                        ESP_NOW_RX,
                        PROMISCUOUS_RX,
                        AP_RX_SLOTS,
                        AP_RX_BYTES,
                    >,
                ),
            )
        };
        rx_debug! {
            g_sta_rx_register_calls.fetch_add(1, Ordering::AcqRel);
            g_sta_rx_last_register_ret.store(ret as u32, Ordering::Release);
            g_sta_rx_expected_slot.store(
                rx_callback_sta::<EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX, AP_RX_SLOTS, AP_RX_BYTES> as usize
                    as u32,
                Ordering::Release,
            );
        }
        crate::wifi::EspError::check(ret).map_err(|_| DriverError::BackendRejected)
    }

    fn register_ap_rx_callback(&mut self) -> Result<(), DriverError> {
        let ret = unsafe {
            esp8266_wifi_sys::esp_wifi_internal_reg_rxcb(
                crate::wifi::controller::WifiInterface::Ap.to_raw(),
                Some(
                    rx_callback_ap::<
                        EVENTS,
                        RX,
                        SUBS,
                        ESP_NOW_PEERS,
                        ESP_NOW_RX,
                        PROMISCUOUS_RX,
                        AP_RX_SLOTS,
                        AP_RX_BYTES,
                    >,
                ),
            )
        };
        rx_debug! {
            g_ap_rx_register_calls.fetch_add(1, Ordering::AcqRel);
            g_ap_rx_last_register_ret.store(ret as u32, Ordering::Release);
            g_ap_rx_expected_slot.store(
                rx_callback_ap::<EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX, AP_RX_SLOTS, AP_RX_BYTES> as usize
                    as u32,
                Ordering::Release,
            );
        }
        crate::wifi::EspError::check(ret).map_err(|_| DriverError::BackendRejected)
    }

    fn unregister_sta_rx_callback(&mut self) {
        let _ret = unsafe {
            esp8266_wifi_sys::esp_wifi_internal_reg_rxcb(
                crate::wifi::controller::WifiInterface::Sta.to_raw(),
                None,
            )
        };
        rx_debug! {
            g_sta_rx_unregister_calls.fetch_add(1, Ordering::AcqRel);
            g_sta_rx_last_unregister_ret.store(_ret as u32, Ordering::Release);
        }
    }

    fn unregister_ap_rx_callback(&mut self) {
        let _ret = unsafe {
            esp8266_wifi_sys::esp_wifi_internal_reg_rxcb(
                crate::wifi::controller::WifiInterface::Ap.to_raw(),
                None,
            )
        };
        rx_debug! {
            g_ap_rx_unregister_calls.fetch_add(1, Ordering::AcqRel);
            g_ap_rx_last_unregister_ret.store(_ret as u32, Ordering::Release);
        }
    }

    fn ensure_ap_netif_started(&mut self) -> Result<(), DriverError> {
        if self.ap_netif_started {
            return Ok(());
        }

        let ap_mac = self.ap_mac_address()?;
        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp8266_hal_ap_netif_start(ap_mac.as_ptr())
        })
        .map_err(|_| DriverError::BackendRejected)?;
        self.ap_netif_started = true;
        Ok(())
    }

    fn stop_ap_netif(&mut self) {
        if !self.ap_netif_started {
            return;
        }

        let _ =
            crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp8266_hal_ap_netif_stop() });
        self.ap_netif_started = false;
    }
}

#[inline(never)]
fn reset_sta_rx_callback_debug() {
    rx_debug! {
        g_sta_rx_cb_calls.store(0, Ordering::Release);
        g_sta_rx_cb_enqueued.store(0, Ordering::Release);
        g_sta_rx_cb_rejected.store(0, Ordering::Release);
        g_sta_rx_cb_null_resources.store(0, Ordering::Release);
        g_sta_rx_cb_last_len.store(0, Ordering::Release);
        g_sta_rx_attach_calls.store(0, Ordering::Release);
        g_sta_rx_refresh_calls.store(0, Ordering::Release);
        g_sta_rx_detach_calls.store(0, Ordering::Release);
        g_sta_rx_register_calls.store(0, Ordering::Release);
        g_sta_rx_unregister_calls.store(0, Ordering::Release);
        g_sta_rx_last_register_ret.store(0, Ordering::Release);
        g_sta_rx_last_unregister_ret.store(0, Ordering::Release);
        g_sta_rx_expected_slot.store(0, Ordering::Release);
        g_ap_rx_cb_calls.store(0, Ordering::Release);
        g_ap_rx_cb_enqueued.store(0, Ordering::Release);
        g_ap_rx_cb_rejected.store(0, Ordering::Release);
        g_ap_rx_cb_null_resources.store(0, Ordering::Release);
        g_ap_rx_cb_last_len.store(0, Ordering::Release);
        g_ap_rx_attach_calls.store(0, Ordering::Release);
        g_ap_rx_refresh_calls.store(0, Ordering::Release);
        g_ap_rx_detach_calls.store(0, Ordering::Release);
        g_ap_rx_register_calls.store(0, Ordering::Release);
        g_ap_rx_unregister_calls.store(0, Ordering::Release);
        g_ap_rx_last_register_ret.store(0, Ordering::Release);
        g_ap_rx_last_unregister_ret.store(0, Ordering::Release);
        g_ap_rx_expected_slot.store(0, Ordering::Release);
    }
}

#[inline(always)]
fn ap_rx_release_only_enabled() -> bool {
    matches!(option_env!("ESP8266_HAL_AP_RX_RELEASE_ONLY"), Some("1"))
}

#[cfg(feature = "wifi-debug-trace")]
pub fn sta_rx_callback_debug_snapshot() -> RxCallbackDebugSnapshot {
    unsafe {
        RxCallbackDebugSnapshot {
            calls: g_sta_rx_cb_calls.load(Ordering::Acquire),
            enqueued: g_sta_rx_cb_enqueued.load(Ordering::Acquire),
            rejected: g_sta_rx_cb_rejected.load(Ordering::Acquire),
            null_resources: g_sta_rx_cb_null_resources.load(Ordering::Acquire),
            last_len: g_sta_rx_cb_last_len.load(Ordering::Acquire),
            attach_calls: g_sta_rx_attach_calls.load(Ordering::Acquire),
            refresh_calls: g_sta_rx_refresh_calls.load(Ordering::Acquire),
            detach_calls: g_sta_rx_detach_calls.load(Ordering::Acquire),
            register_calls: g_sta_rx_register_calls.load(Ordering::Acquire),
            unregister_calls: g_sta_rx_unregister_calls.load(Ordering::Acquire),
            last_register_ret: g_sta_rx_last_register_ret.load(Ordering::Acquire),
            last_unregister_ret: g_sta_rx_last_unregister_ret.load(Ordering::Acquire),
            sta_slot: sta_rxcb[0],
            ap_slot: sta_rxcb[1],
            expected_sta_slot: g_sta_rx_expected_slot.load(Ordering::Acquire),
            ap_calls: g_ap_rx_cb_calls.load(Ordering::Acquire),
            ap_enqueued: g_ap_rx_cb_enqueued.load(Ordering::Acquire),
            ap_rejected: g_ap_rx_cb_rejected.load(Ordering::Acquire),
            ap_null_resources: g_ap_rx_cb_null_resources.load(Ordering::Acquire),
            ap_last_len: g_ap_rx_cb_last_len.load(Ordering::Acquire),
            ap_attach_calls: g_ap_rx_attach_calls.load(Ordering::Acquire),
            ap_refresh_calls: g_ap_rx_refresh_calls.load(Ordering::Acquire),
            ap_detach_calls: g_ap_rx_detach_calls.load(Ordering::Acquire),
            ap_register_calls: g_ap_rx_register_calls.load(Ordering::Acquire),
            ap_unregister_calls: g_ap_rx_unregister_calls.load(Ordering::Acquire),
            ap_last_register_ret: g_ap_rx_last_register_ret.load(Ordering::Acquire),
            ap_last_unregister_ret: g_ap_rx_last_unregister_ret.load(Ordering::Acquire),
            expected_ap_slot: g_ap_rx_expected_slot.load(Ordering::Acquire),
        }
    }
}

#[cfg(feature = "wifi-debug-trace")]
pub fn deliver_data_debug_snapshot() -> DeliverDataDebugSnapshot {
    unsafe {
        DeliverDataDebugSnapshot {
            calls: core::ptr::read_volatile(
                &raw const crate::wifi::compat::pp_stage::g_ieee80211_deliver_data_calls,
            ),
            if0_calls: core::ptr::read_volatile(
                &raw const crate::wifi::compat::pp_stage::g_ieee80211_deliver_data_if0,
            ),
            if1_calls: core::ptr::read_volatile(
                &raw const crate::wifi::compat::pp_stage::g_ieee80211_deliver_data_if1,
            ),
            other_if_calls: core::ptr::read_volatile(
                &raw const crate::wifi::compat::pp_stage::g_ieee80211_deliver_data_other_if,
            ),
            last_if: core::ptr::read_volatile(
                &raw const crate::wifi::compat::pp_stage::g_ieee80211_deliver_data_last_if,
            ),
            last_len: core::ptr::read_volatile(
                &raw const crate::wifi::compat::pp_stage::g_ieee80211_deliver_data_last_len,
            ),
            last_sta_slot: core::ptr::read_volatile(
                &raw const crate::wifi::compat::pp_stage::g_ieee80211_deliver_data_last_sta_slot,
            ),
            last_ap_slot: core::ptr::read_volatile(
                &raw const crate::wifi::compat::pp_stage::g_ieee80211_deliver_data_last_ap_slot,
            ),
            cb_hit: core::ptr::read_volatile(
                &raw const crate::wifi::compat::pp_stage::g_ieee80211_deliver_data_cb_hit,
            ),
            cb_miss: core::ptr::read_volatile(
                &raw const crate::wifi::compat::pp_stage::g_ieee80211_deliver_data_cb_miss,
            ),
            last_cb_delta: core::ptr::read_volatile(
                &raw const crate::wifi::compat::pp_stage::g_ieee80211_deliver_data_last_cb_delta,
            ),
            heap_checks: core::ptr::read_volatile(
                &raw const crate::wifi::compat::pp_stage::g_ieee80211_deliver_data_heap_checks,
            ),
            last_free_heap: core::ptr::read_volatile(
                &raw const crate::wifi::compat::pp_stage::g_ieee80211_deliver_data_last_free_heap,
            ),
            alloc_attempts: core::ptr::read_volatile(
                &raw const crate::wifi::compat::pp_stage::g_ieee80211_deliver_data_alloc_attempts,
            ),
            alloc_failures: core::ptr::read_volatile(
                &raw const crate::wifi::compat::pp_stage::g_ieee80211_deliver_data_alloc_failures,
            ),
            last_alloc_size: core::ptr::read_volatile(
                &raw const crate::wifi::compat::pp_stage::g_ieee80211_deliver_data_last_alloc_size,
            ),
        }
    }
}

#[cfg(feature = "wifi-debug-trace")]
pub fn tx_debug_snapshot() -> super::TxDebugSnapshot {
    crate::wifi::sdk::wifi_init::internal_tx_debug_snapshot()
}

impl<
        'd,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    > Drop
    for Esp8266TargetBackend<
        'd,
        EVENTS,
        RX,
        SUBS,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
{
    fn drop(&mut self) {
        crate::wifi::event::disable_sdk_sta_auto_connect();
        self.detach_ap_rx();
        self.detach_sta_rx();
        self.stop_ap_netif();
        #[cfg(feature = "wifi-smartconfig")]
        self.unregister_smartconfig_event_handler();
        self.unregister_event_handler();
        self.registration = None;
        self.controller = None;
    }
}

impl<
        'd,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    > WifiLifecycleBackend
    for Esp8266TargetBackend<
        'd,
        EVENTS,
        RX,
        SUBS,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
{
    fn initialize(&mut self, config: &InitConfig) -> Result<(), InitError> {
        let mut controller = crate::wifi::WifiController::init(map_init_config(config))
            .map_err(map_wifi_init_error)?;
        controller
            .set_storage(crate::wifi::Storage::Ram)
            .map_err(map_wifi_init_error_from_driver)?;

        let registration = CallbackRegistration::install(self.resources)?;
        self.registration = Some(registration);
        self.controller = Some(controller);
        Ok(())
    }

    fn set_mode(&mut self, mode: RawMode) -> Result<(), DriverError> {
        self.controller_mut()?
            .set_mode(map_mode(mode))
            .map_err(map_wifi_driver_error)
    }

    fn start(&mut self) -> Result<(), DriverError> {
        let mode = self
            .controller()?
            .get_mode()
            .map_err(map_wifi_driver_error)?;
        let wait_for_sta_start = matches!(
            mode,
            crate::wifi::WifiMode::Sta | crate::wifi::WifiMode::ApSta
        );
        let sta_start_seq = CORE_STA_START_SEQ.load(Ordering::Acquire);
        self.apply_station_power_save_for_mode()?;
        self.register_event_handler()?;
        crate::wifi::event::disable_sdk_sta_auto_connect();

        if let Err(error) = self
            .controller_mut()?
            .start()
            .map_err(map_wifi_driver_error)
        {
            self.unregister_event_handler();
            return Err(error);
        }

        if wait_for_sta_start {
            let deadline = Deadline::new(self.now_ms(), STA_START_EVENT_TIMEOUT_MS);
            while CORE_STA_START_SEQ.load(Ordering::Acquire) == sta_start_seq {
                if deadline.is_expired(self.now_ms()) {
                    let _ = self.controller_mut()?.stop().map_err(map_wifi_driver_error);
                    self.unregister_event_handler();
                    return Err(DriverError::StaStartTimeout);
                }
                self.spin_once();
            }
        }

        Ok(())
    }

    fn stop(&mut self) -> Result<(), DriverError> {
        crate::wifi::event::disable_sdk_sta_auto_connect();
        self.detach_ap_rx();
        self.detach_sta_rx();
        self.stop_ap_netif();
        let enterprise_result = self.disable_wpa2_enterprise_if_enabled();
        let result = self.controller_mut()?.stop().map_err(map_wifi_driver_error);
        self.unregister_event_handler();
        enterprise_result.and(result)
    }
}

impl<
        'd,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    > WifiStaBackend
    for Esp8266TargetBackend<
        'd,
        EVENTS,
        RX,
        SUBS,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
{
    fn configure_sta(&mut self, config: &ClientConfig) -> Result<(), DriverError> {
        self.station_power_save = config.power_save();
        let controller = self.controller_mut()?;
        controller
            .set_mode(crate::wifi::WifiMode::Sta)
            .map_err(map_wifi_driver_error)?;
        controller
            .set_sta_config(&map_client_config(config))
            .map_err(map_wifi_driver_error)?;
        self.configure_wpa2_enterprise_for_station(config)?;
        Ok(())
    }

    fn connect(&mut self) -> Result<(), ConnectError> {
        let result = self
            .controller_mut()
            .map_err(|_| ConnectError::BackendRejected)?
            .connect()
            .map_err(map_wifi_connect_error);
        result
    }

    fn disconnect(&mut self) -> Result<(), DisconnectError> {
        let result = self
            .controller_mut()
            .map_err(|_| DisconnectError::BackendRejected)?
            .disconnect()
            .map_err(map_wifi_disconnect_error);
        if result.is_ok() && self.wpa2_enterprise_enabled {
            crate::wifi::compat::memory::release_eap_rsa_workspace();
        }
        result
    }

    fn sta_mac_address(&self) -> Result<[u8; 6], DriverError> {
        self.controller()?
            .get_mac(crate::wifi::controller::WifiInterface::Sta)
            .map_err(map_wifi_driver_error)
    }

    fn connect_blocking(&mut self, timeout_ms: u32) -> Option<Result<(), DriverError>> {
        Some(self.connect_blocking_inner(timeout_ms))
    }
}

impl<
        'd,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    > WifiApBackend
    for Esp8266TargetBackend<
        'd,
        EVENTS,
        RX,
        SUBS,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
{
    fn configure_ap(&mut self, config: &AccessPointConfig) -> Result<(), DriverError> {
        if matches!(config.credential(), Credential::Wpa2Enterprise(_)) {
            return Err(DriverError::BackendRejected);
        }
        self.disable_wpa2_enterprise_if_enabled()?;
        let controller = self.controller_mut()?;
        controller
            .set_mode(crate::wifi::WifiMode::Ap)
            .map_err(map_wifi_driver_error)?;
        controller
            .set_ap_config(&map_ap_config(config))
            .map_err(map_wifi_driver_error)
    }

    fn configure_mixed(
        &mut self,
        client: &ClientConfig,
        access_point: &AccessPointConfig,
    ) -> Result<(), DriverError> {
        if matches!(access_point.credential(), Credential::Wpa2Enterprise(_)) {
            return Err(DriverError::BackendRejected);
        }
        self.station_power_save = client.power_save();
        let controller = self.controller_mut()?;
        controller
            .set_mode(crate::wifi::WifiMode::ApSta)
            .map_err(map_wifi_driver_error)?;
        controller
            .set_sta_config(&map_client_config(client))
            .map_err(map_wifi_driver_error)?;
        controller
            .set_ap_config(&map_ap_config(access_point))
            .map_err(map_wifi_driver_error)?;
        self.configure_wpa2_enterprise_for_station(client)?;
        Ok(())
    }

    fn ap_mac_address(&self) -> Result<[u8; 6], DriverError> {
        self.controller()?
            .get_mac(crate::wifi::controller::WifiInterface::Ap)
            .map_err(map_wifi_driver_error)
    }

    fn connected_ap_stations<const N: usize>(
        &mut self,
    ) -> Result<heapless::Vec<ApStationInfo, N>, DriverError> {
        let mut raw: esp8266_wifi_sys::wifi_sta_list_t = unsafe { core::mem::zeroed() };
        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_wifi_ap_get_sta_list(&mut raw)
        })
        .map_err(|_| DriverError::BackendRejected)?;

        let mut stations = heapless::Vec::new();
        let count = (raw.num as usize).min(esp8266_wifi_sys::ESP_WIFI_MAX_CONN_NUM);
        for station in raw.sta[..count].iter() {
            stations
                .push(ApStationInfo::new(
                    station.mac,
                    station.phy_11b() != 0,
                    station.phy_11g() != 0,
                    station.phy_11n() != 0,
                ))
                .map_err(|_| DriverError::PacketOverflow)?;
        }

        Ok(stations)
    }

    fn deauth_ap_station(&mut self, aid: u16) -> Result<(), DriverError> {
        crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_wifi_deauth_sta(aid) })
            .map_err(|_| DriverError::BackendRejected)
    }
}

impl<
        'd,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    > WifiPacketBackend
    for Esp8266TargetBackend<
        'd,
        EVENTS,
        RX,
        SUBS,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
{
    fn attach_sta_rx(&mut self) -> Result<(), DriverError> {
        if self.sta_rx_registered {
            return Ok(());
        }

        rx_debug! { g_sta_rx_attach_calls.fetch_add(1, Ordering::AcqRel); }
        reset_sta_rx_callback_debug();
        rx_debug! { g_sta_rx_attach_calls.store(1, Ordering::Release); }
        self.register_sta_rx_callback()?;
        self.sta_rx_registered = true;
        Ok(())
    }

    fn attach_ap_rx(&mut self) -> Result<(), DriverError> {
        if self.ap_rx_registered {
            return Ok(());
        }

        rx_debug! { g_ap_rx_attach_calls.fetch_add(1, Ordering::AcqRel); }
        self.ensure_ap_netif_started()?;
        self.register_ap_rx_callback()?;
        self.ap_rx_registered = true;
        Ok(())
    }

    fn refresh_sta_rx(&mut self) -> Result<(), DriverError> {
        rx_debug! { g_sta_rx_refresh_calls.fetch_add(1, Ordering::AcqRel); }
        reset_sta_rx_callback_debug();
        rx_debug! { g_sta_rx_refresh_calls.store(1, Ordering::Release); }
        if self.sta_rx_registered {
            self.unregister_sta_rx_callback();
        }
        self.register_sta_rx_callback()?;
        self.sta_rx_registered = true;
        Ok(())
    }

    fn refresh_ap_rx(&mut self) -> Result<(), DriverError> {
        rx_debug! { g_ap_rx_refresh_calls.fetch_add(1, Ordering::AcqRel); }
        self.ensure_ap_netif_started()?;
        if self.ap_rx_registered {
            self.unregister_ap_rx_callback();
        }
        self.register_ap_rx_callback()?;
        self.ap_rx_registered = true;
        Ok(())
    }

    fn detach_sta_rx(&mut self) {
        if self.sta_rx_registered {
            rx_debug! { g_sta_rx_detach_calls.fetch_add(1, Ordering::AcqRel); }
            self.unregister_sta_rx_callback();
            self.sta_rx_registered = false;
        }
    }

    fn detach_ap_rx(&mut self) {
        if self.ap_rx_registered {
            rx_debug! { g_ap_rx_detach_calls.fetch_add(1, Ordering::AcqRel); }
            self.unregister_ap_rx_callback();
            self.ap_rx_registered = false;
        }
        self.stop_ap_netif();
    }

    fn transmit(&mut self, frame: &[u8]) -> Result<(), DriverError> {
        let ret = unsafe {
            esp8266_wifi_sys::esp_wifi_internal_tx(
                crate::wifi::controller::WifiInterface::Sta.to_raw(),
                frame.as_ptr() as *mut core::ffi::c_void,
                frame.len() as u16,
            )
        };

        match ret {
            0 => Ok(()),
            -1 => Err(DriverError::WouldBlock),
            _ => Err(DriverError::BackendRejected),
        }
    }

    fn transmit_ap(&mut self, frame: &[u8]) -> Result<(), DriverError> {
        let ret = unsafe {
            esp8266_wifi_sys::esp_wifi_internal_tx(
                crate::wifi::controller::WifiInterface::Ap.to_raw(),
                frame.as_ptr() as *mut core::ffi::c_void,
                frame.len() as u16,
            )
        };

        match ret {
            0 => Ok(()),
            -1 => Err(DriverError::WouldBlock),
            _ => Err(DriverError::BackendRejected),
        }
    }
}

impl<
        'd,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    > WifiScanBackend
    for Esp8266TargetBackend<
        'd,
        EVENTS,
        RX,
        SUBS,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
{
    fn scan_into<const N: usize>(
        &mut self,
        options: &ScanOptions,
        records: &mut heapless::Vec<AccessPointRecord, N>,
    ) -> Result<(), DriverError> {
        records.clear();
        let restore_events = self.event_handler_registered;
        if restore_events {
            self.unregister_event_handler();
        }

        let scan_result = start_blocking_scan(options).map_err(map_wifi_driver_error);
        let register_result = if restore_events {
            self.register_event_handler()
        } else {
            Ok(())
        };

        scan_result?;
        register_result?;
        collect_scan_records_into(records)
    }
}

#[inline(never)]
fn start_blocking_scan(options: &ScanOptions) -> Result<(), crate::wifi::WifiError> {
    clear_scan_prerequisites();
    if options.target_ssid().is_none() && options.target_bssid().is_none() {
        return start_blocking_scan_without_filters(options);
    }

    let legacy_config = map_scan_options(options);
    let mut buffers = crate::wifi::scan::RawScanConfigBuffers::from_config(&legacy_config);
    let raw_config = buffers.to_raw();
    crate::wifi::EspError::check(unsafe {
        esp8266_wifi_sys::esp_wifi_scan_start(&raw_config, true)
    })?;
    Ok(())
}

fn clear_scan_prerequisites() {
    unsafe {
        let _ = esp8266_wifi_sys::esp_wifi_scan_stop();
        let _ = esp8266_wifi_sys::esp_wifi_disconnect();
        let _ = esp8266_wifi_sys::esp_wifi_clear_fast_connect();
    }
}

#[inline(never)]
fn start_blocking_scan_without_filters(
    options: &ScanOptions,
) -> Result<(), crate::wifi::WifiError> {
    let timings = options.scan_timings();
    let raw_config = esp8266_wifi_sys::wifi_scan_config_t {
        ssid: core::ptr::null_mut(),
        bssid: core::ptr::null_mut(),
        channel: options.target_channel().map_or(0, Channel::get),
        show_hidden: options.shows_hidden(),
        scan_type: match options.scan_kind() {
            super::ScanType::Active => esp8266_wifi_sys::WIFI_SCAN_TYPE_ACTIVE,
            super::ScanType::Passive => esp8266_wifi_sys::WIFI_SCAN_TYPE_PASSIVE,
        },
        scan_time: match options.scan_kind() {
            super::ScanType::Active => esp8266_wifi_sys::wifi_scan_time_t {
                active: esp8266_wifi_sys::wifi_active_scan_time_t {
                    min: timings.active_min_ms(),
                    max: timings.active_max_ms(),
                },
            },
            super::ScanType::Passive => esp8266_wifi_sys::wifi_scan_time_t {
                passive: timings.passive_ms(),
            },
        },
    };

    crate::wifi::EspError::check(unsafe {
        esp8266_wifi_sys::esp_wifi_scan_start(&raw_config, true)
    })?;
    Ok(())
}

struct RawScanRecordBuffer {
    ptr: *mut esp8266_wifi_sys::wifi_ap_record_t,
}

impl RawScanRecordBuffer {
    fn new(len: usize) -> Result<Self, DriverError> {
        let size = core::mem::size_of::<esp8266_wifi_sys::wifi_ap_record_t>()
            .checked_mul(len)
            .ok_or(DriverError::OutOfMemory)?;
        let ptr = unsafe { crate::wifi::compat::memory::malloc(size) }
            as *mut esp8266_wifi_sys::wifi_ap_record_t;
        if ptr.is_null() {
            Err(DriverError::OutOfMemory)
        } else {
            Ok(Self { ptr })
        }
    }

    fn as_mut_ptr(&mut self) -> *mut esp8266_wifi_sys::wifi_ap_record_t {
        self.ptr
    }

    unsafe fn as_slice(&self, len: usize) -> &[esp8266_wifi_sys::wifi_ap_record_t] {
        unsafe { core::slice::from_raw_parts(self.ptr, len) }
    }
}

impl Drop for RawScanRecordBuffer {
    fn drop(&mut self) {
        unsafe { crate::wifi::compat::memory::free(self.ptr.cast()) };
    }
}

#[inline(never)]
fn collect_scan_records_into<const N: usize>(
    records: &mut heapless::Vec<AccessPointRecord, N>,
) -> Result<(), DriverError> {
    let mut count: u16 = 0;
    crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_wifi_scan_get_ap_num(&mut count) })
        .map_err(|_| DriverError::BackendRejected)?;

    if N == 0 || count == 0 {
        return Ok(());
    }

    let fetch = (count as usize).min(N) as u16;
    let mut raw_records = RawScanRecordBuffer::new(fetch as usize)?;
    let mut actual = fetch;
    crate::wifi::EspError::check(unsafe {
        esp8266_wifi_sys::esp_wifi_scan_get_ap_records(&mut actual, raw_records.as_mut_ptr())
    })
    .map_err(|_| DriverError::BackendRejected)?;

    for raw_record in unsafe { raw_records.as_slice(actual as usize) } {
        let ssid = crate::wifi::bytes_to_str(&raw_record.ssid);
        let channel = match Channel::try_from(raw_record.primary) {
            Ok(channel) => channel,
            Err(_) => continue,
        };
        let Ok(record) = AccessPointRecord::new(
            ssid,
            raw_record.bssid,
            channel,
            raw_record.rssi,
            crate::wifi::AuthMode::from_raw(raw_record.authmode).to_raw() as u8,
            raw_record.wps() != 0,
        ) else {
            continue;
        };
        records
            .push(record)
            .map_err(|_| DriverError::PacketOverflow)?;
    }

    Ok(())
}

impl<
        'd,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    > WifiFeatureBackend
    for Esp8266TargetBackend<
        'd,
        EVENTS,
        RX,
        SUBS,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
{
    fn set_channel(&mut self, channel: Channel) -> Result<(), DriverError> {
        let result = crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_wifi_set_channel(
                channel.get(),
                esp8266_wifi_sys::WIFI_SECOND_CHAN_NONE,
            )
        })
        .map_err(|_| DriverError::BackendRejected);
        if result.is_ok() {
            PROMISCUOUS_CHANNEL.store(channel.get() as u32, Ordering::Release);
        }
        result
    }

    fn enable_promiscuous(&mut self, filter: PromiscuousFilter) -> Result<(), DriverError> {
        let raw_filter = esp8266_wifi_sys::wifi_promiscuous_filter_t {
            filter_mask: map_promiscuous_filter(filter),
        };

        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_wifi_set_promiscuous_filter(&raw_filter)
        })
        .map_err(|_| DriverError::BackendRejected)?;
        PROMISCUOUS_FILTER_BITS.store(0, Ordering::Release);

        crate::wifi::compat::port::set_wdev_tick_poll_enabled(true);
        for _ in 0..4 {
            self.spin_once();
        }
        if let Err(error) = crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_wifi_set_promiscuous(true)
        })
        .map_err(|_| DriverError::BackendRejected)
        {
            crate::wifi::compat::port::set_wdev_tick_poll_enabled(false);
            PROMISCUOUS_FILTER_BITS.store(0, Ordering::Release);
            return Err(error);
        }

        if let Err(error) = crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_wifi_set_promiscuous_rx_cb(Some(
                promiscuous_callback::<
                    EVENTS,
                    RX,
                    SUBS,
                    ESP_NOW_PEERS,
                    ESP_NOW_RX,
                    PROMISCUOUS_RX,
                    AP_RX_SLOTS,
                    AP_RX_BYTES,
                >,
            ))
        })
        .map_err(|_| DriverError::BackendRejected)
        {
            let _ = unsafe { esp8266_wifi_sys::esp_wifi_set_promiscuous(false) };
            crate::wifi::compat::port::set_wdev_tick_poll_enabled(false);
            PROMISCUOUS_FILTER_BITS.store(0, Ordering::Release);
            return Err(error);
        }

        PROMISCUOUS_FILTER_BITS.store(filter.bits() as u32, Ordering::Release);
        Ok(())
    }

    fn set_promiscuous_control_filter(
        &mut self,
        filter: PromiscuousControlFilter,
    ) -> Result<(), DriverError> {
        let raw_filter = esp8266_wifi_sys::wifi_promiscuous_filter_t {
            filter_mask: map_promiscuous_control_filter(filter),
        };

        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_wifi_set_promiscuous_ctrl_filter(&raw_filter)
        })
        .map_err(|_| DriverError::BackendRejected)
    }

    fn disable_promiscuous(&mut self) -> Result<(), DriverError> {
        let clear_result = crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_wifi_set_promiscuous_rx_cb(None)
        })
        .map_err(|_| DriverError::BackendRejected);
        PROMISCUOUS_FILTER_BITS.store(0, Ordering::Release);
        PROMISCUOUS_CHANNEL.store(0, Ordering::Release);
        let disable_result = crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_wifi_set_promiscuous(false)
        })
        .map_err(|_| DriverError::BackendRejected);
        crate::wifi::compat::port::set_wdev_tick_poll_enabled(false);

        clear_result.and(disable_result)
    }

    fn enable_esp_now(&mut self) -> Result<(), DriverError> {
        crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_now_init() })
            .map_err(|_| DriverError::BackendRejected)?;

        if crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_now_register_recv_cb(Some(
                esp_now_recv_callback::<
                    EVENTS,
                    RX,
                    SUBS,
                    ESP_NOW_PEERS,
                    ESP_NOW_RX,
                    PROMISCUOUS_RX,
                    AP_RX_SLOTS,
                    AP_RX_BYTES,
                >,
            ))
        })
        .is_err()
        {
            let _ = unsafe { esp8266_wifi_sys::esp_now_deinit() };
            return Err(DriverError::BackendRejected);
        }

        if crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_now_register_send_cb(Some(
                esp_now_send_callback::<
                    EVENTS,
                    RX,
                    SUBS,
                    ESP_NOW_PEERS,
                    ESP_NOW_RX,
                    PROMISCUOUS_RX,
                    AP_RX_SLOTS,
                    AP_RX_BYTES,
                >,
            ))
        })
        .is_err()
        {
            let _ = unsafe { esp8266_wifi_sys::esp_now_unregister_recv_cb() };
            let _ = unsafe { esp8266_wifi_sys::esp_now_deinit() };
            return Err(DriverError::BackendRejected);
        }

        Ok(())
    }

    fn disable_esp_now(&mut self) -> Result<(), DriverError> {
        let unregister_send_result =
            crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_now_unregister_send_cb() })
                .map_err(|_| DriverError::BackendRejected);
        let unregister_result =
            crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_now_unregister_recv_cb() })
                .map_err(|_| DriverError::BackendRejected);
        let deinit_result =
            crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_now_deinit() })
                .map_err(|_| DriverError::BackendRejected);
        unregister_send_result
            .and(unregister_result)
            .and(deinit_result)
    }

    fn set_esp_now_pmk(&mut self, pmk: &[u8; 16]) -> Result<(), DriverError> {
        crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_now_set_pmk(pmk.as_ptr()) })
            .map_err(|_| DriverError::BackendRejected)
    }

    fn add_esp_now_peer(&mut self, peer: &EspNowPeerConfig) -> Result<(), DriverError> {
        let raw = raw_esp_now_peer(peer);

        crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_now_add_peer(&raw) })
            .map_err(|_| DriverError::BackendRejected)
    }

    fn modify_esp_now_peer(&mut self, peer: &EspNowPeerConfig) -> Result<(), DriverError> {
        let raw = raw_esp_now_peer(peer);

        crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_now_mod_peer(&raw) })
            .map_err(|_| DriverError::BackendRejected)
    }

    fn remove_esp_now_peer(&mut self, peer_addr: &[u8; 6]) -> Result<(), DriverError> {
        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_now_del_peer(peer_addr.as_ptr())
        })
        .map_err(|_| DriverError::BackendRejected)
    }

    fn send_esp_now(&mut self, peer_addr: &[u8; 6], payload: &[u8]) -> Result<(), DriverError> {
        if payload.len() > super::ESP_NOW_MAX_PAYLOAD {
            return Err(DriverError::PayloadTooLong);
        }

        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_now_send(peer_addr.as_ptr(), payload.as_ptr(), payload.len())
        })
        .map_err(|_| DriverError::BackendRejected)
    }

    fn enable_wps(&mut self, config: &WpsConfig) -> Result<(), DriverError> {
        self.register_event_handler()?;
        let pin = config.pin_override();
        unsafe {
            esp8266_hal_wps_set_pin(
                pin.as_ref()
                    .map_or(core::ptr::null(), |pin| pin.as_bytes().as_ptr()),
            );
            if let Some(ssid) = config.target_ssid() {
                esp8266_hal_wps_set_target_ssid(
                    ssid.as_bytes().as_ptr(),
                    ssid.as_bytes().len() as u32,
                );
            } else {
                esp8266_hal_wps_set_target_ssid(core::ptr::null(), 0);
            }
        }
        let raw = raw_wps_config(config);
        crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_wifi_wps_enable(&raw) })
            .map_err(|error| DriverError::BackendError(error.code()))
    }

    fn start_wps(&mut self, timeout_ms: i32) -> Result<(), DriverError> {
        crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_wifi_wps_start(timeout_ms) })
            .map_err(|error| DriverError::BackendError(error.code()))
    }

    fn disable_wps(&mut self) -> Result<(), DriverError> {
        let result =
            crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_wifi_wps_disable() })
                .map_err(|error| DriverError::BackendError(error.code()));
        unsafe {
            esp8266_hal_wps_set_pin(core::ptr::null());
            esp8266_hal_wps_set_target_ssid(core::ptr::null(), 0);
        }
        result
    }

    #[cfg(feature = "wifi-smartconfig")]
    fn set_smartconfig_type(&mut self, kind: SmartConfigType) -> Result<(), DriverError> {
        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_smartconfig_set_type(map_smartconfig_type(kind))
        })
        .map_err(|_| DriverError::BackendRejected)
    }

    #[cfg(feature = "wifi-smartconfig")]
    fn set_smartconfig_timeout(&mut self, timeout: SmartConfigTimeout) -> Result<(), DriverError> {
        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_esptouch_set_timeout(timeout.seconds())
        })
        .map_err(|_| DriverError::BackendRejected)
    }

    #[cfg(feature = "wifi-smartconfig")]
    fn set_smartconfig_fast_mode(&mut self, enabled: bool) -> Result<(), DriverError> {
        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_smartconfig_fast_mode(enabled)
        })
        .map_err(|_| DriverError::BackendRejected)
    }

    #[cfg(feature = "wifi-smartconfig")]
    fn start_smartconfig(&mut self, config: &SmartConfigStartConfig) -> Result<(), DriverError> {
        self.register_smartconfig_event_handler()?;
        if unsafe { crate::wifi::compat::timer::start_ets_timer_task() } != 1 {
            self.unregister_smartconfig_event_handler();
            return Err(DriverError::OutOfMemory);
        }
        let mut key = [0_i8; 17];
        if let Some(value) = config.esp_touch_v2_key_value() {
            for (dst, src) in key[..16].iter_mut().zip(value) {
                *dst = src as i8;
            }
        }
        let raw = esp8266_wifi_sys::smartconfig_start_config_t {
            enable_log: config.log_enabled(),
            esp_touch_v2_enable_crypt: config.esp_touch_v2_key_value().is_some(),
            esp_touch_v2_key: if config.esp_touch_v2_key_value().is_some() {
                key.as_mut_ptr()
            } else {
                core::ptr::null_mut()
            },
        };

        crate::wifi::compat::port::set_wdev_tick_poll_enabled(true);
        if crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_smartconfig_start(&raw) })
            .is_err()
        {
            crate::wifi::compat::port::set_wdev_tick_poll_enabled(false);
            return Err(DriverError::BackendRejected);
        }
        Ok(())
    }

    #[cfg(feature = "wifi-smartconfig")]
    fn stop_smartconfig(&mut self) -> Result<(), DriverError> {
        let stop_result =
            crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_smartconfig_stop() })
                .map_err(|_| DriverError::BackendRejected);
        crate::wifi::compat::port::set_wdev_tick_poll_enabled(false);
        self.unregister_smartconfig_event_handler();
        stop_result
    }

    #[cfg(feature = "wifi-smartconfig")]
    fn read_smartconfig_reserved_data(&mut self, buffer: &mut [u8]) -> Result<(), DriverError> {
        if buffer.is_empty() || buffer.len() > u8::MAX as usize {
            return Err(DriverError::PayloadTooLong);
        }

        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_smartconfig_get_rvd_data(buffer.as_mut_ptr(), buffer.len() as u8)
        })
        .map_err(|_| DriverError::BackendRejected)
    }
}

impl<
        'd,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    > WifiRuntimeBackend
    for Esp8266TargetBackend<
        'd,
        EVENTS,
        RX,
        SUBS,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
{
    fn now_ms(&self) -> u64 {
        let _ = self.controller();
        // The compat scheduler advances the FreeRTOS tick reliably while the
        // current task blocks/yields in SDK WiFi calls. `esp_timer_get_time()`
        // is not a dependable deadline source in those paths on this port.
        unsafe { u64::from(crate::wifi::compat::task::xTaskGetTickCount()).saturating_mul(10) }
    }

    fn spin_once(&mut self) {
        unsafe { crate::wifi::compat::task::vTaskDelay(1) };
    }
}

unsafe fn release_rx_ownership(buffer: *mut core::ffi::c_void, eb: *mut core::ffi::c_void) {
    if !eb.is_null() {
        unsafe { esp8266_wifi_sys::esp_wifi_internal_free_rx_buffer(eb) };
    } else if !buffer.is_null() {
        unsafe { crate::wifi::compat::memory::free(buffer) };
    }
}

unsafe extern "C" fn wifi_core_event_trampoline<
    const EVENTS: usize,
    const RX: usize,
    const SUBS: usize,
    const ESP_NOW_PEERS: usize,
    const ESP_NOW_RX: usize,
    const PROMISCUOUS_RX: usize,
    const AP_RX_SLOTS: usize,
    const AP_RX_BYTES: usize,
>(
    _arg: *mut core::ffi::c_void,
    _base: esp8266_wifi_sys::esp_event_base_t,
    event_id: i32,
    event_data: *mut core::ffi::c_void,
) {
    use crate::wifi::controller::{note_sta_disconnect, StaConnectionState, AP_STATE, STA_STATE};

    let resources = unsafe {
        super::target_callback::active_resources::<
            EVENTS,
            RX,
            SUBS,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >()
    };
    if resources.is_null() {
        return;
    }
    let resources = unsafe { &*resources };
    let event = match event_id as u32 {
        esp8266_wifi_sys::WIFI_EVENT_STA_START => {
            CORE_STA_START_SEQ.fetch_add(1, Ordering::AcqRel);
            STA_STATE.store(StaConnectionState::Connecting as u8, Ordering::Release);
            Some(WifiEvent::StaStarted)
        }
        esp8266_wifi_sys::WIFI_EVENT_SCAN_DONE => {
            if event_data.is_null() {
                Some(WifiEvent::ScanDone {
                    count: 0,
                    success: false,
                })
            } else {
                let done = unsafe {
                    &*(event_data as *const esp8266_wifi_sys::wifi_event_sta_scan_done_t)
                };
                Some(WifiEvent::ScanDone {
                    count: done.number,
                    success: done.status == 0,
                })
            }
        }
        esp8266_wifi_sys::WIFI_EVENT_STA_STOP => {
            STA_STATE.store(StaConnectionState::Disconnected as u8, Ordering::Release);
            Some(WifiEvent::StaStopped)
        }
        esp8266_wifi_sys::WIFI_EVENT_STA_CONNECTED => {
            STA_STATE.store(StaConnectionState::Connected as u8, Ordering::Release);
            Some(WifiEvent::StaConnected)
        }
        esp8266_wifi_sys::WIFI_EVENT_STA_DISCONNECTED => {
            let reason = if event_data.is_null() {
                None
            } else {
                let disconnected = unsafe {
                    &*(event_data as *const esp8266_wifi_sys::wifi_event_sta_disconnected_t)
                };
                Some(disconnected.reason as u8)
            };

            note_sta_disconnect(crate::wifi::event::DisconnectReason::from_raw(
                reason.unwrap_or(esp8266_wifi_sys::WIFI_REASON_UNSPECIFIED as u8),
            ));
            Some(WifiEvent::StaDisconnected { reason })
        }
        esp8266_wifi_sys::WIFI_EVENT_STA_WPS_ER_SUCCESS => Some(WifiEvent::WpsSucceeded),
        esp8266_wifi_sys::WIFI_EVENT_STA_WPS_ER_FAILED => {
            let reason = if event_data.is_null() {
                None
            } else {
                Some(unsafe {
                    *(event_data as *const esp8266_wifi_sys::wifi_event_sta_wps_fail_reason_t)
                        as u32
                })
            };
            Some(WifiEvent::WpsFailed { reason })
        }
        esp8266_wifi_sys::WIFI_EVENT_STA_WPS_ER_TIMEOUT => Some(WifiEvent::WpsTimedOut),
        esp8266_wifi_sys::WIFI_EVENT_STA_WPS_ER_PIN => {
            if event_data.is_null() {
                None
            } else {
                let pin = unsafe {
                    &*(event_data as *const esp8266_wifi_sys::wifi_event_sta_wps_er_pin_t)
                };
                WpsPin::from_digits(pin.pin_code)
                    .ok()
                    .map(|pin| WifiEvent::WpsPin { pin })
            }
        }
        esp8266_wifi_sys::WIFI_EVENT_AP_START => {
            AP_STATE.store(1, Ordering::Release);
            Some(WifiEvent::ApStarted)
        }
        esp8266_wifi_sys::WIFI_EVENT_AP_STOP => {
            AP_STATE.store(0, Ordering::Release);
            Some(WifiEvent::ApStopped)
        }
        _ => None,
    };

    if let Some(event) = event {
        let _ = resources.publish_event(event);
    }
}

#[cfg(feature = "wifi-smartconfig")]
unsafe extern "C" fn smartconfig_event_trampoline<
    const EVENTS: usize,
    const RX: usize,
    const SUBS: usize,
    const ESP_NOW_PEERS: usize,
    const ESP_NOW_RX: usize,
    const PROMISCUOUS_RX: usize,
    const AP_RX_SLOTS: usize,
    const AP_RX_BYTES: usize,
>(
    _arg: *mut core::ffi::c_void,
    _base: esp8266_wifi_sys::esp_event_base_t,
    event_id: i32,
    event_data: *mut core::ffi::c_void,
) {
    let raw_event_id = event_id as u32;
    #[cfg(esp8266_hal_smartconfig_event_trace)]
    {
        SC_EVENT_TRACE_CALLS.fetch_add(1, Ordering::AcqRel);
        SC_EVENT_TRACE_LAST_ID.store(raw_event_id, Ordering::Release);
        SC_EVENT_TRACE_LAST_DATA.store(event_data as u32, Ordering::Release);
        if raw_event_id == esp8266_wifi_sys::SC_EVENT_GOT_SSID_PSWD {
            SC_EVENT_TRACE_GOT_CALLS.fetch_add(1, Ordering::AcqRel);
        }
    }

    let resources = unsafe {
        super::target_callback::active_resources::<
            EVENTS,
            RX,
            SUBS,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >()
    };
    if resources.is_null() {
        #[cfg(esp8266_hal_smartconfig_event_trace)]
        SC_EVENT_TRACE_NULL_RESOURCES.fetch_add(1, Ordering::AcqRel);
        return;
    }
    let resources = unsafe { &*resources };

    let event = match raw_event_id {
        esp8266_wifi_sys::SC_EVENT_SCAN_DONE => Some(WifiEvent::SmartConfigScanDone),
        esp8266_wifi_sys::SC_EVENT_FOUND_CHANNEL => Some(WifiEvent::SmartConfigFoundChannel),
        esp8266_wifi_sys::SC_EVENT_SEND_ACK_DONE => Some(WifiEvent::SmartConfigSendAckDone),
        esp8266_wifi_sys::SC_EVENT_GOT_SSID_PSWD => {
            if event_data.is_null() {
                None
            } else {
                let data = unsafe {
                    &*(event_data as *const esp8266_wifi_sys::smartconfig_event_got_ssid_pswd_t)
                };
                let ssid = &data.ssid[..nul_terminated_len(&data.ssid)];
                let password = &data.password[..nul_terminated_len(&data.password)];
                let bssid = if data.bssid_set {
                    Some(data.bssid)
                } else {
                    None
                };
                SmartConfigCredentials::new(
                    ssid,
                    password,
                    bssid,
                    map_smartconfig_type_from_raw(data.type_),
                    data.token,
                    Ipv4Addr::new(
                        data.cellphone_ip[0],
                        data.cellphone_ip[1],
                        data.cellphone_ip[2],
                        data.cellphone_ip[3],
                    ),
                )
                .ok()
                .map(|credentials| WifiEvent::SmartConfigGotCredentials { credentials })
            }
        }
        _ => None,
    };

    if let Some(event) = event {
        #[cfg(esp8266_hal_smartconfig_event_trace)]
        {
            SC_EVENT_TRACE_PUBLISHED.fetch_add(1, Ordering::AcqRel);
            if raw_event_id == esp8266_wifi_sys::SC_EVENT_GOT_SSID_PSWD {
                SC_EVENT_TRACE_GOT_ACCEPTED.fetch_add(1, Ordering::AcqRel);
            }
        }
        let _ = resources.publish_event(event);
    } else {
        #[cfg(esp8266_hal_smartconfig_event_trace)]
        {
            if raw_event_id == esp8266_wifi_sys::SC_EVENT_GOT_SSID_PSWD {
                SC_EVENT_TRACE_GOT_REJECTED.fetch_add(1, Ordering::AcqRel);
            }
        }
    }
}

unsafe extern "C" fn rx_callback_sta<
    const EVENTS: usize,
    const RX: usize,
    const SUBS: usize,
    const ESP_NOW_PEERS: usize,
    const ESP_NOW_RX: usize,
    const PROMISCUOUS_RX: usize,
    const AP_RX_SLOTS: usize,
    const AP_RX_BYTES: usize,
>(
    buffer: *mut core::ffi::c_void,
    len: u16,
    eb: *mut core::ffi::c_void,
) -> esp8266_wifi_sys::esp_err_t {
    rx_debug! {
        g_sta_rx_cb_calls.fetch_add(1, Ordering::AcqRel);
        g_sta_rx_cb_last_len.store(len as u32, Ordering::Release);
    }
    let resources = unsafe {
        super::target_callback::active_resources::<
            EVENTS,
            RX,
            SUBS,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >()
    };
    if resources.is_null() {
        rx_debug! { g_sta_rx_cb_null_resources.fetch_add(1, Ordering::AcqRel); }
        unsafe { release_rx_ownership(buffer, eb) };
        return esp8266_wifi_sys::ESP_OK;
    }

    let resources = unsafe { &*resources };
    let record = unsafe { RawRxRecord::from_raw(buffer, len, eb, Some(release_rx_ownership)) };
    if resources.enqueue_rx(record).is_ok() {
        rx_debug! { g_sta_rx_cb_enqueued.fetch_add(1, Ordering::AcqRel); }
    } else {
        rx_debug! { g_sta_rx_cb_rejected.fetch_add(1, Ordering::AcqRel); }
    }
    esp8266_wifi_sys::ESP_OK
}

unsafe extern "C" fn rx_callback_ap<
    const EVENTS: usize,
    const RX: usize,
    const SUBS: usize,
    const ESP_NOW_PEERS: usize,
    const ESP_NOW_RX: usize,
    const PROMISCUOUS_RX: usize,
    const AP_RX_SLOTS: usize,
    const AP_RX_BYTES: usize,
>(
    buffer: *mut core::ffi::c_void,
    len: u16,
    eb: *mut core::ffi::c_void,
) -> esp8266_wifi_sys::esp_err_t {
    rx_debug! {
        g_ap_rx_cb_calls.fetch_add(1, Ordering::AcqRel);
        g_ap_rx_cb_last_len.store(len as u32, Ordering::Release);
    }
    let resources = unsafe {
        super::target_callback::active_resources::<
            EVENTS,
            RX,
            SUBS,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >()
    };
    if resources.is_null() {
        rx_debug! { g_ap_rx_cb_null_resources.fetch_add(1, Ordering::AcqRel); }
        unsafe { release_rx_ownership(buffer, eb) };
        return esp8266_wifi_sys::ESP_OK;
    }

    let resources = unsafe { &*resources };
    let enqueue = if ap_rx_release_only_enabled() {
        Ok(())
    } else {
        resources.enqueue_ap_rx_copy(buffer, len)
    };
    unsafe { release_rx_ownership(buffer, eb) };
    if enqueue.is_ok() {
        rx_debug! { g_ap_rx_cb_enqueued.fetch_add(1, Ordering::AcqRel); }
    } else {
        rx_debug! { g_ap_rx_cb_rejected.fetch_add(1, Ordering::AcqRel); }
    }
    esp8266_wifi_sys::ESP_OK
}

unsafe extern "C" fn promiscuous_callback<
    const EVENTS: usize,
    const RX: usize,
    const SUBS: usize,
    const ESP_NOW_PEERS: usize,
    const ESP_NOW_RX: usize,
    const PROMISCUOUS_RX: usize,
    const AP_RX_SLOTS: usize,
    const AP_RX_BYTES: usize,
>(
    buffer: *mut core::ffi::c_void,
    packet_type: esp8266_wifi_sys::wifi_promiscuous_pkt_type_t,
) {
    if buffer.is_null() {
        return;
    }

    let resources = unsafe {
        super::target_callback::active_resources::<
            EVENTS,
            RX,
            SUBS,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >()
    };
    if resources.is_null() {
        return;
    }

    let packet = unsafe { &*(buffer as *const esp8266_wifi_sys::wifi_promiscuous_pkt_t) };
    let packet_type = map_promiscuous_packet_type(packet_type);
    let payload_len = if packet_type == PromiscuousPacketType::Control {
        0
    } else if packet.rx_ctrl.sig_mode() != 0 {
        packet.rx_ctrl.ht_length() as usize
    } else {
        packet.rx_ctrl.legacy_length() as usize
    }
    .min(PROMISCUOUS_MAX_PAYLOAD_PREFIX);

    let payload = if payload_len == 0 {
        &[]
    } else {
        unsafe { core::slice::from_raw_parts(packet.payload.as_ptr(), payload_len) }
    };
    let active_filter = PromiscuousFilter::from_bits_truncate(
        PROMISCUOUS_FILTER_BITS.load(Ordering::Acquire) as u8,
    );
    if active_filter.is_empty() || !active_filter.contains(packet_type) {
        return;
    }
    let raw_channel = packet.rx_ctrl.channel();
    let channel = if (1..=14).contains(&raw_channel) {
        raw_channel
    } else {
        PROMISCUOUS_CHANNEL.load(Ordering::Acquire) as u8
    };
    if !(1..=14).contains(&channel) {
        return;
    }

    let info = PromiscuousFrameInfo {
        rssi: packet.rx_ctrl.rssi(),
        channel,
        packet_type,
    };

    if let Ok(frame) = PromiscuousRxFrame::new(info, payload) {
        let _ = unsafe { &*resources }.enqueue_promiscuous_rx(frame);
    }
}

unsafe extern "C" fn esp_now_recv_callback<
    const EVENTS: usize,
    const RX: usize,
    const SUBS: usize,
    const ESP_NOW_PEERS: usize,
    const ESP_NOW_RX: usize,
    const PROMISCUOUS_RX: usize,
    const AP_RX_SLOTS: usize,
    const AP_RX_BYTES: usize,
>(
    peer_addr: *const u8,
    payload: *const u8,
    payload_len: core::ffi::c_int,
) {
    if peer_addr.is_null() || payload_len < 0 || (payload_len > 0 && payload.is_null()) {
        return;
    }

    let resources = unsafe {
        super::target_callback::active_resources::<
            EVENTS,
            RX,
            SUBS,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >()
    };
    if resources.is_null() {
        return;
    }

    let peer_addr = unsafe { *(peer_addr as *const [u8; 6]) };
    let payload = if payload_len == 0 {
        &[]
    } else {
        unsafe { core::slice::from_raw_parts(payload, payload_len as usize) }
    };
    if let Ok(frame) = EspNowRxFrame::new(peer_addr, payload) {
        let _ = unsafe { &*resources }.enqueue_esp_now_rx(frame);
    }
}

unsafe extern "C" fn esp_now_send_callback<
    const EVENTS: usize,
    const RX: usize,
    const SUBS: usize,
    const ESP_NOW_PEERS: usize,
    const ESP_NOW_RX: usize,
    const PROMISCUOUS_RX: usize,
    const AP_RX_SLOTS: usize,
    const AP_RX_BYTES: usize,
>(
    peer_addr: *const u8,
    status: esp8266_wifi_sys::esp_now_send_status_t,
) {
    if peer_addr.is_null() {
        return;
    }

    let resources = unsafe {
        super::target_callback::active_resources::<
            EVENTS,
            RX,
            SUBS,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >()
    };
    if resources.is_null() {
        return;
    }

    let peer_addr = unsafe { *(peer_addr as *const [u8; 6]) };
    let status = match status {
        esp8266_wifi_sys::ESP_NOW_SEND_SUCCESS => EspNowSendStatus::Success,
        _ => EspNowSendStatus::Failed,
    };
    let _ =
        unsafe { &*resources }.enqueue_esp_now_tx_status(EspNowTxStatus::new(peer_addr, status));
}

fn map_wifi_init_error(error: crate::wifi::WifiError) -> InitError {
    match error {
        crate::wifi::WifiError::Internal(crate::wifi::EspError(code))
            if code == esp8266_wifi_sys::ESP_ERR_INVALID_STATE =>
        {
            InitError::AlreadyInitialized
        }
        _ => InitError::BackendUnavailable,
    }
}

fn map_wifi_init_error_from_driver(error: crate::wifi::WifiError) -> InitError {
    match error {
        crate::wifi::WifiError::Internal(crate::wifi::EspError(code))
            if code == esp8266_wifi_sys::ESP_ERR_INVALID_STATE =>
        {
            InitError::AlreadyInitialized
        }
        _ => InitError::BackendUnavailable,
    }
}

fn map_wifi_driver_error(error: crate::wifi::WifiError) -> DriverError {
    match error {
        crate::wifi::WifiError::Timeout => DriverError::Timeout,
        _ => DriverError::BackendRejected,
    }
}

fn map_enterprise_esp_error(error: crate::wifi::EspError) -> DriverError {
    if error.code() == esp8266_wifi_sys::ESP_ERR_NO_MEM {
        DriverError::OutOfMemory
    } else {
        DriverError::BackendError(error.code())
    }
}

fn map_wifi_connect_error(error: crate::wifi::WifiError) -> ConnectError {
    match error {
        crate::wifi::WifiError::Timeout => ConnectError::Timeout,
        crate::wifi::WifiError::Disconnected(_) => ConnectError::Disconnected,
        _ => ConnectError::BackendRejected,
    }
}

fn map_wifi_disconnect_error(error: crate::wifi::WifiError) -> DisconnectError {
    match error {
        crate::wifi::WifiError::Timeout => DisconnectError::BackendRejected,
        _ => DisconnectError::BackendRejected,
    }
}
