//! WiFi and IP event definitions and dispatch.
//!
//! The ESP8266 RTOS SDK posts WiFi and IP events through its event loop.
//! This module provides:
//!
//! 1. A Rust enum representation ([`WifiEvent`])
//! 2. A synchronous callback mechanism ([`set_event_handler`])
//! 3. An async PubSub event bus ([`EVENT_BUS`]) — create subscribers with
//!    [`event_subscriber`] and await events via `next_message_pure()`

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::pubsub::PubSubChannel;
use esp8266_wifi_sys as sys;
use portable_atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};

/// A WiFi or IP event received from the SDK event loop.
#[derive(Clone)]
pub enum WifiEvent {
    /// The WiFi subsystem is ready.
    Ready,
    /// A scan has completed.
    ScanDone {
        /// Number of APs found.
        count: u8,
        /// `true` if the scan succeeded.
        success: bool,
    },
    /// Station interface started.
    StaStart,
    /// Station interface stopped.
    StaStop,
    /// Station connected to an AP.
    StaConnected {
        ssid: heapless::Vec<u8, 32>,
        bssid: [u8; 6],
        channel: u8,
        auth_mode: crate::wifi::AuthMode,
    },
    /// Station disconnected from AP.
    StaDisconnected {
        ssid: heapless::Vec<u8, 32>,
        bssid: [u8; 6],
        reason: DisconnectReason,
    },
    /// The authentication mode of the connected AP changed.
    StaAuthModeChanged {
        old: crate::wifi::AuthMode,
        new: crate::wifi::AuthMode,
    },
    /// The RSSI of the connected AP dropped below the configured threshold.
    StaBssRssiLow { rssi: i32 },
    /// WPS enrollment succeeded.
    StaWpsSuccess,
    /// WPS enrollment failed.
    StaWpsFailed,
    /// WPS enrollment timed out.
    StaWpsTimeout,
    /// WPS PIN code received.
    StaWpsPin { pin: [u8; 8] },
    /// Soft-AP interface started.
    ApStart,
    /// Soft-AP interface stopped.
    ApStop,
    /// A station connected to the soft-AP.
    ApStaConnected { mac: [u8; 6], aid: u8 },
    /// A station disconnected from the soft-AP.
    ApStaDisconnected { mac: [u8; 6], aid: u8 },
    /// Probe request received on the soft-AP.
    ApProbeRequestReceived { mac: [u8; 6], rssi: i32 },
}

/// Compact event tag for lightweight signaling (no data payload).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiEventKind {
    Ready,
    ScanDone,
    StaStart,
    StaStop,
    StaConnected,
    StaDisconnected,
    StaAuthModeChanged,
    StaBssRssiLow,
    StaWpsSuccess,
    StaWpsFailed,
    StaWpsTimeout,
    StaWpsPin,
    ApStart,
    ApStop,
    ApStaConnected,
    ApStaDisconnected,
    ApProbeRequestReceived,
}

// Manual Debug — prints only the event kind to avoid the large derived match
// (with heapless::Vec, array, and enum fields) that triggers an Xtensa LLVM
// register-spill codegen bug when compiled with LTO.
impl core::fmt::Debug for WifiEvent {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.kind().as_str())
    }
}

impl WifiEvent {
    /// Get the event kind (tag without data).
    pub fn kind(&self) -> WifiEventKind {
        match self {
            WifiEvent::Ready => WifiEventKind::Ready,
            WifiEvent::ScanDone { .. } => WifiEventKind::ScanDone,
            WifiEvent::StaStart => WifiEventKind::StaStart,
            WifiEvent::StaStop => WifiEventKind::StaStop,
            WifiEvent::StaConnected { .. } => WifiEventKind::StaConnected,
            WifiEvent::StaDisconnected { .. } => WifiEventKind::StaDisconnected,
            WifiEvent::StaAuthModeChanged { .. } => WifiEventKind::StaAuthModeChanged,
            WifiEvent::StaBssRssiLow { .. } => WifiEventKind::StaBssRssiLow,
            WifiEvent::StaWpsSuccess => WifiEventKind::StaWpsSuccess,
            WifiEvent::StaWpsFailed => WifiEventKind::StaWpsFailed,
            WifiEvent::StaWpsTimeout => WifiEventKind::StaWpsTimeout,
            WifiEvent::StaWpsPin { .. } => WifiEventKind::StaWpsPin,
            WifiEvent::ApStart => WifiEventKind::ApStart,
            WifiEvent::ApStop => WifiEventKind::ApStop,
            WifiEvent::ApStaConnected { .. } => WifiEventKind::ApStaConnected,
            WifiEvent::ApStaDisconnected { .. } => WifiEventKind::ApStaDisconnected,
            WifiEvent::ApProbeRequestReceived { .. } => WifiEventKind::ApProbeRequestReceived,
        }
    }
}

impl WifiEventKind {
    /// Return the variant name as a static string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::ScanDone => "ScanDone",
            Self::StaStart => "StaStart",
            Self::StaStop => "StaStop",
            Self::StaConnected => "StaConnected",
            Self::StaDisconnected => "StaDisconnected",
            Self::StaAuthModeChanged => "StaAuthModeChanged",
            Self::StaBssRssiLow => "StaBssRssiLow",
            Self::StaWpsSuccess => "StaWpsSuccess",
            Self::StaWpsFailed => "StaWpsFailed",
            Self::StaWpsTimeout => "StaWpsTimeout",
            Self::StaWpsPin => "StaWpsPin",
            Self::ApStart => "ApStart",
            Self::ApStop => "ApStop",
            Self::ApStaConnected => "ApStaConnected",
            Self::ApStaDisconnected => "ApStaDisconnected",
            Self::ApProbeRequestReceived => "ApProbeRequestReceived",
        }
    }
}

/// Reason codes for station disconnection.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DisconnectReason {
    Unspecified,
    AuthExpire,
    AuthLeave,
    AssocExpire,
    AssocTooMany,
    NotAuthed,
    NotAssoced,
    AssocLeave,
    AssocNotAuthed,
    BeaconTimeout,
    NoApFound,
    AuthFail,
    AssocFail,
    HandshakeTimeout,
    ConnectionFail,
    ApTsfReset,
    BasicRateNotSupport,
    /// An unknown reason code from the SDK.
    Other(u8),
}

// Manual Debug — just prints the raw code to avoid the large match that
// triggers an Xtensa LLVM register-spill codegen bug when compiled with LTO.
impl core::fmt::Debug for DisconnectReason {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "DisconnectReason({})", self.to_raw())
    }
}

impl DisconnectReason {
    /// Convert back to the raw SDK reason code.
    pub fn to_raw(&self) -> u8 {
        match self {
            Self::Unspecified => sys::WIFI_REASON_UNSPECIFIED as u8,
            Self::AuthExpire => sys::WIFI_REASON_AUTH_EXPIRE as u8,
            Self::AuthLeave => sys::WIFI_REASON_AUTH_LEAVE as u8,
            Self::AssocExpire => sys::WIFI_REASON_ASSOC_EXPIRE as u8,
            Self::AssocTooMany => sys::WIFI_REASON_ASSOC_TOOMANY as u8,
            Self::NotAuthed => sys::WIFI_REASON_NOT_AUTHED as u8,
            Self::NotAssoced => sys::WIFI_REASON_NOT_ASSOCED as u8,
            Self::AssocLeave => sys::WIFI_REASON_ASSOC_LEAVE as u8,
            Self::AssocNotAuthed => sys::WIFI_REASON_ASSOC_NOT_AUTHED as u8,
            Self::BeaconTimeout => sys::WIFI_REASON_BEACON_TIMEOUT as u8,
            Self::NoApFound => sys::WIFI_REASON_NO_AP_FOUND as u8,
            Self::AuthFail => sys::WIFI_REASON_AUTH_FAIL as u8,
            Self::AssocFail => sys::WIFI_REASON_ASSOC_FAIL as u8,
            Self::HandshakeTimeout => sys::WIFI_REASON_HANDSHAKE_TIMEOUT as u8,
            Self::ConnectionFail => sys::WIFI_REASON_CONNECTION_FAIL as u8,
            Self::ApTsfReset => sys::WIFI_REASON_AP_TSF_RESET as u8,
            Self::BasicRateNotSupport => sys::WIFI_REASON_BASIC_RATE_NOT_SUPPORT as u8,
            Self::Other(code) => *code,
        }
    }

    pub(crate) fn from_raw(code: u8) -> Self {
        match code as u32 {
            sys::WIFI_REASON_UNSPECIFIED => Self::Unspecified,
            sys::WIFI_REASON_AUTH_EXPIRE => Self::AuthExpire,
            sys::WIFI_REASON_AUTH_LEAVE => Self::AuthLeave,
            sys::WIFI_REASON_ASSOC_EXPIRE => Self::AssocExpire,
            sys::WIFI_REASON_ASSOC_TOOMANY => Self::AssocTooMany,
            sys::WIFI_REASON_NOT_AUTHED => Self::NotAuthed,
            sys::WIFI_REASON_NOT_ASSOCED => Self::NotAssoced,
            sys::WIFI_REASON_ASSOC_LEAVE => Self::AssocLeave,
            sys::WIFI_REASON_ASSOC_NOT_AUTHED => Self::AssocNotAuthed,
            sys::WIFI_REASON_BEACON_TIMEOUT => Self::BeaconTimeout,
            sys::WIFI_REASON_NO_AP_FOUND => Self::NoApFound,
            sys::WIFI_REASON_AUTH_FAIL => Self::AuthFail,
            sys::WIFI_REASON_ASSOC_FAIL => Self::AssocFail,
            sys::WIFI_REASON_HANDSHAKE_TIMEOUT => Self::HandshakeTimeout,
            sys::WIFI_REASON_CONNECTION_FAIL => Self::ConnectionFail,
            sys::WIFI_REASON_AP_TSF_RESET => Self::ApTsfReset,
            sys::WIFI_REASON_BASIC_RATE_NOT_SUPPORT => Self::BasicRateNotSupport,
            _ => Self::Other(code),
        }
    }
}

// ---------------------------------------------------------------------------
// Async event infrastructure
// ---------------------------------------------------------------------------

/// Global event bus — multiple subscribers, buffered.
///
/// - 8 events queue depth per subscriber
/// - Up to 4 concurrent subscribers
/// - 1 publisher (the C trampoline)
///
/// Each subscriber gets its own independent queue, so slow consumers don't
/// block fast ones. If a subscriber's queue is full, the oldest event in that
/// subscriber's queue is dropped.
pub static EVENT_BUS: PubSubChannel<CriticalSectionRawMutex, WifiEvent, 8, 4, 1> =
    PubSubChannel::new();

type RawEventSubscriber =
    embassy_sync::pubsub::Subscriber<'static, CriticalSectionRawMutex, WifiEvent, 8, 4, 1>;

/// Convenience wrapper for an event subscriber.
pub struct EventSubscriber {
    inner: RawEventSubscriber,
}

impl EventSubscriber {
    /// Wait for the next event, without tracking lag.
    pub async fn next_message_pure(&mut self) -> WifiEvent {
        self.inner.next_message_pure().await
    }

    /// Try to receive the next event without blocking.
    pub fn try_next_message_pure(&mut self) -> Option<WifiEvent> {
        self.inner.try_next_message_pure()
    }
}

impl Drop for EventSubscriber {
    fn drop(&mut self) {
        EVENT_SUBSCRIBER_COUNT.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Create a new event subscriber.
///
/// Each subscriber has its own 8-event buffer. Up to 4 subscribers can be
/// active simultaneously. Returns `None` if the subscriber limit is reached.
pub fn event_subscriber() -> Option<EventSubscriber> {
    let inner = EVENT_BUS.subscriber().ok()?;
    EVENT_SUBSCRIBER_COUNT.fetch_add(1, Ordering::AcqRel);
    Some(EventSubscriber { inner })
}

/// Wait until a specific event kind occurs on the given subscriber.
pub async fn wait_for_event(sub: &mut EventSubscriber, kind: WifiEventKind) -> WifiEvent {
    loop {
        let event = sub.next_message_pure().await;
        if event.kind() == kind {
            return event;
        }
    }
}

// ---------------------------------------------------------------------------
// Synchronous callback infrastructure (retained for compatibility)
// ---------------------------------------------------------------------------

/// User-provided event callback, stored as an atomic function pointer.
static EVENT_HANDLER: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

/// Whether the async event system trampoline has been registered.
static TRAMPOLINE_REGISTERED: AtomicBool = AtomicBool::new(false);
static EVENT_SUBSCRIBER_COUNT: AtomicU32 = AtomicU32::new(0);
static SDK_STA_AUTO_CONNECT_ENABLED: AtomicBool = AtomicBool::new(false);
static SDK_STA_AUTO_CONNECT_MAX_RETRIES: AtomicU32 = AtomicU32::new(0);
static SDK_STA_AUTO_CONNECT_RETRY_COUNT: AtomicU32 = AtomicU32::new(0);
static SDK_STA_AUTO_CONNECT_FAIL_SEQ: AtomicU32 = AtomicU32::new(0);
static SDK_STA_AUTO_CONNECT_EVENT_GROUP: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

const SDK_STA_CONNECTED_BIT: u32 = 1 << 0;
const SDK_STA_FAIL_BIT: u32 = 1 << 1;

fn load_event_handler() -> Option<fn(WifiEvent)> {
    let ptr = EVENT_HANDLER.load(Ordering::Acquire);
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { core::mem::transmute::<*mut (), fn(WifiEvent)>(ptr) })
    }
}

/// Register a global WiFi/IP event handler (synchronous callback).
///
/// Only one handler can be active at a time. Calling this again replaces the
/// previous handler. The callback runs in the SDK event task context — keep it
/// fast and non-blocking.
pub fn set_event_handler(handler: fn(WifiEvent)) -> Result<(), crate::wifi::WifiError> {
    EVENT_HANDLER.store(handler as *mut (), Ordering::Release);
    ensure_trampoline_registered()
}

/// Enable SDK-style station auto-connect handling in the event task.
///
/// This matches the ESP8266 RTOS SDK station example shape:
/// `WIFI_EVENT_STA_START` issues the first `esp_wifi_connect()`, and
/// `WIFI_EVENT_STA_DISCONNECTED` retries it up to `max_retries` times before
/// latching a failure sequence.
pub fn enable_sdk_sta_auto_connect(max_retries: u32) -> Result<(), crate::wifi::WifiError> {
    ensure_sdk_sta_auto_connect_event_group()?;
    SDK_STA_AUTO_CONNECT_RETRY_COUNT.store(0, Ordering::Release);
    SDK_STA_AUTO_CONNECT_FAIL_SEQ.store(0, Ordering::Release);
    SDK_STA_AUTO_CONNECT_MAX_RETRIES.store(max_retries, Ordering::Release);
    SDK_STA_AUTO_CONNECT_ENABLED.store(true, Ordering::Release);
    ensure_trampoline_registered()
}

fn ensure_sdk_sta_auto_connect_event_group(
) -> Result<*mut core::ffi::c_void, crate::wifi::WifiError> {
    let current = SDK_STA_AUTO_CONNECT_EVENT_GROUP.load(Ordering::Acquire);
    if !current.is_null() {
        return Ok(current.cast());
    }

    let created = unsafe { crate::wifi::compat::event_group::xEventGroupCreate() };
    if created.is_null() {
        return Err(crate::wifi::WifiError::Internal(crate::wifi::EspError(
            sys::ESP_ERR_NO_MEM,
        )));
    }

    match SDK_STA_AUTO_CONNECT_EVENT_GROUP.compare_exchange(
        core::ptr::null_mut(),
        created.cast(),
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => Ok(created),
        Err(existing) => {
            unsafe { crate::wifi::compat::event_group::vEventGroupDelete(created) };
            Ok(existing.cast())
        }
    }
}

pub(crate) fn wait_for_sdk_sta_auto_connect_bits() -> Result<u32, crate::wifi::WifiError> {
    let group = ensure_sdk_sta_auto_connect_event_group()?;
    Ok(unsafe {
        crate::wifi::compat::event_group::xEventGroupWaitBits(
            group,
            SDK_STA_CONNECTED_BIT | SDK_STA_FAIL_BIT,
            0,
            0,
            0xFFFF_FFFF,
        )
    })
}

pub fn sdk_sta_auto_connect_fail_seq() -> u32 {
    SDK_STA_AUTO_CONNECT_FAIL_SEQ.load(Ordering::Acquire)
}

/// Ensure the C trampoline is registered with the SDK event loop.
///
/// Called automatically by both `set_event_handler` and the first async event
/// API call. Safe to call multiple times.
pub(crate) fn ensure_trampoline_registered() -> Result<(), crate::wifi::WifiError> {
    if TRAMPOLINE_REGISTERED.load(Ordering::Acquire) {
        return Ok(());
    }

    let ret = unsafe {
        sys::esp_event_handler_register(
            sys::WIFI_EVENT,
            -1, // ESP_EVENT_ANY_ID
            Some(wifi_event_trampoline),
            core::ptr::null_mut(),
        )
    };
    crate::wifi::EspError::check(ret)?;

    // Only mark as registered after successful registration, so transient
    // failures don't permanently latch the flag and prevent retries.
    TRAMPOLINE_REGISTERED.store(true, Ordering::Release);
    Ok(())
}

/// C-compatible trampoline for WiFi events.
///
/// Dispatches to both the synchronous callback (if set) and the async
/// event channel/signal.
///
/// # Safety
///
/// Called by the SDK event loop; `event_data` must be valid for the given
/// `event_id`.
unsafe extern "C" fn wifi_event_trampoline(
    _arg: *mut core::ffi::c_void,
    _base: sys::esp_event_base_t,
    event_id: i32,
    event_data: *mut core::ffi::c_void,
) {
    // Fast path: update atomic state from raw event_id (near-zero stack).
    update_sta_state_raw(event_id, event_data);

    // Slow path: build the full WifiEvent for the async bus and optional sync handler.
    dispatch_event(event_id, event_data);
}

/// Update STA/AP atomic state directly from raw event_id. Minimal stack.
#[inline(never)]
fn update_sta_state_raw(event_id: i32, event_data: *mut core::ffi::c_void) {
    use crate::wifi::controller::{note_sta_disconnect, StaConnectionState, AP_STATE, STA_STATE};
    match event_id as u32 {
        sys::WIFI_EVENT_STA_START => {
            STA_STATE.store(StaConnectionState::Connecting as u8, Ordering::Release);
            if SDK_STA_AUTO_CONNECT_ENABLED.load(Ordering::Acquire) {
                if let Ok(group) = ensure_sdk_sta_auto_connect_event_group() {
                    unsafe {
                        crate::wifi::compat::event_group::xEventGroupClearBits(
                            group,
                            SDK_STA_CONNECTED_BIT | SDK_STA_FAIL_BIT,
                        );
                    }
                }
                SDK_STA_AUTO_CONNECT_RETRY_COUNT.store(0, Ordering::Release);
                let _ = unsafe { sys::esp_wifi_connect() };
            }
        }
        sys::WIFI_EVENT_STA_CONNECTED => {
            STA_STATE.store(StaConnectionState::Connected as u8, Ordering::Release);
            if SDK_STA_AUTO_CONNECT_ENABLED.load(Ordering::Acquire) {
                SDK_STA_AUTO_CONNECT_RETRY_COUNT.store(0, Ordering::Release);
                if let Ok(group) = ensure_sdk_sta_auto_connect_event_group() {
                    unsafe {
                        crate::wifi::compat::event_group::xEventGroupSetBits(
                            group,
                            SDK_STA_CONNECTED_BIT,
                        );
                    }
                }
            }
        }
        sys::WIFI_EVENT_STA_DISCONNECTED => {
            if !event_data.is_null() {
                let reason = unsafe { *(event_data as *const u8).add(32 + 1 + 6) };
                note_sta_disconnect(DisconnectReason::from_raw(reason));
                crate::println!("DISCONNECT reason={}", reason);
            } else {
                note_sta_disconnect(DisconnectReason::Unspecified);
            }
            if SDK_STA_AUTO_CONNECT_ENABLED.load(Ordering::Acquire) {
                let max_retries = SDK_STA_AUTO_CONNECT_MAX_RETRIES.load(Ordering::Acquire);
                // Match the RTOS SDK station example's ordering:
                // compare the current retry count first, then increment only
                // after issuing that retry attempt.
                let retry_count = SDK_STA_AUTO_CONNECT_RETRY_COUNT.load(Ordering::Acquire);
                if retry_count < max_retries {
                    let _ = unsafe { sys::esp_wifi_connect() };
                    SDK_STA_AUTO_CONNECT_RETRY_COUNT
                        .store(retry_count.saturating_add(1), Ordering::Release);
                } else {
                    SDK_STA_AUTO_CONNECT_FAIL_SEQ.fetch_add(1, Ordering::AcqRel);
                    if let Ok(group) = ensure_sdk_sta_auto_connect_event_group() {
                        unsafe {
                            crate::wifi::compat::event_group::xEventGroupSetBits(
                                group,
                                SDK_STA_FAIL_BIT,
                            );
                        }
                    }
                }
            }
        }
        sys::WIFI_EVENT_STA_STOP => {
            STA_STATE.store(StaConnectionState::Disconnected as u8, Ordering::Release);
        }
        sys::WIFI_EVENT_AP_START => {
            AP_STATE.store(1, Ordering::Release);
        }
        sys::WIFI_EVENT_AP_STOP => {
            AP_STATE.store(0, Ordering::Release);
        }
        _ => {}
    }
}

/// Build full WifiEvent and dispatch to registered handler.
#[inline(never)]
fn dispatch_event(event_id: i32, event_data: *mut core::ffi::c_void) {
    let handler = load_event_handler();
    let has_async_subscribers = EVENT_SUBSCRIBER_COUNT.load(Ordering::Acquire) != 0;
    if handler.is_none() && !has_async_subscribers {
        return;
    }

    if let Some(event) = unsafe { parse_event(event_id, event_data) } {
        if has_async_subscribers {
            if handler.is_some() {
                EVENT_BUS
                    .immediate_publisher()
                    .publish_immediate(event.clone());
            } else {
                EVENT_BUS.immediate_publisher().publish_immediate(event);
                return;
            }
        }

        if let Some(handler) = handler {
            handler(event);
        }
    }
}

/// Parse a raw SDK event into a `WifiEvent`.
///
/// # Safety
///
/// `event_data` must be valid for the given `event_id`.
unsafe fn parse_event(event_id: i32, event_data: *mut core::ffi::c_void) -> Option<WifiEvent> {
    let event = match event_id as u32 {
        sys::WIFI_EVENT_WIFI_READY => WifiEvent::Ready,
        sys::WIFI_EVENT_SCAN_DONE => {
            let d = unsafe { &*(event_data as *const sys::wifi_event_sta_scan_done_t) };
            WifiEvent::ScanDone {
                count: d.number,
                success: d.status == 0,
            }
        }
        sys::WIFI_EVENT_STA_START => WifiEvent::StaStart,
        sys::WIFI_EVENT_STA_STOP => WifiEvent::StaStop,
        sys::WIFI_EVENT_STA_CONNECTED => {
            let d = unsafe { &*(event_data as *const sys::wifi_event_sta_connected_t) };
            let len = (d.ssid_len as usize).min(32);
            let mut ssid = heapless::Vec::new();
            let _ = ssid.extend_from_slice(&d.ssid[..len]);
            WifiEvent::StaConnected {
                ssid,
                bssid: d.bssid,
                channel: d.channel,
                auth_mode: crate::wifi::AuthMode::from_raw(d.authmode),
            }
        }
        sys::WIFI_EVENT_STA_DISCONNECTED => {
            let d = unsafe { &*(event_data as *const sys::wifi_event_sta_disconnected_t) };
            let len = (d.ssid_len as usize).min(32);
            let mut ssid = heapless::Vec::new();
            let _ = ssid.extend_from_slice(&d.ssid[..len]);
            WifiEvent::StaDisconnected {
                ssid,
                bssid: d.bssid,
                reason: DisconnectReason::from_raw(d.reason),
            }
        }
        sys::WIFI_EVENT_STA_AUTHMODE_CHANGE => {
            let d = unsafe { &*(event_data as *const sys::wifi_event_sta_authmode_change_t) };
            WifiEvent::StaAuthModeChanged {
                old: crate::wifi::AuthMode::from_raw(d.old_mode),
                new: crate::wifi::AuthMode::from_raw(d.new_mode),
            }
        }
        sys::WIFI_EVENT_STA_BSS_RSSI_LOW => {
            let d = unsafe { &*(event_data as *const sys::wifi_event_bss_rssi_low_t) };
            WifiEvent::StaBssRssiLow { rssi: d.rssi }
        }
        sys::WIFI_EVENT_STA_WPS_ER_SUCCESS => WifiEvent::StaWpsSuccess,
        sys::WIFI_EVENT_STA_WPS_ER_FAILED => WifiEvent::StaWpsFailed,
        sys::WIFI_EVENT_STA_WPS_ER_TIMEOUT => WifiEvent::StaWpsTimeout,
        sys::WIFI_EVENT_STA_WPS_ER_PIN => {
            let d = unsafe { &*(event_data as *const sys::wifi_event_sta_wps_er_pin_t) };
            WifiEvent::StaWpsPin { pin: d.pin_code }
        }
        sys::WIFI_EVENT_AP_START => WifiEvent::ApStart,
        sys::WIFI_EVENT_AP_STOP => WifiEvent::ApStop,
        sys::WIFI_EVENT_AP_STACONNECTED => {
            let d = unsafe { &*(event_data as *const sys::wifi_event_ap_staconnected_t) };
            WifiEvent::ApStaConnected {
                mac: d.mac,
                aid: d.aid,
            }
        }
        sys::WIFI_EVENT_AP_STADISCONNECTED => {
            let d = unsafe { &*(event_data as *const sys::wifi_event_ap_stadisconnected_t) };
            WifiEvent::ApStaDisconnected {
                mac: d.mac,
                aid: d.aid,
            }
        }
        sys::WIFI_EVENT_AP_PROBEREQRECVED => {
            let d = unsafe { &*(event_data as *const sys::wifi_event_ap_probe_req_rx_t) };
            WifiEvent::ApProbeRequestReceived {
                mac: d.mac,
                rssi: d.rssi,
            }
        }
        _ => return None,
    };

    Some(event)
}
