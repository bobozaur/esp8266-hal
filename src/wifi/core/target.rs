use portable_atomic::{AtomicBool, AtomicU32, Ordering};

#[cfg(feature = "wifi-promiscuous")]
use super::feature::PROMISCUOUS_MAX_PAYLOAD_PREFIX;
use super::target_callback::CallbackRegistration;
use super::target_map::*;
use super::{
    AccessPointConfig, AccessPointRecord, ApStationInfo, AuthenticationMethod, Channel,
    ClientConfig, ConnectError, Deadline, DisconnectError, DriverError, InitConfig, InitError,
    RawMode, RawRxRecord, ScanOptions, StationPowerSave, Uninitialized, WifiApBackend, WifiDriver,
    WifiEvent, WifiFeatureBackend, WifiLifecycleBackend, WifiPacketBackend, WifiResources,
    WifiRuntimeBackend, WifiScanBackend, WifiStaBackend,
};
#[cfg(feature = "wifi-esp-now")]
use super::{EspNowPeerConfig, EspNowRxFrame, EspNowSendStatus};
#[cfg(feature = "wifi-smartconfig")]
use super::{
    Ipv4Addr, SmartConfigCredentials, SmartConfigStartConfig, SmartConfigTimeout, SmartConfigType,
};
#[cfg(feature = "wifi-promiscuous")]
use super::{
    PromiscuousControlFilter, PromiscuousFilter, PromiscuousFrameInfo, PromiscuousPacketType,
    PromiscuousRxFrame,
};
#[cfg(feature = "wifi-enterprise")]
use super::{Wpa2EnterpriseCredentials, Wpa2EnterpriseMaterialFormat};
#[cfg(feature = "wifi-wps")]
use super::{WpsConfig, WpsPin};

#[cfg(feature = "wifi-wps")]
extern "C" {
    fn esp8266_hal_wps_set_pin(pin: *const u8);
    fn esp8266_hal_wps_set_target_ssid(ssid: *const u8, len: u32);
}

#[cfg(feature = "wifi-promiscuous")]
static PROMISCUOUS_FILTER_BITS: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "wifi-promiscuous")]
static PROMISCUOUS_CHANNEL: AtomicU32 = AtomicU32::new(0);
static CORE_STA_START_SEQ: AtomicU32 = AtomicU32::new(0);
static WIFI_INSTANCE_TAKEN: AtomicBool = AtomicBool::new(false);
static CORE_STA_AUTO_RECONNECT: AtomicBool = AtomicBool::new(false);

// STA_START normally arrives on the first scheduler turn after
// `esp_wifi_start()`. Keep a wide 1s margin so `start()` fails explicitly
// instead of letting callers race an SDK station state that is not ready.
const STA_START_EVENT_TIMEOUT_MS: u32 = 1_000;

pub struct Esp8266TargetBackend<
    'd,
    const EVENTS: usize,
    const RX: usize,
    const ESP_NOW_PEERS: usize = 0,
    const ESP_NOW_RX: usize = 0,
    const PROMISCUOUS_RX: usize = 0,
    const AP_RX_SLOTS: usize = 0,
    const AP_RX_BYTES: usize = 0,
> {
    initialized: bool,
    started: bool,
    mode: Option<RawMode>,
    resources: &'d WifiResources<
        EVENTS,
        RX,
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
    #[cfg(feature = "wifi-enterprise")]
    wpa2_enterprise_enabled: bool,
    #[cfg(feature = "wifi-enterprise")]
    eap_workspace_reserved: bool,
    #[cfg(feature = "wifi-promiscuous")]
    promiscuous_enabled: bool,
    #[cfg(feature = "wifi-esp-now")]
    esp_now_initialized: bool,
    #[cfg(feature = "wifi-esp-now")]
    esp_now_recv_registered: bool,
    #[cfg(feature = "wifi-esp-now")]
    esp_now_send_registered: bool,
    #[cfg(feature = "wifi-wps")]
    wps_enabled: bool,
    #[cfg(feature = "wifi-smartconfig")]
    smartconfig_active: bool,
    station_power_save: StationPowerSave,
}

impl<
        'd,
        const EVENTS: usize,
        const RX: usize,
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
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
{
    pub(crate) fn new(
        resources: &'d WifiResources<
            EVENTS,
            RX,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
    ) -> Self {
        Self {
            initialized: false,
            started: false,
            mode: None,
            resources,
            registration: None,
            event_handler_registered: false,
            #[cfg(feature = "wifi-smartconfig")]
            smartconfig_handler_registered: false,
            sta_rx_registered: false,
            ap_rx_registered: false,
            ap_netif_started: false,
            #[cfg(feature = "wifi-enterprise")]
            wpa2_enterprise_enabled: false,
            #[cfg(feature = "wifi-enterprise")]
            eap_workspace_reserved: false,
            #[cfg(feature = "wifi-promiscuous")]
            promiscuous_enabled: false,
            #[cfg(feature = "wifi-esp-now")]
            esp_now_initialized: false,
            #[cfg(feature = "wifi-esp-now")]
            esp_now_recv_registered: false,
            #[cfg(feature = "wifi-esp-now")]
            esp_now_send_registered: false,
            #[cfg(feature = "wifi-wps")]
            wps_enabled: false,
            #[cfg(feature = "wifi-smartconfig")]
            smartconfig_active: false,
            station_power_save: StationPowerSave::None,
        }
    }

    fn ensure_initialized(&self) -> Result<(), DriverError> {
        if self.initialized {
            Ok(())
        } else {
            Err(DriverError::BackendRejected)
        }
    }

    fn apply_station_power_save_for_mode(&mut self) -> Result<(), DriverError> {
        self.ensure_initialized()?;
        if matches!(self.mode, Some(RawMode::Sta | RawMode::ApSta)) {
            let power_save = raw_station_power_save(self.station_power_save);
            crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_wifi_set_ps(power_save) })
                .map_err(map_esp_driver_error)?;
        }
        Ok(())
    }

    #[cfg(feature = "wifi-enterprise")]
    fn configure_wpa2_enterprise_for_station(
        &mut self,
        config: &ClientConfig<'_>,
    ) -> Result<(), DriverError> {
        if let Some(credentials) = config.credential().enterprise_credentials() {
            self.enable_wpa2_enterprise(credentials)
        } else {
            self.disable_wpa2_enterprise_if_enabled()
        }
    }

    #[cfg(feature = "wifi-enterprise")]
    fn enable_wpa2_enterprise(
        &mut self,
        credentials: &Wpa2EnterpriseCredentials<'_>,
    ) -> Result<(), DriverError> {
        self.disable_wpa2_enterprise_if_enabled()?;
        self.clear_wpa2_enterprise_credentials();
        let result = self.enable_wpa2_enterprise_inner(credentials);
        if result.is_err() {
            self.clear_wpa2_enterprise_credentials();
            return result;
        }

        self.wpa2_enterprise_enabled = true;
        if credentials.client_certificate().is_some() {
            crate::wifi::compat::memory::reserve_eap_rsa_workspace();
            self.eap_workspace_reserved = true;
        }
        Ok(())
    }

    #[cfg(feature = "wifi-enterprise")]
    fn enable_wpa2_enterprise_inner(
        &mut self,
        credentials: &Wpa2EnterpriseCredentials<'_>,
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
            require_enterprise_material(
                ca_cert,
                credentials
                    .ca_cert_format()
                    .unwrap_or(Wpa2EnterpriseMaterialFormat::Pem),
            )?;
            crate::wifi::EspError::check(unsafe {
                esp8266_wifi_sys::esp_wifi_sta_wpa2_ent_set_ca_cert(
                    ca_cert.as_ptr(),
                    ca_cert.len() as i32,
                )
            })
            .map_err(map_enterprise_esp_error)?;
        }

        if let Some(certificate) = credentials.client_certificate() {
            require_enterprise_material(
                certificate.client_cert(),
                certificate.client_cert_format(),
            )?;
            require_enterprise_material(
                certificate.private_key(),
                certificate.private_key_format(),
            )?;
            let password = certificate.private_key_password();
            if let Some(password) = password {
                require_enterprise_pem(password)?;
            }
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

        Ok(())
    }

    #[cfg(feature = "wifi-enterprise")]
    fn disable_wpa2_enterprise_if_enabled(&mut self) -> Result<(), DriverError> {
        if !self.wpa2_enterprise_enabled {
            return Ok(());
        }

        crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_wifi_sta_wpa2_ent_disable() })
            .map_err(map_enterprise_esp_error)?;
        self.clear_wpa2_enterprise_credentials();
        self.wpa2_enterprise_enabled = false;
        self.release_eap_workspace();
        Ok(())
    }

    #[cfg(feature = "wifi-enterprise")]
    fn release_eap_workspace(&mut self) {
        if self.eap_workspace_reserved {
            crate::wifi::compat::memory::release_eap_rsa_workspace();
            self.eap_workspace_reserved = false;
        }
    }

    #[cfg(feature = "wifi-enterprise")]
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
        self.ensure_initialized()?;
        CORE_STA_AUTO_RECONNECT.store(true, Ordering::Release);
        let result = self.connect_blocking_with_event_retries(timeout_ms);
        if result.is_err() {
            CORE_STA_AUTO_RECONNECT.store(false, Ordering::Release);
            if self.resources.link_state() != super::LinkState::Up {
                let _ = unsafe { esp8266_wifi_sys::esp_wifi_disconnect() };
            }
        }
        result
    }

    fn connect_blocking_with_event_retries(&mut self, timeout_ms: u32) -> Result<(), DriverError> {
        let deadline = Deadline::new(self.now_ms(), timeout_ms);
        crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_wifi_connect() })
            .map_err(map_esp_driver_error)?;

        loop {
            if self.resources.link_state() == super::LinkState::Up {
                self.apply_station_power_save_for_mode()?;
                return Ok(());
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

    fn unregister_event_handler(&mut self) -> Result<(), DriverError> {
        if !self.event_handler_registered {
            return Ok(());
        }

        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_event_handler_unregister(
                esp8266_wifi_sys::WIFI_EVENT,
                -1,
                Some(
                    wifi_core_event_trampoline::<
                        EVENTS,
                        RX,
                        ESP_NOW_PEERS,
                        ESP_NOW_RX,
                        PROMISCUOUS_RX,
                        AP_RX_SLOTS,
                        AP_RX_BYTES,
                    >,
                ),
            )
        })
        .map_err(|_| DriverError::BackendRejected)?;
        self.event_handler_registered = false;
        Ok(())
    }

    fn mac_address(
        &self,
        interface: esp8266_wifi_sys::wifi_interface_t,
    ) -> Result<[u8; 6], DriverError> {
        self.ensure_initialized()?;
        let mut mac = [0; 6];
        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_wifi_get_mac(interface, mac.as_mut_ptr())
        })
        .map_err(map_esp_driver_error)?;
        Ok(mac)
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
    fn unregister_smartconfig_event_handler(&mut self) -> Result<(), DriverError> {
        if !self.smartconfig_handler_registered {
            return Ok(());
        }

        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_event_handler_unregister(
                esp8266_wifi_sys::SC_EVENT,
                -1,
                Some(
                    smartconfig_event_trampoline::<
                        EVENTS,
                        RX,
                        ESP_NOW_PEERS,
                        ESP_NOW_RX,
                        PROMISCUOUS_RX,
                        AP_RX_SLOTS,
                        AP_RX_BYTES,
                    >,
                ),
            )
        })
        .map_err(|_| DriverError::BackendRejected)?;
        self.smartconfig_handler_registered = false;
        Ok(())
    }

    #[inline(never)]
    fn register_sta_rx_callback(&mut self) -> Result<(), DriverError> {
        let ret = unsafe {
            esp8266_wifi_sys::esp_wifi_internal_reg_rxcb(
                esp8266_wifi_sys::WIFI_IF_STA,
                Some(
                    rx_callback_sta::<
                        EVENTS,
                        RX,
                        ESP_NOW_PEERS,
                        ESP_NOW_RX,
                        PROMISCUOUS_RX,
                        AP_RX_SLOTS,
                        AP_RX_BYTES,
                    >,
                ),
            )
        };
        crate::wifi::EspError::check(ret).map_err(|_| DriverError::BackendRejected)
    }

    fn register_ap_rx_callback(&mut self) -> Result<(), DriverError> {
        let ret = unsafe {
            esp8266_wifi_sys::esp_wifi_internal_reg_rxcb(
                esp8266_wifi_sys::WIFI_IF_AP,
                Some(
                    rx_callback_ap::<
                        EVENTS,
                        RX,
                        ESP_NOW_PEERS,
                        ESP_NOW_RX,
                        PROMISCUOUS_RX,
                        AP_RX_SLOTS,
                        AP_RX_BYTES,
                    >,
                ),
            )
        };
        crate::wifi::EspError::check(ret).map_err(|_| DriverError::BackendRejected)
    }

    fn unregister_sta_rx_callback(&mut self) -> Result<(), DriverError> {
        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_wifi_internal_reg_rxcb(esp8266_wifi_sys::WIFI_IF_STA, None)
        })
        .map_err(|_| DriverError::BackendRejected)
    }

    fn unregister_ap_rx_callback(&mut self) -> Result<(), DriverError> {
        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_wifi_internal_reg_rxcb(esp8266_wifi_sys::WIFI_IF_AP, None)
        })
        .map_err(|_| DriverError::BackendRejected)
    }

    fn ensure_ap_netif_started(&mut self) -> Result<(), DriverError> {
        if self.ap_netif_started {
            return Ok(());
        }

        let ap_mac = self.ap_mac_address()?;
        crate::wifi::ap_netif::start(ap_mac)
            .map_err(|code| map_esp_driver_error(crate::wifi::EspError(code)))?;
        self.ap_netif_started = true;
        Ok(())
    }

    fn stop_ap_netif(&mut self) -> Result<(), DriverError> {
        if !self.ap_netif_started {
            return Ok(());
        }

        crate::wifi::ap_netif::stop()
            .map_err(|code| map_esp_driver_error(crate::wifi::EspError(code)))?;
        self.ap_netif_started = false;
        Ok(())
    }

    fn quiesce(&mut self) -> Result<(), DriverError> {
        CORE_STA_AUTO_RECONNECT.store(false, Ordering::Release);

        let mut result = Ok(());
        #[cfg(feature = "wifi-smartconfig")]
        if self.smartconfig_active {
            result = result.and(self.stop_smartconfig());
        }
        #[cfg(feature = "wifi-promiscuous")]
        if self.promiscuous_enabled {
            result = result.and(self.disable_promiscuous());
        }
        #[cfg(feature = "wifi-esp-now")]
        if self.esp_now_initialized || self.esp_now_recv_registered || self.esp_now_send_registered
        {
            result = result.and(self.disable_esp_now());
        }
        #[cfg(feature = "wifi-wps")]
        if self.wps_enabled {
            result = result.and(self.disable_wps());
        }
        #[cfg(feature = "wifi-enterprise")]
        {
            result = result.and(self.disable_wpa2_enterprise_if_enabled());
        }

        let stop_result = if self.started {
            crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_wifi_stop() })
                .map_err(map_esp_driver_error)
        } else {
            Ok(())
        };
        crate::wifi::compat::port::set_wdev_tick_poll_enabled(false);
        if stop_result.is_ok() {
            self.started = false;
            self.detach_ap_rx();
            self.detach_sta_rx();
            if self.ap_rx_registered || self.sta_rx_registered {
                result = result.and(Err(DriverError::BackendRejected));
            }
            result = result.and(self.stop_ap_netif());
            #[cfg(feature = "wifi-smartconfig")]
            {
                result = result.and(self.unregister_smartconfig_event_handler());
            }
            result = result.and(self.unregister_event_handler());
            self.resources.reset_runtime();
        }

        result.and(stop_result)
    }
}

impl<
        'd,
        const EVENTS: usize,
        const RX: usize,
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
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
{
    fn drop(&mut self) {
        if !self.initialized {
            return;
        }

        // Make every const-generic callback a no-op before teardown begins.
        // If any SDK callback cannot be unregistered, the singleton remains
        // quarantined so a later resource layout can never be observed by a
        // stale trampoline.
        self.registration = None;
        let _ = self.quiesce();
        crate::wifi::compat::port::set_wdev_tick_poll_enabled(false);
        let deinitialized =
            crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_wifi_deinit() }).is_ok();

        self.started = false;
        self.detach_ap_rx();
        self.detach_sta_rx();
        let _ = self.stop_ap_netif();
        #[cfg(feature = "wifi-smartconfig")]
        let _ = self.unregister_smartconfig_event_handler();
        let _ = self.unregister_event_handler();
        self.resources.reset_runtime();

        let callbacks_detached =
            !self.event_handler_registered && !self.sta_rx_registered && !self.ap_rx_registered;
        #[cfg(feature = "wifi-smartconfig")]
        let callbacks_detached =
            callbacks_detached && !self.smartconfig_handler_registered && !self.smartconfig_active;
        #[cfg(feature = "wifi-promiscuous")]
        let callbacks_detached = callbacks_detached && !self.promiscuous_enabled;
        #[cfg(feature = "wifi-esp-now")]
        let callbacks_detached = callbacks_detached
            && !self.esp_now_initialized
            && !self.esp_now_recv_registered
            && !self.esp_now_send_registered;
        #[cfg(feature = "wifi-wps")]
        let callbacks_detached = callbacks_detached && !self.wps_enabled;

        if deinitialized {
            unsafe { crate::wifi::compat::critical::clear_wdev_ready() };
            #[cfg(feature = "wifi-enterprise")]
            self.release_eap_workspace();
        }
        self.initialized = false;
        self.mode = None;
        if deinitialized && callbacks_detached {
            WIFI_INSTANCE_TAKEN.store(false, Ordering::Release);
        }
    }
}

impl<
        'd,
        const EVENTS: usize,
        const RX: usize,
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
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
{
    fn initialize(&mut self, config: &InitConfig) -> Result<(), InitError> {
        if self.initialized || WIFI_INSTANCE_TAKEN.swap(true, Ordering::AcqRel) {
            return Err(InitError::AlreadyInitialized);
        }

        let mut sdk_initialized = false;
        let result = (|| {
            // The closed Wi-Fi libraries create FreeRTOS tasks during init.
            unsafe { crate::wifi::compat::entry::init_compat_scheduler() };

            // Force the linker-provided PHY function table to be materialized
            // before the blob starts dereferencing it.
            extern "C" {
                static g_phyFuns: *const u32;
            }
            let phy_functions = unsafe { g_phyFuns };
            if !phy_functions.is_null() {
                let _ = unsafe { phy_functions.read_volatile() };
            }

            let event_loop_result = unsafe { esp8266_wifi_sys::esp_event_loop_create_default() };
            if event_loop_result != esp8266_wifi_sys::ESP_OK
                && event_loop_result != esp8266_wifi_sys::ESP_ERR_INVALID_STATE
            {
                return Err(InitError::BackendUnavailable);
            }

            unsafe { crate::wifi::compat::critical::mark_wdev_ready() };

            let raw = raw_init_config(config);
            let init_result = unsafe { esp8266_wifi_sys::esp_wifi_init(&raw) };
            if init_result != esp8266_wifi_sys::ESP_OK {
                return Err(map_esp_init_error(crate::wifi::EspError(init_result)));
            }
            sdk_initialized = true;

            #[cfg(feature = "wifi-smartconfig")]
            if unsafe { crate::wifi::compat::timer::start_ets_timer_task() } != 1 {
                return Err(InitError::BackendUnavailable);
            }

            crate::wifi::sdk::delay::enable_frc2_delay();
            crate::wifi::EspError::check(unsafe {
                esp8266_wifi_sys::esp_wifi_set_storage(esp8266_wifi_sys::WIFI_STORAGE_RAM)
            })
            .map_err(map_esp_init_error)?;

            let registration = CallbackRegistration::install(self.resources)?;
            self.registration = Some(registration);
            self.initialized = true;
            Ok(())
        })();

        if result.is_err() {
            crate::wifi::compat::port::set_wdev_tick_poll_enabled(false);
            let rollback_complete = !sdk_initialized
                || unsafe { esp8266_wifi_sys::esp_wifi_deinit() } == esp8266_wifi_sys::ESP_OK;
            if rollback_complete {
                unsafe { crate::wifi::compat::critical::clear_wdev_ready() };
                WIFI_INSTANCE_TAKEN.store(false, Ordering::Release);
            }
            self.registration = None;
        }
        result
    }

    fn set_mode(&mut self, mode: RawMode) -> Result<(), DriverError> {
        self.ensure_initialized()?;
        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_wifi_set_mode(raw_mode(mode))
        })
        .map_err(map_esp_driver_error)?;
        self.mode = Some(mode);
        Ok(())
    }

    fn start(&mut self) -> Result<(), DriverError> {
        self.ensure_initialized()?;
        let wait_for_sta_start = matches!(self.mode, Some(RawMode::Sta | RawMode::ApSta));
        let sta_start_seq = CORE_STA_START_SEQ.load(Ordering::Acquire);
        self.apply_station_power_save_for_mode()?;
        self.register_event_handler()?;
        CORE_STA_AUTO_RECONNECT.store(false, Ordering::Release);

        if let Err(error) =
            crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_wifi_start() })
                .map_err(map_esp_driver_error)
        {
            let _ = self.unregister_event_handler();
            return Err(error);
        }
        self.started = true;

        if wait_for_sta_start {
            let deadline = Deadline::new(self.now_ms(), STA_START_EVENT_TIMEOUT_MS);
            while CORE_STA_START_SEQ.load(Ordering::Acquire) == sta_start_seq {
                if deadline.is_expired(self.now_ms()) {
                    let _ = self.quiesce();
                    return Err(DriverError::StaStartTimeout);
                }
                self.spin_once();
            }
        }

        Ok(())
    }

    fn stop(&mut self) -> Result<(), DriverError> {
        self.ensure_initialized()?;
        self.quiesce()
    }
}

impl<
        'd,
        const EVENTS: usize,
        const RX: usize,
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
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
{
    fn configure_sta(&mut self, config: &ClientConfig<'_>) -> Result<(), DriverError> {
        self.station_power_save = config.power_save();
        let mode = if self.started && self.mode == Some(RawMode::ApSta) {
            RawMode::ApSta
        } else {
            RawMode::Sta
        };
        self.set_mode(mode)?;
        let mut raw = esp8266_wifi_sys::wifi_config_t {
            sta: raw_client_config(config),
        };
        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_wifi_set_config(esp8266_wifi_sys::WIFI_IF_STA, &mut raw)
        })
        .map_err(map_esp_driver_error)?;
        #[cfg(feature = "wifi-enterprise")]
        self.configure_wpa2_enterprise_for_station(config)?;
        Ok(())
    }

    fn connect(&mut self) -> Result<(), ConnectError> {
        self.ensure_initialized()
            .map_err(|_| ConnectError::BackendRejected)?;
        CORE_STA_AUTO_RECONNECT.store(true, Ordering::Release);
        let result = crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_wifi_connect() })
            .map_err(map_esp_connect_error);
        if result.is_err() {
            CORE_STA_AUTO_RECONNECT.store(false, Ordering::Release);
        }
        result
    }

    fn disconnect(&mut self) -> Result<(), DisconnectError> {
        self.ensure_initialized()
            .map_err(|_| DisconnectError::BackendRejected)?;
        CORE_STA_AUTO_RECONNECT.store(false, Ordering::Release);
        crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_wifi_disconnect() })
            .map_err(map_esp_disconnect_error)?;
        let _ = self
            .resources
            .publish_event(WifiEvent::StaDisconnected { reason: None });
        Ok(())
    }

    fn sta_mac_address(&self) -> Result<[u8; 6], DriverError> {
        self.mac_address(esp8266_wifi_sys::WIFI_IF_STA)
    }

    fn associated_bssid(&self) -> Result<[u8; 6], DriverError> {
        let mut record: esp8266_wifi_sys::wifi_ap_record_t = unsafe { core::mem::zeroed() };
        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_wifi_sta_get_ap_info(&mut record)
        })
        .map_err(map_esp_driver_error)?;
        Ok(record.bssid)
    }

    fn connect_blocking(&mut self, timeout_ms: u32) -> Option<Result<(), DriverError>> {
        Some(self.connect_blocking_inner(timeout_ms))
    }
}

impl<
        'd,
        const EVENTS: usize,
        const RX: usize,
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
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
{
    fn configure_ap(&mut self, config: &AccessPointConfig) -> Result<(), DriverError> {
        #[cfg(feature = "wifi-enterprise")]
        self.disable_wpa2_enterprise_if_enabled()?;
        self.set_mode(RawMode::Ap)?;
        let mut raw = esp8266_wifi_sys::wifi_config_t {
            ap: raw_ap_config(config),
        };
        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_wifi_set_config(esp8266_wifi_sys::WIFI_IF_AP, &mut raw)
        })
        .map_err(map_esp_driver_error)
    }

    fn configure_mixed(
        &mut self,
        client: &ClientConfig<'_>,
        access_point: &AccessPointConfig,
    ) -> Result<(), DriverError> {
        self.station_power_save = client.power_save();
        self.set_mode(RawMode::ApSta)?;
        let mut sta = esp8266_wifi_sys::wifi_config_t {
            sta: raw_client_config(client),
        };
        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_wifi_set_config(esp8266_wifi_sys::WIFI_IF_STA, &mut sta)
        })
        .map_err(map_esp_driver_error)?;
        let mut ap = esp8266_wifi_sys::wifi_config_t {
            ap: raw_ap_config(access_point),
        };
        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_wifi_set_config(esp8266_wifi_sys::WIFI_IF_AP, &mut ap)
        })
        .map_err(map_esp_driver_error)?;
        #[cfg(feature = "wifi-enterprise")]
        self.configure_wpa2_enterprise_for_station(client)?;
        Ok(())
    }

    fn ap_mac_address(&self) -> Result<[u8; 6], DriverError> {
        self.mac_address(esp8266_wifi_sys::WIFI_IF_AP)
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

    fn deauth_all_ap_stations(&mut self) -> Result<(), DriverError> {
        crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_wifi_deauth_sta(0) })
            .map_err(|_| DriverError::BackendRejected)
    }
}

impl<
        'd,
        const EVENTS: usize,
        const RX: usize,
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

        self.register_sta_rx_callback()?;
        self.sta_rx_registered = true;
        Ok(())
    }

    fn attach_ap_rx(&mut self) -> Result<(), DriverError> {
        if self.ap_rx_registered {
            return Ok(());
        }

        self.ensure_ap_netif_started()?;
        self.register_ap_rx_callback()?;
        self.ap_rx_registered = true;
        Ok(())
    }

    fn refresh_sta_rx(&mut self) -> Result<(), DriverError> {
        if self.sta_rx_registered {
            self.unregister_sta_rx_callback()?;
            self.sta_rx_registered = false;
        }
        self.register_sta_rx_callback()?;
        self.sta_rx_registered = true;
        Ok(())
    }

    fn refresh_ap_rx(&mut self) -> Result<(), DriverError> {
        self.ensure_ap_netif_started()?;
        if self.ap_rx_registered {
            self.unregister_ap_rx_callback()?;
            self.ap_rx_registered = false;
        }
        self.register_ap_rx_callback()?;
        self.ap_rx_registered = true;
        Ok(())
    }

    fn detach_sta_rx(&mut self) {
        if self.sta_rx_registered && self.unregister_sta_rx_callback().is_ok() {
            self.sta_rx_registered = false;
        }
    }

    fn detach_ap_rx(&mut self) {
        if self.ap_rx_registered && self.unregister_ap_rx_callback().is_ok() {
            self.ap_rx_registered = false;
        }
    }

    fn transmit(&mut self, frame: &[u8]) -> Result<(), DriverError> {
        super::validate_frame_len(frame.len())?;
        let ret = unsafe {
            esp8266_wifi_sys::esp_wifi_internal_tx(
                esp8266_wifi_sys::WIFI_IF_STA,
                frame.as_ptr() as *mut core::ffi::c_void,
                frame.len() as u16,
            )
        };

        map_internal_tx_result(ret)
    }

    fn transmit_ap(&mut self, frame: &[u8]) -> Result<(), DriverError> {
        super::validate_frame_len(frame.len())?;
        let ret = unsafe {
            esp8266_wifi_sys::esp_wifi_internal_tx(
                esp8266_wifi_sys::WIFI_IF_AP,
                frame.as_ptr() as *mut core::ffi::c_void,
                frame.len() as u16,
            )
        };

        map_internal_tx_result(ret)
    }
}

impl<
        'd,
        const EVENTS: usize,
        const RX: usize,
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
        if N == 0 {
            return Err(DriverError::PacketOverflow);
        }
        // The ESP8266 blocking scan can hang while the core event trampoline
        // is installed. Never hide a real link transition to work around that
        // SDK limitation: associated callers must disconnect explicitly.
        if self.resources.link_state() == super::LinkState::Up
            || CORE_STA_AUTO_RECONNECT.load(Ordering::Acquire)
        {
            return Err(DriverError::BackendRejected);
        }

        let restore_events = self.event_handler_registered;
        if restore_events {
            self.unregister_event_handler()?;
        }

        let scan_result = start_blocking_scan(options);
        let register_result = if restore_events {
            self.register_event_handler()
        } else {
            Ok(())
        };

        let records_result = match scan_result {
            Ok(()) => collect_scan_records_into(records),
            Err(error) => Err(error),
        };
        register_result?;
        records_result
    }
}

#[inline(never)]
fn start_blocking_scan(options: &ScanOptions) -> Result<(), DriverError> {
    clear_scan_prerequisites();
    let mut buffers = RawScanConfigBuffers::from_options(options);
    let raw_config = buffers.as_raw(options);
    crate::wifi::EspError::check(unsafe {
        esp8266_wifi_sys::esp_wifi_scan_start(&raw_config, true)
    })
    .map_err(map_esp_driver_error)?;
    Ok(())
}

fn clear_scan_prerequisites() {
    unsafe {
        let _ = esp8266_wifi_sys::esp_wifi_scan_stop();
        let _ = esp8266_wifi_sys::esp_wifi_clear_fast_connect();
    }
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
    if crate::wifi::EspError::check(unsafe {
        esp8266_wifi_sys::esp_wifi_scan_get_ap_num(&mut count)
    })
    .is_err()
    {
        discard_scan_records();
        return Err(DriverError::BackendRejected);
    }

    if count == 0 {
        discard_scan_records();
        return Ok(());
    }

    let fetch = (count as usize).min(N) as u16;
    let mut raw_records = match RawScanRecordBuffer::new(fetch as usize) {
        Ok(records) => records,
        Err(error) => {
            discard_scan_records();
            return Err(error);
        }
    };
    let mut actual = fetch;
    if crate::wifi::EspError::check(unsafe {
        esp8266_wifi_sys::esp_wifi_scan_get_ap_records(&mut actual, raw_records.as_mut_ptr())
    })
    .is_err()
    {
        discard_scan_records();
        return Err(DriverError::BackendRejected);
    }
    if actual > fetch {
        discard_scan_records();
        return Err(DriverError::BackendRejected);
    }

    for raw_record in unsafe { raw_records.as_slice(actual as usize) } {
        let ssid_len = raw_record
            .ssid
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(raw_record.ssid.len());
        let ssid = &raw_record.ssid[..ssid_len];
        let channel = match Channel::try_from(raw_record.primary) {
            Ok(channel) => channel,
            Err(_) => continue,
        };
        let Ok(record) = AccessPointRecord::new(
            ssid,
            raw_record.bssid,
            channel,
            raw_record.rssi,
            AuthenticationMethod::from_raw(raw_record.authmode as u8),
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

fn discard_scan_records() {
    let mut count = 1;
    let mut record: esp8266_wifi_sys::wifi_ap_record_t = unsafe { core::mem::zeroed() };
    let _ = unsafe { esp8266_wifi_sys::esp_wifi_scan_get_ap_records(&mut count, &mut record) };
}

impl<
        'd,
        const EVENTS: usize,
        const RX: usize,
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
        #[cfg(feature = "wifi-promiscuous")]
        if result.is_ok() {
            PROMISCUOUS_CHANNEL.store(channel.get() as u32, Ordering::Release);
        }
        result
    }

    #[cfg(feature = "wifi-promiscuous")]
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
        self.promiscuous_enabled = true;
        Ok(())
    }

    #[cfg(feature = "wifi-promiscuous")]
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

    #[cfg(feature = "wifi-promiscuous")]
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

        let result = clear_result.and(disable_result);
        if result.is_ok() {
            self.promiscuous_enabled = false;
        }
        result
    }

    #[cfg(feature = "wifi-esp-now")]
    fn enable_esp_now(&mut self) -> Result<(), DriverError> {
        if self.esp_now_initialized || self.esp_now_recv_registered || self.esp_now_send_registered
        {
            self.disable_esp_now()?;
        }
        crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_now_init() })
            .map_err(|_| DriverError::BackendRejected)?;
        self.esp_now_initialized = true;

        if ESP_NOW_RX > 0 {
            if crate::wifi::EspError::check(unsafe {
                esp8266_wifi_sys::esp_now_register_recv_cb(Some(
                    esp_now_recv_callback::<
                        EVENTS,
                        RX,
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
                let _ = self.disable_esp_now();
                return Err(DriverError::BackendRejected);
            }
            self.esp_now_recv_registered = true;
        }

        if ESP_NOW_PEERS > 0 {
            if crate::wifi::EspError::check(unsafe {
                esp8266_wifi_sys::esp_now_register_send_cb(Some(
                    esp_now_send_callback::<
                        EVENTS,
                        RX,
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
                let _ = self.disable_esp_now();
                return Err(DriverError::BackendRejected);
            }
            self.esp_now_send_registered = true;
        }

        Ok(())
    }

    #[cfg(feature = "wifi-esp-now")]
    fn disable_esp_now(&mut self) -> Result<(), DriverError> {
        let mut result = Ok(());
        if self.esp_now_send_registered {
            let unregister_send_result = crate::wifi::EspError::check(unsafe {
                esp8266_wifi_sys::esp_now_unregister_send_cb()
            })
            .map_err(|_| DriverError::BackendRejected);
            if unregister_send_result.is_ok() {
                self.esp_now_send_registered = false;
            }
            result = result.and(unregister_send_result);
        }
        if self.esp_now_recv_registered {
            let unregister_recv_result = crate::wifi::EspError::check(unsafe {
                esp8266_wifi_sys::esp_now_unregister_recv_cb()
            })
            .map_err(|_| DriverError::BackendRejected);
            if unregister_recv_result.is_ok() {
                self.esp_now_recv_registered = false;
            }
            result = result.and(unregister_recv_result);
        }
        if self.esp_now_send_registered || self.esp_now_recv_registered {
            return Err(DriverError::BackendRejected);
        }
        if self.esp_now_initialized {
            let deinit_result =
                crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_now_deinit() })
                    .map_err(|_| DriverError::BackendRejected);
            if deinit_result.is_ok() {
                self.esp_now_initialized = false;
            }
            result = result.and(deinit_result);
        }
        result
    }

    #[cfg(feature = "wifi-esp-now")]
    fn set_esp_now_pmk(&mut self, pmk: &[u8; 16]) -> Result<(), DriverError> {
        crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_now_set_pmk(pmk.as_ptr()) })
            .map_err(|_| DriverError::BackendRejected)
    }

    #[cfg(feature = "wifi-esp-now")]
    fn add_esp_now_peer(&mut self, peer: &EspNowPeerConfig) -> Result<(), DriverError> {
        let raw = raw_esp_now_peer(peer);

        crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_now_add_peer(&raw) })
            .map_err(|_| DriverError::BackendRejected)
    }

    #[cfg(feature = "wifi-esp-now")]
    fn modify_esp_now_peer(&mut self, peer: &EspNowPeerConfig) -> Result<(), DriverError> {
        let raw = raw_esp_now_peer(peer);

        crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_now_mod_peer(&raw) })
            .map_err(|_| DriverError::BackendRejected)
    }

    #[cfg(feature = "wifi-esp-now")]
    fn remove_esp_now_peer(&mut self, peer_addr: &[u8; 6]) -> Result<(), DriverError> {
        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_now_del_peer(peer_addr.as_ptr())
        })
        .map_err(|_| DriverError::BackendRejected)
    }

    #[cfg(feature = "wifi-esp-now")]
    fn send_esp_now(&mut self, peer_addr: &[u8; 6], payload: &[u8]) -> Result<(), DriverError> {
        if payload.len() > super::ESP_NOW_MAX_PAYLOAD {
            return Err(DriverError::PayloadTooLong);
        }

        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_now_send(peer_addr.as_ptr(), payload.as_ptr(), payload.len())
        })
        .map_err(|_| DriverError::BackendRejected)
    }

    #[cfg(feature = "wifi-wps")]
    fn enable_wps(&mut self, config: &WpsConfig<'_>) -> Result<(), DriverError> {
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
            .map_err(|error| DriverError::BackendError(error.code()))?;
        self.wps_enabled = true;
        Ok(())
    }

    #[cfg(feature = "wifi-wps")]
    fn start_wps(&mut self) -> Result<(), DriverError> {
        crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_wifi_wps_start(0) })
            .map_err(|error| DriverError::BackendError(error.code()))
    }

    #[cfg(feature = "wifi-wps")]
    fn disable_wps(&mut self) -> Result<(), DriverError> {
        let result =
            crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_wifi_wps_disable() })
                .map_err(|error| DriverError::BackendError(error.code()));
        unsafe {
            esp8266_hal_wps_set_pin(core::ptr::null());
            esp8266_hal_wps_set_target_ssid(core::ptr::null(), 0);
        }
        if result.is_ok() {
            self.wps_enabled = false;
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
            let _ = self.unregister_smartconfig_event_handler();
            return Err(DriverError::OutOfMemory);
        }
        let mut key = [0_i8; 17];
        if let Some(value) = config.esp_touch_v2_key_value() {
            for (dst, src) in key[..16].iter_mut().zip(value) {
                *dst = src as i8;
            }
        }
        let raw = esp8266_wifi_sys::smartconfig_start_config_t {
            // Hardware validation on this no-std ESP8266 port shows the
            // closed SmartConfig library can stall before SC_EVENT_SCAN_DONE
            // with its log path disabled. esp_log is stubbed here, so force
            // the reliable SDK path without adding UART output.
            enable_log: true,
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
            let _ = self.unregister_smartconfig_event_handler();
            return Err(DriverError::BackendRejected);
        }
        self.smartconfig_active = true;
        Ok(())
    }

    #[cfg(feature = "wifi-smartconfig")]
    fn stop_smartconfig(&mut self) -> Result<(), DriverError> {
        let stop_result =
            crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_smartconfig_stop() })
                .map_err(|_| DriverError::BackendRejected);
        crate::wifi::compat::port::set_wdev_tick_poll_enabled(false);
        if stop_result.is_ok() {
            self.smartconfig_active = false;
        }
        stop_result.and(self.unregister_smartconfig_event_handler())
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
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
{
    fn now_ms(&self) -> u64 {
        // The compat scheduler advances the FreeRTOS tick reliably while the
        // current task blocks/yields in SDK WiFi calls. `esp_timer_get_time()`
        // is not a dependable deadline source in those paths on this port.
        unsafe { crate::wifi::compat::task::get_tick_count().saturating_mul(10) }
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
    let resources = unsafe {
        super::target_callback::active_resources::<
            EVENTS,
            RX,
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
            Some(WifiEvent::StaStarted)
        }
        esp8266_wifi_sys::WIFI_EVENT_STA_STOP => Some(WifiEvent::StaStopped),
        esp8266_wifi_sys::WIFI_EVENT_STA_CONNECTED => Some(WifiEvent::StaConnected),
        esp8266_wifi_sys::WIFI_EVENT_STA_DISCONNECTED => {
            let reason = if event_data.is_null() {
                None
            } else {
                let disconnected = unsafe {
                    &*(event_data as *const esp8266_wifi_sys::wifi_event_sta_disconnected_t)
                };
                Some(disconnected.reason as u8)
            };

            Some(WifiEvent::StaDisconnected { reason })
        }
        #[cfg(feature = "wifi-wps")]
        esp8266_wifi_sys::WIFI_EVENT_STA_WPS_ER_SUCCESS => Some(WifiEvent::WpsSucceeded),
        #[cfg(feature = "wifi-wps")]
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
        #[cfg(feature = "wifi-wps")]
        esp8266_wifi_sys::WIFI_EVENT_STA_WPS_ER_TIMEOUT => Some(WifiEvent::WpsTimedOut),
        #[cfg(feature = "wifi-wps")]
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
        esp8266_wifi_sys::WIFI_EVENT_AP_START => Some(WifiEvent::ApStarted),
        esp8266_wifi_sys::WIFI_EVENT_AP_STOP => Some(WifiEvent::ApStopped),
        _ => None,
    };

    if let Some(event) = event {
        let _ = resources.publish_event(event);
        if matches!(event, WifiEvent::StaDisconnected { .. })
            && CORE_STA_AUTO_RECONNECT.load(Ordering::Acquire)
        {
            let _ = unsafe { esp8266_wifi_sys::esp_wifi_connect() };
        }
    }
}

#[cfg(feature = "wifi-smartconfig")]
unsafe extern "C" fn smartconfig_event_trampoline<
    const EVENTS: usize,
    const RX: usize,
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

    let resources = unsafe {
        super::target_callback::active_resources::<
            EVENTS,
            RX,
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
                let credentials = SmartConfigCredentials::new(
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
                );
                match credentials {
                    Ok(credentials) => {
                        let _ = resources.publish_smartconfig_credentials(credentials);
                        None
                    }
                    Err(_) => None,
                }
            }
        }
        _ => None,
    };

    if let Some(event) = event {
        let _ = resources.publish_event(event);
    }
}

unsafe extern "C" fn rx_callback_sta<
    const EVENTS: usize,
    const RX: usize,
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
    if len != 0 && buffer.is_null() {
        unsafe { release_rx_ownership(buffer, eb) };
        return esp8266_wifi_sys::ESP_ERR_INVALID_ARG;
    }

    let resources = unsafe {
        super::target_callback::active_resources::<
            EVENTS,
            RX,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >()
    };
    if resources.is_null() {
        unsafe { release_rx_ownership(buffer, eb) };
        return esp8266_wifi_sys::ESP_OK;
    }

    let resources = unsafe { &*resources };
    let record = unsafe { RawRxRecord::from_raw(buffer, len, eb, Some(release_rx_ownership)) };
    let _ = resources.enqueue_rx(record);
    esp8266_wifi_sys::ESP_OK
}

unsafe extern "C" fn rx_callback_ap<
    const EVENTS: usize,
    const RX: usize,
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
    if len != 0 && buffer.is_null() {
        unsafe { release_rx_ownership(buffer, eb) };
        return esp8266_wifi_sys::ESP_ERR_INVALID_ARG;
    }

    let resources = unsafe {
        super::target_callback::active_resources::<
            EVENTS,
            RX,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >()
    };
    if resources.is_null() {
        unsafe { release_rx_ownership(buffer, eb) };
        return esp8266_wifi_sys::ESP_OK;
    }

    let resources = unsafe { &*resources };
    let frame = if len == 0 {
        &[]
    } else {
        unsafe { core::slice::from_raw_parts(buffer.cast::<u8>(), len as usize) }
    };
    let enqueue = resources.enqueue_ap_rx_copy(frame);
    unsafe { release_rx_ownership(buffer, eb) };
    let _ = enqueue;
    esp8266_wifi_sys::ESP_OK
}

#[cfg(feature = "wifi-promiscuous")]
unsafe extern "C" fn promiscuous_callback<
    const EVENTS: usize,
    const RX: usize,
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
    let original_len = if packet.rx_ctrl.sig_mode() != 0 {
        packet.rx_ctrl.ht_length() as usize
    } else {
        packet.rx_ctrl.legacy_length() as usize
    };
    let payload_len = match packet_type {
        PromiscuousPacketType::Misc => 0,
        PromiscuousPacketType::Management
        | PromiscuousPacketType::Control
        | PromiscuousPacketType::Data => original_len.min(PROMISCUOUS_MAX_PAYLOAD_PREFIX),
    };

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
        original_len: original_len.min(u16::MAX as usize) as u16,
    };

    if let Ok(frame) = PromiscuousRxFrame::new(info, payload) {
        let _ = unsafe { &*resources }.enqueue_promiscuous_rx(frame);
    }
}

#[cfg(feature = "wifi-esp-now")]
unsafe extern "C" fn esp_now_recv_callback<
    const EVENTS: usize,
    const RX: usize,
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
    if peer_addr.is_null()
        || payload_len < 0
        || payload_len as usize > super::ESP_NOW_MAX_PAYLOAD
        || (payload_len > 0 && payload.is_null())
    {
        return;
    }

    let resources = unsafe {
        super::target_callback::active_resources::<
            EVENTS,
            RX,
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

#[cfg(feature = "wifi-esp-now")]
unsafe extern "C" fn esp_now_send_callback<
    const EVENTS: usize,
    const RX: usize,
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

    let status = match status {
        esp8266_wifi_sys::ESP_NOW_SEND_SUCCESS => EspNowSendStatus::Success,
        _ => EspNowSendStatus::Failed,
    };
    let _ = unsafe { &*resources }.complete_esp_now_tx(status);
}

fn map_internal_tx_result(result: i32) -> Result<(), DriverError> {
    match result {
        0 => Ok(()),
        -1 => Err(DriverError::WouldBlock),
        esp8266_wifi_sys::ESP_ERR_NO_MEM => Err(DriverError::OutOfMemory),
        _ => Err(DriverError::BackendRejected),
    }
}

fn map_esp_init_error(error: crate::wifi::EspError) -> InitError {
    if error.code() == esp8266_wifi_sys::ESP_ERR_INVALID_STATE {
        InitError::AlreadyInitialized
    } else {
        InitError::BackendUnavailable
    }
}

fn map_esp_driver_error(error: crate::wifi::EspError) -> DriverError {
    match error.code() {
        esp8266_wifi_sys::ESP_ERR_NO_MEM => DriverError::OutOfMemory,
        esp8266_wifi_sys::ESP_ERR_TIMEOUT | esp8266_wifi_sys::ESP_ERR_WIFI_TIMEOUT => {
            DriverError::Timeout
        }
        esp8266_wifi_sys::ESP_ERR_WIFI_NOT_STARTED => DriverError::NotStarted,
        esp8266_wifi_sys::ESP_ERR_WIFI_WOULD_BLOCK => DriverError::WouldBlock,
        code => DriverError::BackendError(code),
    }
}

#[cfg(feature = "wifi-enterprise")]
fn require_enterprise_material(
    bytes: &[u8],
    format: Wpa2EnterpriseMaterialFormat,
) -> Result<(), DriverError> {
    match format {
        Wpa2EnterpriseMaterialFormat::Pem => require_enterprise_pem(bytes),
        Wpa2EnterpriseMaterialFormat::Der => require_enterprise_der(bytes),
    }
}

#[cfg(feature = "wifi-enterprise")]
fn require_enterprise_pem(bytes: &[u8]) -> Result<(), DriverError> {
    if bytes.last() == Some(&0) {
        Ok(())
    } else {
        Err(DriverError::BackendRejected)
    }
}

#[cfg(feature = "wifi-enterprise")]
fn require_enterprise_der(bytes: &[u8]) -> Result<(), DriverError> {
    if bytes.first() == Some(&0x30) {
        Ok(())
    } else {
        Err(DriverError::BackendRejected)
    }
}

#[cfg(feature = "wifi-enterprise")]
fn map_enterprise_esp_error(error: crate::wifi::EspError) -> DriverError {
    if error.code() == esp8266_wifi_sys::ESP_ERR_NO_MEM {
        DriverError::OutOfMemory
    } else {
        DriverError::BackendError(error.code())
    }
}

fn map_esp_connect_error(error: crate::wifi::EspError) -> ConnectError {
    match error.code() {
        esp8266_wifi_sys::ESP_ERR_TIMEOUT | esp8266_wifi_sys::ESP_ERR_WIFI_TIMEOUT => {
            ConnectError::Timeout
        }
        esp8266_wifi_sys::ESP_ERR_WIFI_NOT_CONNECT => ConnectError::Disconnected,
        _ => ConnectError::BackendRejected,
    }
}

fn map_esp_disconnect_error(_error: crate::wifi::EspError) -> DisconnectError {
    DisconnectError::BackendRejected
}

impl<
        'd,
        const EVENTS: usize,
        const RX: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    >
    WifiDriver<
        'd,
        Uninitialized,
        Esp8266TargetBackend<
            'd,
            EVENTS,
            RX,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
{
    /// Creates the ESP8266 backend and binds it to the driver's resource pool.
    pub fn new(
        resources: &'d WifiResources<
            EVENTS,
            RX,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
    ) -> Self {
        Self::with_backend(resources, Esp8266TargetBackend::new(resources))
    }
}
