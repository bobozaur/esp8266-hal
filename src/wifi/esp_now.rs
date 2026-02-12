//! ESP-NOW peer-to-peer communication.
//!
//! ESP-NOW allows direct frame exchange between ESP8266 devices without
//! an access point. Requires the `esp-now` feature flag.

use esp8266_wifi_sys as sys;
use portable_atomic::{AtomicPtr, Ordering};

use crate::wifi::controller::WifiInterface;
use crate::wifi::error::{EspError, WifiError};

/// Maximum data payload for a single ESP-NOW frame.
pub const MAX_DATA_LEN: usize = sys::ESP_NOW_MAX_DATA_LEN as usize;

/// ESP-NOW send result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendStatus {
    Success,
    Fail,
}

/// ESP-NOW peer information.
#[derive(Debug, Clone)]
pub struct PeerInfo {
    /// MAC address of the peer.
    pub peer_addr: [u8; 6],
    /// Local Master Key for encryption (16 bytes). All zeros = no encryption.
    pub lmk: [u8; 16],
    /// Channel (0 = use current channel).
    pub channel: u8,
    /// Interface to use for communication.
    pub interface: WifiInterface,
    /// Whether to encrypt communication with this peer.
    pub encrypt: bool,
}

impl PeerInfo {
    fn to_raw(&self) -> sys::esp_now_peer_info_t {
        sys::esp_now_peer_info_t {
            peer_addr: self.peer_addr,
            lmk: self.lmk,
            channel: self.channel,
            ifidx: self.interface.to_raw(),
            encrypt: self.encrypt,
            priv_: core::ptr::null_mut(),
        }
    }

    fn from_raw(raw: &sys::esp_now_peer_info_t) -> Self {
        Self {
            peer_addr: raw.peer_addr,
            lmk: raw.lmk,
            channel: raw.channel,
            interface: match raw.ifidx {
                sys::WIFI_IF_AP => WifiInterface::Ap,
                _ => WifiInterface::Sta,
            },
            encrypt: raw.encrypt,
        }
    }
}

/// Peer count information.
#[derive(Debug, Clone, Copy)]
pub struct PeerCount {
    pub total: usize,
    pub encrypted: usize,
}

// Callback storage using atomic pointers (no static mut).
static RECV_HANDLER: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());
static SEND_HANDLER: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

fn load_recv_handler() -> Option<fn(&[u8; 6], &[u8])> {
    let ptr = RECV_HANDLER.load(Ordering::Acquire);
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { core::mem::transmute::<*mut (), fn(&[u8; 6], &[u8])>(ptr) })
    }
}

fn load_send_handler() -> Option<fn(&[u8; 6], SendStatus)> {
    let ptr = SEND_HANDLER.load(Ordering::Acquire);
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { core::mem::transmute::<*mut (), fn(&[u8; 6], SendStatus)>(ptr) })
    }
}

/// Initialize ESP-NOW.
///
/// WiFi must be initialized and started before calling this.
pub fn init() -> Result<(), WifiError> {
    EspError::check(unsafe { sys::esp_now_init() })?;
    Ok(())
}

/// De-initialize ESP-NOW.
pub fn deinit() -> Result<(), WifiError> {
    EspError::check(unsafe { sys::esp_now_deinit() })?;
    RECV_HANDLER.store(core::ptr::null_mut(), Ordering::Release);
    SEND_HANDLER.store(core::ptr::null_mut(), Ordering::Release);
    Ok(())
}

/// Get the ESP-NOW protocol version.
pub fn get_version() -> Result<u32, WifiError> {
    let mut version: u32 = 0;
    EspError::check(unsafe { sys::esp_now_get_version(&mut version) })?;
    Ok(version)
}

/// Register a callback for received ESP-NOW data.
///
/// The callback receives the sender's MAC address and the data payload.
pub fn register_recv_callback(handler: fn(&[u8; 6], &[u8])) -> Result<(), WifiError> {
    RECV_HANDLER.store(handler as *mut (), Ordering::Release);
    EspError::check(unsafe { sys::esp_now_register_recv_cb(Some(recv_trampoline)) })?;
    Ok(())
}

/// Unregister the receive callback.
pub fn unregister_recv_callback() -> Result<(), WifiError> {
    EspError::check(unsafe { sys::esp_now_unregister_recv_cb() })?;
    RECV_HANDLER.store(core::ptr::null_mut(), Ordering::Release);
    Ok(())
}

/// Register a callback for send completion status.
pub fn register_send_callback(handler: fn(&[u8; 6], SendStatus)) -> Result<(), WifiError> {
    SEND_HANDLER.store(handler as *mut (), Ordering::Release);
    EspError::check(unsafe { sys::esp_now_register_send_cb(Some(send_trampoline)) })?;
    Ok(())
}

/// Unregister the send callback.
pub fn unregister_send_callback() -> Result<(), WifiError> {
    EspError::check(unsafe { sys::esp_now_unregister_send_cb() })?;
    SEND_HANDLER.store(core::ptr::null_mut(), Ordering::Release);
    Ok(())
}

/// Send data to a peer.
///
/// If `peer_addr` is the broadcast address (`FF:FF:FF:FF:FF:FF`), the frame
/// is sent to all registered peers.
pub fn send(peer_addr: &[u8; 6], data: &[u8]) -> Result<(), WifiError> {
    if data.len() > MAX_DATA_LEN {
        return Err(WifiError::InvalidArgument);
    }
    EspError::check(unsafe { sys::esp_now_send(peer_addr.as_ptr(), data.as_ptr(), data.len()) })?;
    Ok(())
}

/// Add a peer.
pub fn add_peer(peer: &PeerInfo) -> Result<(), WifiError> {
    let raw = peer.to_raw();
    EspError::check(unsafe { sys::esp_now_add_peer(&raw) })?;
    Ok(())
}

/// Remove a peer by MAC address.
pub fn del_peer(addr: &[u8; 6]) -> Result<(), WifiError> {
    EspError::check(unsafe { sys::esp_now_del_peer(addr.as_ptr()) })?;
    Ok(())
}

/// Modify an existing peer's configuration.
pub fn mod_peer(peer: &PeerInfo) -> Result<(), WifiError> {
    let raw = peer.to_raw();
    EspError::check(unsafe { sys::esp_now_mod_peer(&raw) })?;
    Ok(())
}

/// Get information about a peer by MAC address.
pub fn get_peer(addr: &[u8; 6]) -> Result<PeerInfo, WifiError> {
    let mut raw: sys::esp_now_peer_info_t = unsafe { core::mem::zeroed() };
    EspError::check(unsafe { sys::esp_now_get_peer(addr.as_ptr(), &mut raw) })?;
    Ok(PeerInfo::from_raw(&raw))
}

/// Check whether a peer exists.
pub fn peer_exists(addr: &[u8; 6]) -> bool {
    unsafe { sys::esp_now_is_peer_exist(addr.as_ptr()) }
}

/// Get peer count information.
pub fn get_peer_count() -> Result<PeerCount, WifiError> {
    let mut raw: sys::esp_now_peer_num_t = unsafe { core::mem::zeroed() };
    EspError::check(unsafe { sys::esp_now_get_peer_num(&mut raw) })?;
    Ok(PeerCount {
        total: raw.total_num as usize,
        encrypted: raw.encrypt_num as usize,
    })
}

/// Set the Primary Master Key (PMK) for ESP-NOW encryption.
///
/// The key must be exactly 16 bytes.
pub fn set_pmk(pmk: &[u8; 16]) -> Result<(), WifiError> {
    EspError::check(unsafe { sys::esp_now_set_pmk(pmk.as_ptr()) })?;
    Ok(())
}

unsafe extern "C" fn recv_trampoline(
    mac_addr: *const u8,
    data: *const u8,
    data_len: core::ffi::c_int,
) {
    let handler = match load_recv_handler() {
        Some(h) => h,
        None => return,
    };

    if mac_addr.is_null() || data.is_null() || data_len <= 0 {
        return;
    }

    let mac = unsafe { &*(mac_addr as *const [u8; 6]) };
    let payload = unsafe { core::slice::from_raw_parts(data, data_len as usize) };
    handler(mac, payload);
}

unsafe extern "C" fn send_trampoline(mac_addr: *const u8, status: sys::esp_now_send_status_t) {
    let handler = match load_send_handler() {
        Some(h) => h,
        None => return,
    };

    if mac_addr.is_null() {
        return;
    }

    let mac = unsafe { &*(mac_addr as *const [u8; 6]) };
    let s = match status {
        sys::ESP_NOW_SEND_SUCCESS => SendStatus::Success,
        _ => SendStatus::Fail,
    };
    handler(mac, s);
}
