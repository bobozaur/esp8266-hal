//! WiFi controller — lifecycle management and configuration.
//!
//! [`WifiController`] is the primary entry point. It manages the WiFi driver
//! lifecycle (init → set mode → set config → start → connect/…) and ensures
//! proper teardown on drop.

use core::ffi::c_void;

use esp8266_wifi_sys as sys;
use portable_atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};

use crate::wifi::config::{ApConfig, Configuration, StaConfig};
use crate::wifi::device::WifiDevice;
use crate::wifi::error::{EspError, WifiError};
use crate::wifi::protocol::Protocol;
use crate::wifi::scan::{AccessPointInfo, ScanConfig};
use crate::wifi::{Bandwidth, PowerSave, SecondaryChannel, Storage, WifiMode};

static INITIALIZED: AtomicBool = AtomicBool::new(false);
// ---------------------------------------------------------------------------
// Connection state tracking
// ---------------------------------------------------------------------------

/// Atomic connection state, updated from the event trampoline.
pub(crate) static STA_STATE: AtomicU8 = AtomicU8::new(StaConnectionState::Disconnected as u8);
pub(crate) static AP_STATE: AtomicU8 = AtomicU8::new(0); // 0 = stopped, 1 = started
pub(crate) static STA_DISCONNECT_SEQ: AtomicU32 = AtomicU32::new(0);
pub(crate) static STA_DISCONNECT_REASON: AtomicU8 =
    AtomicU8::new(sys::WIFI_REASON_UNSPECIFIED as u8);

/// Station connection state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum StaConnectionState {
    Disconnected = 0,
    Connecting = 1,
    Connected = 2,
}

impl StaConnectionState {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Connecting,
            2 => Self::Connected,
            _ => Self::Disconnected,
        }
    }
}

/// Query the current station connection state (lock-free).
///
/// This reads the atomic state directly, so it can be called without
/// holding a reference to `WifiController`.
pub fn current_sta_state() -> StaConnectionState {
    StaConnectionState::from_u8(STA_STATE.load(Ordering::Acquire))
}

/// Query the most recent station disconnect reason observed from the SDK event loop.
pub fn last_sta_disconnect_reason() -> crate::wifi::event::DisconnectReason {
    crate::wifi::event::DisconnectReason::from_raw(STA_DISCONNECT_REASON.load(Ordering::Acquire))
}

/// Query whether the soft-AP is currently started (lock-free).
pub fn is_ap_active() -> bool {
    AP_STATE.load(Ordering::Acquire) != 0
}

/// Update connection state from event trampoline. Called from `event.rs`.
pub(crate) fn update_state_from_event(event: &crate::wifi::WifiEvent) {
    use crate::wifi::WifiEvent;
    match event {
        WifiEvent::StaStart => {
            STA_STATE.store(StaConnectionState::Connecting as u8, Ordering::Release);
        }
        WifiEvent::StaConnected { .. } => {
            STA_STATE.store(StaConnectionState::Connected as u8, Ordering::Release);
        }
        WifiEvent::StaDisconnected { reason, .. } => {
            note_sta_disconnect(*reason);
        }
        WifiEvent::StaStop => {
            STA_STATE.store(StaConnectionState::Disconnected as u8, Ordering::Release);
        }
        WifiEvent::ApStart => {
            AP_STATE.store(1, Ordering::Release);
        }
        WifiEvent::ApStop => {
            AP_STATE.store(0, Ordering::Release);
        }
        _ => {}
    }
}

pub(crate) fn note_sta_disconnect(reason: crate::wifi::event::DisconnectReason) {
    STA_DISCONNECT_REASON.store(reason.to_raw(), Ordering::Release);
    STA_DISCONNECT_SEQ.fetch_add(1, Ordering::AcqRel);
    STA_STATE.store(StaConnectionState::Disconnected as u8, Ordering::Release);
}

/// RAII guard that ensures the compat scheduler lives as long as the controller.
///
/// The compat scheduler cannot actually be stopped (WiFi blobs expect it to
/// remain running), but wrapping it in a guard ties its lifetime to
/// [`WifiController`] and prevents accidental use before initialization.
struct CompatGuard;

impl CompatGuard {
    fn new() -> Self {
        unsafe { crate::wifi::compat::entry::init_compat_scheduler() };
        CompatGuard
    }
}

/// WiFi initialization parameters.
///
/// These map to the configurable fields in `wifi_init_config_t`. Use
/// `Default::default()` for the SDK defaults, then override specific fields.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct WifiInitConfig {
    /// Enable QoS (WMM) support.
    pub qos_enable: bool,
    /// Enable A-MPDU RX aggregation.
    pub ampdu_rx_enable: bool,
    /// Block-ack window size (0–16).
    pub rx_ba_win: u8,
    /// Number of A-MPDU RX buffers (0–16). Only used when `ampdu_rx_enable`
    /// is true. Set to 0 to disable.
    pub ampdu_rx_buf_num: u8,
    /// Length of each A-MPDU RX buffer in bytes. Typical value: 256.
    pub ampdu_rx_buf_len: u32,
    /// Enable A-MSDU RX.
    pub amsdu_rx_enable: bool,
    /// Number of RX buffers (14–28).
    pub rx_buf_num: u8,
    /// Number of RX packets (4–16).
    pub rx_pkt_num: u8,
    /// Minimum continuous RX buffers (0–16).
    pub left_continuous_rx_buf_num: u8,
    /// Number of TX packets (4–16).
    pub tx_buf_num: u8,
    /// Enable NVS flash for persistent WiFi config.
    pub nvs_enable: bool,
    /// Enable WPA3-SAE support.
    pub wpa3_sae_enable: bool,
}

impl Default for WifiInitConfig {
    fn default() -> Self {
        Self {
            qos_enable: false,
            ampdu_rx_enable: false,
            rx_ba_win: 0,
            ampdu_rx_buf_num: 5,
            ampdu_rx_buf_len: 256,
            amsdu_rx_enable: false,
            rx_buf_num: 16,
            rx_pkt_num: 7,
            left_continuous_rx_buf_num: 16,
            tx_buf_num: 6,
            // Keep WiFi blob-side NVS disabled on the live branch for now.
            // The local shim handles the PHY/MAC startup namespaces, but the
            // broader WiFi `nvs.net80211` model is not yet complete enough to
            // enable without regressing runtime on hardware.
            nvs_enable: false,
            wpa3_sae_enable: true,
        }
    }
}

extern "C" {
    /// Bridge from old `system_event_t` to the new `esp_event` loop.
    /// Compiled from `components/esp_event/event_send.c` — this is what
    /// `WIFI_INIT_CONFIG_DEFAULT()` sets as `event_handler`.
    fn esp_event_send(event: *mut sys::system_event_t) -> sys::esp_err_t;
}

// On the embassy-net path, `connect_blocking()` only waits for the WiFi link
// to come up. DHCP/config-up happens later when the network stack runner
// starts, matching `examples/tcp_server.rs`.
const CONNECT_WAIT_STA_CONNECTED_BIT: u32 = 1 << 0;

unsafe extern "C" fn connect_blocking_disconnect_handler(
    _arg: *mut c_void,
    _event_base: sys::esp_event_base_t,
    _event_id: i32,
    _event_data: *mut c_void,
) {
    let _ = unsafe { sys::esp_wifi_connect() };
}

unsafe extern "C" fn connect_blocking_connected_handler(
    event_group: *mut c_void,
    _event_base: sys::esp_event_base_t,
    _event_id: i32,
    _event_data: *mut c_void,
) {
    if !event_group.is_null() {
        unsafe {
            crate::wifi::compat::event_group::xEventGroupSetBits(
                event_group,
                CONNECT_WAIT_STA_CONNECTED_BIT,
            );
        }
    }
}

impl WifiInitConfig {
    fn to_raw(&self) -> sys::wifi_init_config_t {
        // The SDK default keeps this non-zero even when AMPDU RX is disabled,
        // and wDev_Initialize uses it in descriptor-region sizing math.
        let ampdu_rx_buf_num: u8 = self.ampdu_rx_buf_num;
        let ampdu_rx_buf_len: u32 = self.ampdu_rx_buf_len;
        let rx_max_single_pkt_len: u32 = if self.amsdu_rx_enable {
            3000
        } else if self.ampdu_rx_enable {
            1600
        } else {
            1600 - 524
        };
        let rx_buf_len: u32 = 524; // WIFI_HW_RX_BUFFER_LEN

        sys::wifi_init_config_t {
            event_handler: Some(esp_event_send),
            osi_funcs: core::ptr::null_mut(),
            qos_enable: self.qos_enable as u8,
            ampdu_rx_enable: self.ampdu_rx_enable as u8,
            rx_ba_win: self.rx_ba_win,
            rx_ampdu_buf_num: ampdu_rx_buf_num,
            rx_ampdu_buf_len: ampdu_rx_buf_len,
            rx_max_single_pkt_len,
            rx_buf_len,
            amsdu_rx_enable: self.amsdu_rx_enable as u8,
            rx_buf_num: self.rx_buf_num,
            rx_pkt_num: self.rx_pkt_num,
            left_continuous_rx_buf_num: self.left_continuous_rx_buf_num,
            tx_buf_num: self.tx_buf_num,
            nvs_enable: self.nvs_enable as u8,
            nano_enable: 0,
            wpa3_sae_enable: self.wpa3_sae_enable as u8,
            magic: sys::WIFI_INIT_CONFIG_MAGIC,
        }
    }
}

/// Central WiFi controller.
///
/// Owns the WiFi driver lifecycle. Only one instance can exist at a time.
/// Dropping the controller de-initializes the WiFi subsystem.
pub struct WifiController {
    _guard: CompatGuard,
    _not_send_sync: core::marker::PhantomData<*const ()>,
}

impl WifiController {
    /// Initialize the WiFi driver with the given configuration.
    ///
    /// This ensures the FreeRTOS compat scheduler is running (needed by WiFi
    /// blob tasks), then calls `esp_wifi_init`, allocating internal buffers
    /// and starting the WiFi task. Only one controller can be active; returns
    /// [`WifiError::Internal`] if already initialized.
    pub fn init(config: WifiInitConfig) -> Result<Self, WifiError> {
        crate::println!("WifiController::init enter");
        if INITIALIZED.swap(true, Ordering::AcqRel) {
            return Err(WifiError::Internal(EspError(sys::ESP_ERR_INVALID_STATE)));
        }

        // Verify g_phyFuns is properly initialized by linker.
        extern "C" {
            static g_phyFuns: *const u32;
        }
        let funs = unsafe { g_phyFuns };
        crate::println!("g_phyFuns = {:#010x}", funs as usize);
        if !funs.is_null() {
            crate::logger::write_byte(b'Y');
            let _first = unsafe { *funs };
            crate::logger::write_byte(b'Z');
        }

        // PHY clocks already initialized by esp_phy_init_clk() in hardware_init.
        // Do NOT call phy::init_clocks() — double rtc_init garbles UART + corrupts RF.

        // Match the current SDK startup shape.
        //
        // In `startup.c`, Espressif only switches to 160 MHz under
        // `CONFIG_ESP8266_DEFAULT_CPU_FREQ_160`. Our generated sdkconfig for
        // this tree does not enable that path, and the compat port still uses
        // the SDK's 80 MHz `xtbsp_clock_freq_hz()` tick source.
        //
        // Keep the startup CPU frequency unchanged here instead of forcing a
        // mixed 160 MHz DPORT state on top of an 80 MHz scheduler model.

        // Ensure the compat scheduler is running — WiFi blob tasks need it
        // for internal timing during RF calibration.
        crate::logger::write_byte(b'S');
        let guard = CompatGuard::new();
        crate::logger::write_byte(b'T');

        // Match the SDK startup contract: `esp_mac_init()` belongs to startup
        // before `app_main`, and `hardware_init()` already mirrors that order.
        crate::logger::write_byte(b'6');

        // DPORT double-clock already set above before scheduler.

        crate::logger::write_byte(b'7');
        EspError::check(unsafe { sys::esp_event_loop_create_default() })?;
        crate::logger::write_byte(b'{');
        crate::logger::write_byte(b'1');

        // Mark WDEV registers as accessible before WiFi init starts
        crate::logger::write_byte(b'2');
        unsafe { super::compat::critical::mark_wdev_ready() };
        crate::logger::write_byte(b'}');
        crate::logger::write_byte(b'3');

        crate::logger::write_byte(b'W');
        super::compat::memory::clear_alloc_failure_debug();
        let raw = config.to_raw();
        crate::logger::write_byte(b'<');
        let ret = unsafe { sys::esp_wifi_init(&raw) };
        crate::logger::write_byte(if ret == sys::ESP_OK { b'w' } else { b'!' });
        crate::logger::write_byte(b'>');
        if ret != sys::ESP_OK {
            if let Some((kind, size, free, task, count)) =
                super::compat::memory::debug_last_alloc_failure()
            {
                crate::println!(
                    "wifi alloc fail kind={} size=0x{:X} free=0x{:X} task={} count={}",
                    kind as char,
                    size,
                    free,
                    task as char,
                    count,
                );
            }
        }
        if let Err(e) = EspError::check(ret) {
            INITIALIZED.store(false, Ordering::Release);
            return Err(e.into());
        }

        // Switch ets_delay_us from broken CCOUNT-based (init mode) to accurate
        // FRC2-based (runtime mode). This is needed for scan/auth to have proper
        // dwell timing. Must be AFTER esp_wifi_init since register_chipv6_phy
        // depends on broken delays during calibration.
        super::sdk::delay::enable_frc2_delay();

        Ok(Self {
            _guard: guard,
            _not_send_sync: core::marker::PhantomData,
        })
    }

    /// Set the WiFi operating mode.
    pub fn set_mode(&mut self, mode: WifiMode) -> Result<(), WifiError> {
        EspError::check(unsafe { sys::esp_wifi_set_mode(mode.to_raw()) })?;
        Ok(())
    }

    /// Get the current WiFi operating mode.
    pub fn get_mode(&self) -> Result<WifiMode, WifiError> {
        let mut raw: sys::wifi_mode_t = 0;
        EspError::check(unsafe { sys::esp_wifi_get_mode(&mut raw) })?;
        WifiMode::from_raw(raw).ok_or(WifiError::Internal(EspError(sys::ESP_ERR_WIFI_MODE)))
    }

    /// Apply a combined [`Configuration`].
    ///
    /// This sets the mode and both STA/AP configs in one call.
    pub fn set_configuration(&mut self, config: &Configuration) -> Result<(), WifiError> {
        match config {
            Configuration::None => {
                self.set_mode(WifiMode::Sta)?;
            }
            Configuration::Client(sta) => {
                self.set_mode(WifiMode::Sta)?;
                self.set_sta_config(sta)?;
            }
            Configuration::AccessPoint(ap) => {
                self.set_mode(WifiMode::Ap)?;
                self.set_ap_config(ap)?;
            }
            Configuration::Mixed(sta, ap) => {
                self.set_mode(WifiMode::ApSta)?;
                self.set_sta_config(sta)?;
                self.set_ap_config(ap)?;
            }
        }
        Ok(())
    }

    /// Get the current combined configuration.
    pub fn get_configuration(&self) -> Result<Configuration, WifiError> {
        let mode = self.get_mode()?;
        match mode {
            WifiMode::Sta => Ok(Configuration::Client(self.get_sta_config()?)),
            WifiMode::Ap => Ok(Configuration::AccessPoint(self.get_ap_config()?)),
            WifiMode::ApSta => Ok(Configuration::Mixed(
                self.get_sta_config()?,
                self.get_ap_config()?,
            )),
        }
    }

    /// Set the station configuration.
    pub fn set_sta_config(&mut self, config: &StaConfig) -> Result<(), WifiError> {
        let raw_sta = config.to_raw();
        let mut raw = sys::wifi_config_t { sta: raw_sta };
        EspError::check(unsafe { sys::esp_wifi_set_config(sys::WIFI_IF_STA, &mut raw) })?;
        Ok(())
    }

    /// Get the current station configuration.
    pub fn get_sta_config(&self) -> Result<StaConfig, WifiError> {
        let mut raw: sys::wifi_config_t = unsafe { core::mem::zeroed() };
        EspError::check(unsafe { sys::esp_wifi_get_config(sys::WIFI_IF_STA, &mut raw) })?;
        Ok(StaConfig::from_raw(unsafe { &raw.sta }))
    }

    /// Set the access-point configuration.
    pub fn set_ap_config(&mut self, config: &ApConfig) -> Result<(), WifiError> {
        let raw_ap = config.to_raw();
        let mut raw = sys::wifi_config_t { ap: raw_ap };
        EspError::check(unsafe { sys::esp_wifi_set_config(sys::WIFI_IF_AP, &mut raw) })?;
        Ok(())
    }

    /// Get the current access-point configuration.
    pub fn get_ap_config(&self) -> Result<ApConfig, WifiError> {
        let mut raw: sys::wifi_config_t = unsafe { core::mem::zeroed() };
        EspError::check(unsafe { sys::esp_wifi_get_config(sys::WIFI_IF_AP, &mut raw) })?;
        Ok(ApConfig::from_raw(unsafe { &raw.ap }))
    }

    /// Start the WiFi driver according to the current mode and configuration.
    pub fn start(&mut self) -> Result<(), WifiError> {
        crate::wifi::event::ensure_trampoline_registered()?;
        EspError::check(unsafe { sys::esp_wifi_start() })?;
        Ok(())
    }

    /// Stop the WiFi driver.
    pub fn stop(&mut self) -> Result<(), WifiError> {
        EspError::check(unsafe { sys::esp_wifi_stop() })?;
        log::info!("WiFi stopped");
        Ok(())
    }

    /// Connect to the configured AP (station mode, blocking).
    ///
    /// Initiates the connection. Use [`connect_async`](Self::connect_async)
    /// to await the `StaConnected` event, or create an
    /// [`event::event_subscriber`](crate::wifi::event::event_subscriber) to poll
    /// events yourself.
    pub fn connect(&mut self) -> Result<(), WifiError> {
        EspError::check(unsafe { sys::esp_wifi_connect() })?;
        Ok(())
    }

    /// Wait for SDK-style event-task auto-connect to reach the connected state.
    ///
    /// This is intended for flows that enable reconnect in the SDK event task
    /// before `esp_wifi_start()`, mirroring the ESP8266 RTOS SDK station
    /// example. The current task blocks with compat `vTaskDelay()`, closer to
    /// the SDK's blocking app-task waits than an async timer loop.
    #[inline(never)]
    pub fn wait_for_auto_connect_blocking(
        &mut self,
        max_polls: u32,
        poll_delay_ticks: u32,
    ) -> Result<(), WifiError> {
        let _ = self;
        let fail_seq = crate::wifi::event::sdk_sta_auto_connect_fail_seq();

        for _ in 0..max_polls {
            match current_sta_state() {
                StaConnectionState::Connected => return Ok(()),
                StaConnectionState::Disconnected => {
                    if crate::wifi::event::sdk_sta_auto_connect_fail_seq() != fail_seq {
                        break;
                    }
                }
                StaConnectionState::Connecting => {}
            }

            unsafe { crate::wifi::compat::task::vTaskDelay(poll_delay_ticks) };
        }

        Err(WifiError::Disconnected(last_sta_disconnect_reason()))
    }

    /// Wait for SDK-style event-task auto-connect to finish without a local timeout.
    ///
    /// This mirrors the ESP8266 RTOS SDK station example more closely than
    /// [`wait_for_auto_connect_blocking`](Self::wait_for_auto_connect_blocking):
    /// the app task blocks until the event-task retry loop either reaches the
    /// connected state or latches a terminal auto-connect failure sequence,
    /// analogous to the SDK's `xEventGroupWaitBits(..., portMAX_DELAY)`.
    #[inline(never)]
    pub fn wait_for_auto_connect_result_blocking(
        &mut self,
        poll_delay_ticks: u32,
    ) -> Result<(), WifiError> {
        let _ = poll_delay_ticks;
        let _ = self;
        let bits = crate::wifi::event::wait_for_sdk_sta_auto_connect_bits()?;
        if (bits & 0x1) != 0 {
            Ok(())
        } else {
            Err(WifiError::Disconnected(last_sta_disconnect_reason()))
        }
    }

    /// Connect to the configured AP, blocking until connected.
    ///
    /// This follows the current embassy bring-up flow:
    /// - issue `esp_wifi_connect()`
    /// - retry from `WIFI_EVENT_STA_DISCONNECTED`
    /// - block until `WIFI_EVENT_STA_CONNECTED`
    ///
    /// DHCP/config-up is intentionally not part of this wait path. The caller
    /// is expected to start the network stack afterwards and wait for config
    /// there (for example `embassy-net`'s `wait_config_up()`).
    #[inline(never)]
    pub fn connect_blocking(&mut self) -> Result<(), WifiError> {
        crate::wifi::event::ensure_trampoline_registered()?;

        let event_group = unsafe { crate::wifi::compat::event_group::xEventGroupCreate() };
        if event_group.is_null() {
            return Err(WifiError::Internal(EspError(sys::ESP_ERR_NO_MEM)));
        }

        let mut wifi_handler_registered = false;
        let mut connected_handler_registered = false;

        let result = (|| {
            STA_STATE.store(StaConnectionState::Connecting as u8, Ordering::Release);

            EspError::check(unsafe {
                sys::esp_event_handler_register(
                    sys::WIFI_EVENT,
                    sys::WIFI_EVENT_STA_DISCONNECTED as i32,
                    Some(connect_blocking_disconnect_handler),
                    core::ptr::null_mut(),
                )
            })?;
            wifi_handler_registered = true;

            EspError::check(unsafe {
                sys::esp_event_handler_register(
                    sys::WIFI_EVENT,
                    sys::WIFI_EVENT_STA_CONNECTED as i32,
                    Some(connect_blocking_connected_handler),
                    event_group,
                )
            })?;
            connected_handler_registered = true;

            unsafe {
                crate::wifi::compat::event_group::xEventGroupClearBits(
                    event_group,
                    CONNECT_WAIT_STA_CONNECTED_BIT,
                );
            }

            EspError::check(unsafe { sys::esp_wifi_connect() })?;

            let bits = unsafe {
                crate::wifi::compat::event_group::xEventGroupWaitBits(
                    event_group,
                    CONNECT_WAIT_STA_CONNECTED_BIT,
                    1,
                    1,
                    0xFFFF_FFFF,
                )
            };

            if (bits & CONNECT_WAIT_STA_CONNECTED_BIT) != 0 {
                Ok(())
            } else {
                Err(WifiError::Disconnected(last_sta_disconnect_reason()))
            }
        })();

        if connected_handler_registered {
            let _ = unsafe {
                sys::esp_event_handler_unregister(
                    sys::WIFI_EVENT,
                    sys::WIFI_EVENT_STA_CONNECTED as i32,
                    Some(connect_blocking_connected_handler),
                )
            };
        }
        if wifi_handler_registered {
            let _ = unsafe {
                sys::esp_event_handler_unregister(
                    sys::WIFI_EVENT,
                    sys::WIFI_EVENT_STA_DISCONNECTED as i32,
                    Some(connect_blocking_disconnect_handler),
                )
            };
        }
        unsafe { crate::wifi::compat::event_group::vEventGroupDelete(event_group) };

        result
    }

    /// Connect to the configured AP and wait for the connection event.
    ///
    /// Uses timer-based polling of the atomic connection state and retries on
    /// disconnect, matching the SDK station examples' "disconnect -> reconnect"
    /// behavior while keeping the async state machine small.
    ///
    /// Waits up to ~10s for scan + WPA handshake retries to complete. The event
    /// trampoline updates atomic state and disconnect reason on
    /// `StaConnected` / `StaDisconnected`.
    #[inline(never)]
    pub async fn connect_async(&mut self) -> Result<(), WifiError> {
        // Mark Connecting so we don't exit on stale Disconnected state.
        STA_STATE.store(StaConnectionState::Connecting as u8, Ordering::Release);
        let mut seen_disconnect_seq = STA_DISCONNECT_SEQ.load(Ordering::Acquire);
        EspError::check(unsafe { sys::esp_wifi_connect() })?;

        // Poll every 200ms, up to 50 times (10s).
        for _ in 0u32..50 {
            embassy_time::Timer::after(embassy_time::Duration::from_millis(200)).await;
            match current_sta_state() {
                StaConnectionState::Connected => return Ok(()),
                StaConnectionState::Disconnected => {
                    let seq = STA_DISCONNECT_SEQ.load(Ordering::Acquire);
                    if seq != seen_disconnect_seq {
                        seen_disconnect_seq = seq;
                        let _ = unsafe { sys::esp_wifi_connect() };
                    }
                }
                StaConnectionState::Connecting => {}
            }
        }
        Err(WifiError::Disconnected(last_sta_disconnect_reason()))
    }

    /// Disconnect from the AP (station mode, blocking).
    pub fn disconnect(&mut self) -> Result<(), WifiError> {
        EspError::check(unsafe { sys::esp_wifi_disconnect() })?;
        Ok(())
    }

    /// Disconnect and wait for the disconnection event.
    pub async fn disconnect_async(&mut self) -> Result<(), WifiError> {
        crate::wifi::event::ensure_trampoline_registered()?;
        let mut sub = crate::wifi::event::event_subscriber()
            .ok_or(WifiError::Internal(EspError(sys::ESP_ERR_NO_MEM)))?;
        EspError::check(unsafe { sys::esp_wifi_disconnect() })?;

        loop {
            let event = sub.next_message_pure().await;
            if matches!(event, crate::wifi::WifiEvent::StaDisconnected { .. }) {
                return Ok(());
            }
        }
    }

    /// Set the configuration storage destination.
    pub fn set_storage(&mut self, storage: Storage) -> Result<(), WifiError> {
        EspError::check(unsafe { sys::esp_wifi_set_storage(storage.to_raw()) })?;
        Ok(())
    }

    /// Set the 802.11 protocol bitmap for an interface.
    pub fn set_protocol(
        &mut self,
        interface: WifiInterface,
        protocol: Protocol,
    ) -> Result<(), WifiError> {
        EspError::check(unsafe {
            sys::esp_wifi_set_protocol(interface.to_raw(), protocol.bits())
        })?;
        Ok(())
    }

    /// Get the 802.11 protocol bitmap for an interface.
    pub fn get_protocol(&self, interface: WifiInterface) -> Result<Protocol, WifiError> {
        let mut bits: u8 = 0;
        EspError::check(unsafe { sys::esp_wifi_get_protocol(interface.to_raw(), &mut bits) })?;
        Ok(Protocol::from_bits_truncate(bits))
    }

    /// Set the channel bandwidth for an interface.
    pub fn set_bandwidth(
        &mut self,
        interface: WifiInterface,
        bw: Bandwidth,
    ) -> Result<(), WifiError> {
        EspError::check(unsafe { sys::esp_wifi_set_bandwidth(interface.to_raw(), bw.to_raw()) })?;
        Ok(())
    }

    /// Get the channel bandwidth for an interface.
    pub fn get_bandwidth(&self, interface: WifiInterface) -> Result<Bandwidth, WifiError> {
        let mut raw: sys::wifi_bandwidth_t = 0;
        EspError::check(unsafe { sys::esp_wifi_get_bandwidth(interface.to_raw(), &mut raw) })?;
        Ok(Bandwidth::from_raw(raw))
    }

    /// Set the primary and secondary channel.
    pub fn set_channel(
        &mut self,
        primary: u8,
        secondary: SecondaryChannel,
    ) -> Result<(), WifiError> {
        EspError::check(unsafe { sys::esp_wifi_set_channel(primary, secondary.to_raw()) })?;
        Ok(())
    }

    /// Get the current primary and secondary channel.
    pub fn get_channel(&self) -> Result<(u8, SecondaryChannel), WifiError> {
        let mut primary: u8 = 0;
        let mut secondary: sys::wifi_second_chan_t = 0;
        EspError::check(unsafe { sys::esp_wifi_get_channel(&mut primary, &mut secondary) })?;
        Ok((primary, SecondaryChannel::from_raw(secondary)))
    }

    /// Set the MAC address for an interface.
    pub fn set_mac(&mut self, interface: WifiInterface, mac: &[u8; 6]) -> Result<(), WifiError> {
        EspError::check(unsafe { sys::esp_wifi_set_mac(interface.to_raw(), mac.as_ptr()) })?;
        Ok(())
    }

    /// Get the MAC address of an interface.
    pub fn get_mac(&self, interface: WifiInterface) -> Result<[u8; 6], WifiError> {
        let mut mac = [0u8; 6];
        EspError::check(unsafe { sys::esp_wifi_get_mac(interface.to_raw(), mac.as_mut_ptr()) })?;
        Ok(mac)
    }

    /// Set the power-save mode.
    pub fn set_power_save(&mut self, ps: PowerSave) -> Result<(), WifiError> {
        EspError::check(unsafe { sys::esp_wifi_set_ps(ps.to_raw()) })?;
        Ok(())
    }

    /// Get the current power-save mode.
    pub fn get_power_save(&self) -> Result<PowerSave, WifiError> {
        let mut raw: sys::wifi_ps_type_t = 0;
        EspError::check(unsafe { sys::esp_wifi_get_ps(&mut raw) })?;
        Ok(PowerSave::from_raw(raw))
    }

    /// Set maximum TX power (see SDK docs for the mapping table).
    pub fn set_max_tx_power(&mut self, power: i8) -> Result<(), WifiError> {
        EspError::check(unsafe { sys::esp_wifi_set_max_tx_power(power) })?;
        Ok(())
    }

    /// Get maximum TX power.
    pub fn get_max_tx_power(&self) -> Result<i8, WifiError> {
        let mut power: i8 = 0;
        EspError::check(unsafe { sys::esp_wifi_get_max_tx_power(&mut power) })?;
        Ok(power)
    }

    /// Set country configuration.
    pub fn set_country(&mut self, country: &CountryConfig) -> Result<(), WifiError> {
        let raw = country.to_raw();
        EspError::check(unsafe { sys::esp_wifi_set_country(&raw) })?;
        Ok(())
    }

    /// Get the current country configuration.
    pub fn get_country(&self) -> Result<CountryConfig, WifiError> {
        let mut raw: sys::wifi_country_t = unsafe { core::mem::zeroed() };
        EspError::check(unsafe { sys::esp_wifi_get_country(&mut raw) })?;
        Ok(CountryConfig::from_raw(&raw))
    }

    /// Set the RSSI threshold for `WifiEvent::StaBssRssiLow` events.
    ///
    /// Must be called after each `StaBssRssiLow` event to re-arm.
    pub fn set_rssi_threshold(&mut self, rssi: i32) -> Result<(), WifiError> {
        EspError::check(unsafe { sys::esp_wifi_set_rssi_threshold(rssi) })?;
        Ok(())
    }

    /// Set the inactive time for an interface.
    pub fn set_inactive_time(
        &mut self,
        interface: WifiInterface,
        seconds: u16,
    ) -> Result<(), WifiError> {
        EspError::check(unsafe { sys::esp_wifi_set_inactive_time(interface.to_raw(), seconds) })?;
        Ok(())
    }

    /// Get the inactive time for an interface.
    pub fn get_inactive_time(&self, interface: WifiInterface) -> Result<u16, WifiError> {
        let mut sec: u16 = 0;
        EspError::check(unsafe { sys::esp_wifi_get_inactive_time(interface.to_raw(), &mut sec) })?;
        Ok(sec)
    }

    /// Restore all WiFi settings to SDK defaults.
    pub fn restore_defaults(&mut self) -> Result<(), WifiError> {
        EspError::check(unsafe { sys::esp_wifi_restore() })?;
        Ok(())
    }

    /// Get the TSF time of an interface.
    pub fn get_tsf_time(&self, interface: WifiInterface) -> i64 {
        unsafe { sys::esp_wifi_get_tsf_time(interface.to_raw()) }
    }

    /// Send a raw 802.11 frame.
    ///
    /// # Safety
    ///
    /// The buffer must contain a valid 802.11 frame (24–1400 bytes, no FCS).
    pub unsafe fn send_raw_frame(
        &mut self,
        interface: WifiInterface,
        buffer: &[u8],
        use_sys_seq: bool,
    ) -> Result<(), WifiError> {
        EspError::check(unsafe {
            sys::esp_wifi_80211_tx(
                interface.to_raw(),
                buffer.as_ptr() as *const core::ffi::c_void,
                buffer.len() as core::ffi::c_int,
                use_sys_seq,
            )
        })?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Connection state queries
    // -----------------------------------------------------------------------

    /// Returns the current station connection state.
    pub fn sta_state(&self) -> StaConnectionState {
        StaConnectionState::from_u8(STA_STATE.load(Ordering::Acquire))
    }

    /// Returns `true` if the station is connected to an AP.
    pub fn is_connected(&self) -> bool {
        self.sta_state() == StaConnectionState::Connected
    }

    /// Returns `true` if the soft-AP is started.
    pub fn is_ap_started(&self) -> bool {
        AP_STATE.load(Ordering::Acquire) != 0
    }

    // -----------------------------------------------------------------------
    // Scanning
    // -----------------------------------------------------------------------

    /// Perform a scan and return results (async, awaits completion).
    ///
    /// The station interface must be started. This starts a non-blocking
    /// scan, awaits the `ScanDone` event, and retrieves results.
    pub async fn scan_async(
        &mut self,
        config: &ScanConfig,
    ) -> Result<heapless::Vec<AccessPointInfo, 32>, WifiError> {
        crate::wifi::scan::scan_async(config).await
    }

    /// Perform a blocking scan and return results.
    pub fn scan_blocking(
        &mut self,
        config: &ScanConfig,
    ) -> Result<heapless::Vec<AccessPointInfo, 32>, WifiError> {
        crate::wifi::scan::scan_blocking(config)
    }

    // -----------------------------------------------------------------------
    // Device creation
    // -----------------------------------------------------------------------

    /// Create a [`WifiDevice`] for the given interface.
    ///
    /// This is a convenience method. The device must have its RX callback
    /// registered (via [`WifiDevice::register_rx_callback`]) after
    /// [`start()`](Self::start).
    pub fn create_device(&self, interface: WifiInterface) -> WifiDevice {
        WifiDevice::new(interface)
    }

    /// Initialize WiFi and create devices for the configured mode.
    ///
    /// Returns the controller and one or two [`WifiDevice`]s depending on
    /// the mode set in `config`. The RX callbacks still need to be
    /// registered after calling [`start()`](WifiController::start).
    pub fn init_with_devices(
        config: WifiInitConfig,
        wifi_config: &Configuration,
    ) -> Result<(Self, WifiDevice, Option<WifiDevice>), WifiError> {
        let mut ctrl = Self::init(config)?;
        ctrl.set_configuration(wifi_config)?;

        let (sta_dev, ap_dev) = match wifi_config {
            Configuration::None | Configuration::Client(_) => {
                (WifiDevice::new(WifiInterface::Sta), None)
            }
            Configuration::AccessPoint(_) => (WifiDevice::new(WifiInterface::Ap), None),
            Configuration::Mixed(_, _) => (
                WifiDevice::new(WifiInterface::Sta),
                Some(WifiDevice::new(WifiInterface::Ap)),
            ),
        };

        Ok((ctrl, sta_dev, ap_dev))
    }

    /// De-initialize the WiFi subsystem explicitly.
    ///
    /// This is also called automatically on drop.
    pub fn deinit(self) -> Result<(), WifiError> {
        // Drop will handle it
        drop(self);
        Ok(())
    }
}

impl Drop for WifiController {
    fn drop(&mut self) {
        let _ = unsafe { sys::esp_wifi_stop() };
        let _ = unsafe { sys::esp_wifi_deinit() };
        INITIALIZED.store(false, Ordering::Release);
        log::info!("WiFi de-initialized");
    }
}

/// WiFi interface selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiInterface {
    Sta,
    Ap,
}

impl WifiInterface {
    pub(crate) fn to_raw(self) -> sys::wifi_interface_t {
        match self {
            Self::Sta => sys::WIFI_IF_STA,
            Self::Ap => sys::WIFI_IF_AP,
        }
    }
}

/// Country/region configuration.
#[derive(Debug, Clone)]
pub struct CountryConfig {
    /// Two-letter country code (e.g., "US", "CN", "JP").
    pub country_code: [u8; 2],
    /// Start channel (typically 1).
    pub start_channel: u8,
    /// Number of channels.
    pub num_channels: u8,
    /// Maximum TX power (dBm * 4, i.e. quarter-dBm units).
    pub max_tx_power: i8,
    /// Country policy.
    pub policy: CountryPolicy,
}

impl Default for CountryConfig {
    fn default() -> Self {
        Self {
            // "01" = world-safe default (all channels, conservative power).
            country_code: *b"01",
            start_channel: 1,
            num_channels: 14,
            max_tx_power: 20,
            policy: CountryPolicy::Auto,
        }
    }
}

impl CountryConfig {
    fn to_raw(&self) -> sys::wifi_country_t {
        sys::wifi_country_t {
            cc: [
                self.country_code[0] as core::ffi::c_char,
                self.country_code[1] as core::ffi::c_char,
                0,
            ],
            schan: self.start_channel,
            nchan: self.num_channels,
            max_tx_power: self.max_tx_power,
            policy: self.policy.to_raw(),
        }
    }

    fn from_raw(raw: &sys::wifi_country_t) -> Self {
        Self {
            country_code: [raw.cc[0] as u8, raw.cc[1] as u8],
            start_channel: raw.schan,
            num_channels: raw.nchan,
            max_tx_power: raw.max_tx_power,
            policy: CountryPolicy::from_raw(raw.policy),
        }
    }
}

/// Country policy for channel/power configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CountryPolicy {
    /// Use the connected AP's country info when available.
    Auto,
    /// Always use the manually configured country info.
    Manual,
}

impl CountryPolicy {
    fn to_raw(self) -> sys::wifi_country_policy_t {
        match self {
            Self::Auto => sys::WIFI_COUNTRY_POLICY_AUTO,
            Self::Manual => sys::WIFI_COUNTRY_POLICY_MANUAL,
        }
    }

    fn from_raw(raw: sys::wifi_country_policy_t) -> Self {
        match raw {
            sys::WIFI_COUNTRY_POLICY_MANUAL => Self::Manual,
            _ => Self::Auto,
        }
    }
}
