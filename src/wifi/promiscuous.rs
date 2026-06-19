//! Promiscuous (monitor) mode support.
//!
//! Requires the `promiscuous` feature flag.

use esp8266_wifi_sys as sys;
use portable_atomic::{AtomicPtr, Ordering};

use crate::wifi::error::{EspError, WifiError};

/// Promiscuous mode packet type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketType {
    /// Management frame (beacon, probe, auth, etc.).
    Management,
    /// Control frame (ACK, RTS, CTS, etc.).
    Control,
    /// Data frame.
    Data,
    /// Other (MIMO, etc.) — payload is zero-length.
    Misc,
}

impl PacketType {
    fn from_raw(raw: sys::wifi_promiscuous_pkt_type_t) -> Self {
        match raw {
            sys::WIFI_PKT_MGMT => Self::Management,
            sys::WIFI_PKT_CTRL => Self::Control,
            sys::WIFI_PKT_DATA => Self::Data,
            _ => Self::Misc,
        }
    }
}

/// Metadata for a received promiscuous-mode packet.
#[derive(Debug, Clone)]
pub struct RxPacketInfo {
    /// Signal strength in dBm.
    pub rssi: i8,
    /// Channel number.
    pub channel: u8,
    /// Packet type.
    pub packet_type: PacketType,
}

bitflags::bitflags! {
    /// Filter mask for promiscuous mode packet types.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct PromiscuousFilter: u32 {
        const MANAGEMENT = sys::WIFI_PROMIS_FILTER_MASK_MGMT;
        const CONTROL = sys::WIFI_PROMIS_FILTER_MASK_CTRL;
        const DATA = sys::WIFI_PROMIS_FILTER_MASK_DATA;
        const MISC = sys::WIFI_PROMIS_FILTER_MASK_MISC;
        const ALL = Self::MANAGEMENT.bits()
            | Self::CONTROL.bits()
            | Self::DATA.bits()
            | Self::MISC.bits();
    }
}

bitflags::bitflags! {
    /// Filter mask for control frame subtypes in promiscuous mode.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct ControlFilter: u32 {
        const WRAPPER = sys::WIFI_PROMIS_CTRL_FILTER_MASK_WRAPPER;
        const BAR = sys::WIFI_PROMIS_CTRL_FILTER_MASK_BAR;
        const BA = sys::WIFI_PROMIS_CTRL_FILTER_MASK_BA;
        const PSPOLL = sys::WIFI_PROMIS_CTRL_FILTER_MASK_PSPOLL;
        const RTS = sys::WIFI_PROMIS_CTRL_FILTER_MASK_RTS;
        const CTS = sys::WIFI_PROMIS_CTRL_FILTER_MASK_CTS;
        const ACK = sys::WIFI_PROMIS_CTRL_FILTER_MASK_ACK;
        const CFEND = sys::WIFI_PROMIS_CTRL_FILTER_MASK_CFEND;
        const CFENDACK = sys::WIFI_PROMIS_CTRL_FILTER_MASK_CFENDACK;
        const ALL = sys::WIFI_PROMIS_CTRL_FILTER_MASK_ALL;
    }
}

/// User callback for promiscuous mode, stored as an atomic function pointer.
///
/// We store `fn(RxPacketInfo, &[u8])` as a pair of pointer-sized values.
/// Since this is a thin function pointer, it fits in a single `AtomicPtr`.
static PROMISC_HANDLER: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

/// Load the current promiscuous handler, if any.
fn load_promisc_handler() -> Option<fn(RxPacketInfo, &[u8])> {
    let ptr = PROMISC_HANDLER.load(Ordering::Acquire);
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { core::mem::transmute::<*mut (), fn(RxPacketInfo, &[u8])>(ptr) })
    }
}

/// Enable promiscuous mode with a packet receive callback.
///
/// Only one callback can be active. The callback runs in the WiFi task
/// context — keep it fast. The WiFi driver must be started.
pub fn enable(
    handler: fn(RxPacketInfo, &[u8]),
    filter: PromiscuousFilter,
) -> Result<(), WifiError> {
    PROMISC_HANDLER.store(handler as *mut (), Ordering::Release);

    let raw_filter = sys::wifi_promiscuous_filter_t {
        filter_mask: filter.bits(),
    };

    EspError::check(unsafe { sys::esp_wifi_set_promiscuous_filter(&raw_filter) })?;
    EspError::check(unsafe { sys::esp_wifi_set_promiscuous_rx_cb(Some(promisc_trampoline)) })?;
    super::compat::port::set_wdev_tick_poll_enabled(true);
    if let Err(error) = EspError::check(unsafe { sys::esp_wifi_set_promiscuous(true) }) {
        super::compat::port::set_wdev_tick_poll_enabled(false);
        return Err(error.into());
    }

    Ok(())
}

/// Disable promiscuous mode.
pub fn disable() -> Result<(), WifiError> {
    let result =
        EspError::check(unsafe { sys::esp_wifi_set_promiscuous(false) }).map_err(WifiError::from);
    super::compat::port::set_wdev_tick_poll_enabled(false);
    PROMISC_HANDLER.store(core::ptr::null_mut(), Ordering::Release);
    result
}

/// Set the control-frame subtype filter.
pub fn set_control_filter(filter: ControlFilter) -> Result<(), WifiError> {
    let raw = sys::wifi_promiscuous_filter_t {
        filter_mask: filter.bits(),
    };
    EspError::check(unsafe { sys::esp_wifi_set_promiscuous_ctrl_filter(&raw) })?;
    Ok(())
}

/// Check whether promiscuous mode is currently enabled.
pub fn is_enabled() -> Result<bool, WifiError> {
    let mut en = false;
    EspError::check(unsafe { sys::esp_wifi_get_promiscuous(&mut en) })?;
    Ok(en)
}

unsafe extern "C" fn promisc_trampoline(
    buf: *mut core::ffi::c_void,
    pkt_type: sys::wifi_promiscuous_pkt_type_t,
) {
    let handler = match load_promisc_handler() {
        Some(h) => h,
        None => return,
    };

    if buf.is_null() {
        return;
    }

    let pkt = unsafe { &*(buf as *const sys::wifi_promiscuous_pkt_t) };

    let packet_type = PacketType::from_raw(pkt_type);
    let info = RxPacketInfo {
        rssi: pkt.rx_ctrl.rssi(),
        channel: pkt.rx_ctrl.channel(),
        packet_type,
    };

    // Determine payload length from the rx_ctrl metadata.
    // For 11n (sig_mode != 0): use HT length, otherwise: use legacy length.
    let payload_len = if packet_type == PacketType::Control {
        0
    } else if pkt.rx_ctrl.sig_mode() != 0 {
        pkt.rx_ctrl.ht_length() as usize
    } else {
        pkt.rx_ctrl.legacy_length() as usize
    };

    // Limit to a sane maximum to avoid reading garbage.
    let payload_len = payload_len.min(2500);

    let payload = if payload_len == 0 {
        &[]
    } else {
        let payload_ptr = pkt.payload.as_ptr();
        unsafe { core::slice::from_raw_parts(payload_ptr, payload_len) }
    };
    handler(info, payload);
}
