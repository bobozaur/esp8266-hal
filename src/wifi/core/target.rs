#[cfg(any(
    feature = "wifi-promiscuous",
    feature = "wifi-wps",
    feature = "wifi-smartconfig"
))]
use portable_atomic::AtomicU32;
use portable_atomic::{AtomicBool, Ordering};

#[cfg(feature = "wifi-promiscuous")]
use super::feature::PROMISCUOUS_MAX_PAYLOAD_PREFIX;
use super::target_map::*;
#[cfg(feature = "wifi-wps")]
use super::WpsConfig;
use super::{
    resources::{InterfaceId, Resources},
    target_callback::CallbackRegistration,
};
use super::{
    AccessPointConfig, AccessPointRecord, ApStationInfo, AuthenticationMethod, Channel,
    ClientConfig, DriverError, InitConfig, InitError, RawMode, RawRxRecord, ScanOptions,
    StationPowerSave,
};
#[cfg(feature = "wifi-esp-now")]
use super::{EspNowPeerConfig, EspNowRxFrame, EspNowSendStatus};
#[cfg(feature = "wifi-promiscuous")]
use super::{
    PromiscuousControlFilter, PromiscuousFilter, PromiscuousFrameInfo, PromiscuousPacketType,
    PromiscuousRxFrame,
};
#[cfg(feature = "wifi-smartconfig")]
use super::{SmartConfigStartConfig, SmartConfigTimeout, SmartConfigType};
#[cfg(feature = "wifi-enterprise")]
use super::{Wpa2EnterpriseCredentials, Wpa2EnterpriseMaterialFormat};

#[cfg(feature = "wifi-wps")]
extern "C" {
    pub(crate) fn esp8266_hal_wps_set_pin(pin: *const u8);
    pub(crate) fn esp8266_hal_wps_set_target_ssid(ssid: *const u8, len: u32);
}

#[cfg(feature = "wifi-promiscuous")]
static PROMISCUOUS_FILTER_BITS: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "wifi-promiscuous")]
static PROMISCUOUS_CHANNEL: AtomicU32 = AtomicU32::new(0);

static WIFI_INSTANCE_TAKEN: AtomicBool = AtomicBool::new(false);

pub(crate) struct Sdk {
    initialized: bool,
    owns_instance: bool,
    pub(crate) session_claimed: bool,
    pub(crate) started: bool,
    pub(crate) mode: Option<RawMode>,
    resources: *const Resources,
    registration: Option<CallbackRegistration>,
    sta_rx_registered: bool,
    ap_rx_registered: bool,
    #[cfg(feature = "wifi-enterprise")]
    wpa2_enterprise_configured: bool,
    #[cfg(feature = "wifi-promiscuous")]
    pub(crate) promiscuous_enabled: bool,
    #[cfg(feature = "wifi-esp-now")]
    pub(crate) esp_now_initialized: bool,
    #[cfg(feature = "wifi-esp-now")]
    esp_now_recv_registered: bool,
    #[cfg(feature = "wifi-esp-now")]
    esp_now_send_registered: bool,
    #[cfg(feature = "wifi-wps")]
    pub(crate) wps_enabled: bool,
    #[cfg(feature = "wifi-smartconfig")]
    pub(crate) smartconfig_active: bool,
    station_power_save: StationPowerSave,
}

impl Sdk {
    fn resources(&self) -> &Resources {
        // SAFETY: Wifi retains stable owned storage until shutdown completes;
        // failed teardown leaks that storage and quarantines the singleton.
        unsafe { &*self.resources }
    }

    pub(crate) fn new(resources: &Resources) -> Self {
        Self {
            initialized: false,
            owns_instance: false,
            session_claimed: false,
            started: false,
            mode: None,
            resources,
            registration: None,
            sta_rx_registered: false,
            ap_rx_registered: false,
            #[cfg(feature = "wifi-enterprise")]
            wpa2_enterprise_configured: false,
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

    pub(crate) fn ensure_initialized(&self) -> Result<(), DriverError> {
        if self.initialized {
            Ok(())
        } else {
            Err(DriverError::InvalidState)
        }
    }

    pub(crate) fn apply_station_power_save_for_mode(&mut self) -> Result<(), DriverError> {
        self.ensure_initialized()?;
        if matches!(self.mode, Some(RawMode::Sta | RawMode::ApSta)) {
            let power_save = raw_station_power_save(self.station_power_save);
            crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_wifi_set_ps(power_save) })
                .map_err(map_esp_driver_error)?;
        }
        Ok(())
    }

    #[cfg(feature = "wifi-enterprise")]
    pub(crate) fn configure_wpa2_enterprise_for_station(
        &mut self,
        config: &ClientConfig<'_>,
    ) -> Result<(), DriverError> {
        if let Some(credentials) = config.credential().enterprise_credentials() {
            self.enable_wpa2_enterprise(credentials)
        } else {
            Ok(())
        }
    }

    #[cfg(feature = "wifi-enterprise")]
    pub(crate) fn enable_wpa2_enterprise(
        &mut self,
        credentials: &Wpa2EnterpriseCredentials<'_>,
    ) -> Result<(), DriverError> {
        // Mode selection has already cleared the previous credentials.
        // Material is static by the configuration contract; retain cleanup
        // responsibility even if native setup fails partway through.
        self.wpa2_enterprise_configured = true;
        if let Err(error) = self.enable_wpa2_enterprise_inner(credentials) {
            let _ = self.clear_wpa2_enterprise();
            return Err(error);
        }
        Ok(())
    }

    #[cfg(feature = "wifi-enterprise")]
    pub(crate) fn enable_wpa2_enterprise_inner(
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

        crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_wifi_sta_wpa2_ent_enable() })
            .map_err(map_enterprise_esp_error)?;

        Ok(())
    }

    #[cfg(feature = "wifi-enterprise")]
    pub(crate) fn clear_wpa2_enterprise(&mut self) -> Result<(), DriverError> {
        if !self.wpa2_enterprise_configured {
            return Ok(());
        }

        crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_wifi_sta_wpa2_ent_disable() })
            .map_err(map_enterprise_esp_error)?;
        self.clear_wpa2_enterprise_credentials();
        self.wpa2_enterprise_configured = false;
        Ok(())
    }

    #[cfg(feature = "wifi-enterprise")]
    pub(crate) fn clear_wpa2_enterprise_credentials(&self) {
        unsafe {
            esp8266_wifi_sys::esp_wifi_sta_wpa2_ent_clear_identity();
            esp8266_wifi_sys::esp_wifi_sta_wpa2_ent_clear_username();
            esp8266_wifi_sys::esp_wifi_sta_wpa2_ent_clear_password();
            esp8266_wifi_sys::esp_wifi_sta_wpa2_ent_clear_new_password();
            esp8266_wifi_sys::esp_wifi_sta_wpa2_ent_clear_ca_cert();
            esp8266_wifi_sys::esp_wifi_sta_wpa2_ent_clear_cert_key();
        }
    }

    pub(crate) fn mac_address(
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

    #[inline(never)]
    pub(crate) fn register_sta_rx_callback(&mut self) -> Result<(), DriverError> {
        let ret = unsafe {
            esp8266_wifi_sys::esp_wifi_internal_reg_rxcb(
                esp8266_wifi_sys::WIFI_IF_STA,
                Some(rx_callback_sta),
            )
        };
        crate::wifi::EspError::check(ret).map_err(map_esp_driver_error)
    }

    pub(crate) fn register_ap_rx_callback(&mut self) -> Result<(), DriverError> {
        let ret = unsafe {
            esp8266_wifi_sys::esp_wifi_internal_reg_rxcb(
                esp8266_wifi_sys::WIFI_IF_AP,
                Some(rx_callback_ap),
            )
        };
        crate::wifi::EspError::check(ret).map_err(map_esp_driver_error)
    }

    pub(crate) fn unregister_sta_rx_callback(&mut self) -> Result<(), DriverError> {
        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_wifi_internal_reg_rxcb(esp8266_wifi_sys::WIFI_IF_STA, None)
        })
        .map_err(map_esp_driver_error)
    }

    pub(crate) fn unregister_ap_rx_callback(&mut self) -> Result<(), DriverError> {
        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_wifi_internal_reg_rxcb(esp8266_wifi_sys::WIFI_IF_AP, None)
        })
        .map_err(map_esp_driver_error)
    }

    pub(crate) fn quiesce(&mut self) -> Result<(), DriverError> {
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

        let stop_result = if self.started {
            crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_wifi_stop() })
                .map_err(map_esp_driver_error)
        } else {
            Ok(())
        };
        crate::wifi::compat::port::set_wdev_tick_poll_enabled(false);
        if stop_result.is_ok() {
            self.started = false;
            #[cfg(feature = "wifi-enterprise")]
            if self.wpa2_enterprise_configured {
                // Stop the EAP worker and release handshake state, retaining
                // credential inputs so ordinary start can enable it again.
                result = result.and(
                    crate::wifi::EspError::check(unsafe {
                        esp8266_wifi_sys::esp_wifi_sta_wpa2_ent_disable()
                    })
                    .map_err(map_enterprise_esp_error),
                );
            }
            self.detach_ap_rx();
            self.detach_sta_rx();
            if self.ap_rx_registered || self.sta_rx_registered {
                result = result.and(Err(DriverError::InvalidState));
            }
            #[cfg(feature = "wifi-smartconfig")]
            {
                super::ingress::set_smartconfig_enabled(false)?;
            }
            super::ingress::set_wifi_enabled(false)?;
            discard_scan_records();
            self.resources().reset_runtime();
        }

        result.and(stop_result)
    }
}

impl Sdk {
    pub(crate) fn shutdown(&mut self) -> bool {
        if !self.initialized {
            return !self.owns_instance;
        }

        // Close callback resource admission before teardown begins.
        // If any SDK callback cannot be unregistered, the singleton remains
        // quarantined so a later resource layout can never be observed by a
        // stale trampoline.
        self.registration = None;
        if self.quiesce().is_err() {
            return false;
        }
        #[cfg(feature = "wifi-enterprise")]
        if self.clear_wpa2_enterprise().is_err() {
            return false;
        }
        crate::wifi::compat::port::set_wdev_tick_poll_enabled(false);
        let deinitialized =
            crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_wifi_deinit() }).is_ok();

        self.started = false;
        self.detach_ap_rx();
        self.detach_sta_rx();
        self.resources().reset_runtime();

        let callbacks_detached = !self.sta_rx_registered && !self.ap_rx_registered;
        #[cfg(feature = "wifi-smartconfig")]
        let callbacks_detached = callbacks_detached && !self.smartconfig_active;
        #[cfg(feature = "wifi-promiscuous")]
        let callbacks_detached = callbacks_detached && !self.promiscuous_enabled;
        #[cfg(feature = "wifi-esp-now")]
        let callbacks_detached = callbacks_detached
            && !self.esp_now_initialized
            && !self.esp_now_recv_registered
            && !self.esp_now_send_registered;
        #[cfg(feature = "wifi-wps")]
        let callbacks_detached = callbacks_detached && !self.wps_enabled;
        #[cfg(feature = "wifi-enterprise")]
        let callbacks_detached = callbacks_detached && !self.wpa2_enterprise_configured;

        if deinitialized {
            unsafe { crate::wifi::compat::critical::clear_wdev_ready() };
        }
        self.initialized = false;
        self.mode = None;
        let complete = deinitialized && callbacks_detached && CallbackRegistration::quiescent();
        if complete {
            WIFI_INSTANCE_TAKEN.store(false, Ordering::Release);
            self.owns_instance = false;
        }
        complete
    }
}

impl Sdk {
    pub(crate) fn initialize(&mut self, config: &InitConfig) -> Result<(), InitError> {
        if self.initialized || WIFI_INSTANCE_TAKEN.swap(true, Ordering::AcqRel) {
            return Err(InitError::AlreadyInitialized);
        }

        self.owns_instance = true;
        let mut sdk_initialized = false;
        let result = (|| {
            let raw = raw_init_config(config);
            if !crate::wifi::sdk::wifi_init::buffer_configuration_matches(&raw) {
                return Err(InitError::BufferConfigurationLocked);
            }
            // The closed Wi-Fi libraries create FreeRTOS tasks during init.

            // Force the linker-provided PHY function table to be materialized
            // before the blob starts dereferencing it.
            extern "C" {
                static g_phyFuns: *const u32;
            }
            let phy_functions = unsafe { g_phyFuns };
            if !phy_functions.is_null() {
                let _ = unsafe { phy_functions.read_volatile() };
            }

            super::ingress::initialize()?;

            unsafe { crate::wifi::compat::critical::mark_wdev_ready() };

            let init_result = unsafe { esp8266_wifi_sys::esp_wifi_init(&raw) };
            if init_result != esp8266_wifi_sys::ESP_OK {
                return Err(map_esp_init_error(crate::wifi::EspError(init_result)));
            }
            sdk_initialized = true;

            crate::wifi::EspError::check(unsafe {
                esp8266_wifi_sys::esp_wifi_set_storage(esp8266_wifi_sys::WIFI_STORAGE_RAM)
            })
            .map_err(map_esp_init_error)?;

            let registration = CallbackRegistration::install(self.resources())?;
            self.registration = Some(registration);
            self.initialized = true;
            Ok(())
        })();

        if result.is_err() {
            crate::wifi::compat::port::set_wdev_tick_poll_enabled(false);
            let rollback_complete = if sdk_initialized {
                (unsafe { esp8266_wifi_sys::esp_wifi_deinit() }) == esp8266_wifi_sys::ESP_OK
            } else {
                !crate::wifi::sdk::wifi_init::requires_quarantine()
            };
            if rollback_complete {
                unsafe { crate::wifi::compat::critical::clear_wdev_ready() };
                WIFI_INSTANCE_TAKEN.store(false, Ordering::Release);
                self.owns_instance = false;
            }
            self.registration = None;
        }
        result
    }

    pub(crate) fn set_mode(&mut self, mode: RawMode) -> Result<(), DriverError> {
        self.ensure_initialized()?;
        // Every configuration replaces the previous enterprise inputs, including
        // credential-free station mode used for scanning and provisioning.
        #[cfg(feature = "wifi-enterprise")]
        self.clear_wpa2_enterprise()?;
        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_wifi_set_mode(raw_mode(mode))
        })
        .map_err(map_esp_driver_error)?;
        self.mode = Some(mode);
        Ok(())
    }

    pub(crate) fn start(&mut self) -> Result<(), DriverError> {
        self.ensure_initialized()?;
        // Ordinary stop releases enterprise handshake state but retains its
        // credential inputs for the next start.
        #[cfg(feature = "wifi-enterprise")]
        if self.wpa2_enterprise_configured {
            crate::wifi::EspError::check(unsafe {
                esp8266_wifi_sys::esp_wifi_sta_wpa2_ent_enable()
            })
            .map_err(map_esp_driver_error)?;
        }
        self.apply_station_power_save_for_mode()?;
        super::ingress::set_wifi_enabled(true)?;

        if let Err(error) =
            crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_wifi_start() })
                .map_err(map_esp_driver_error)
        {
            let _ = super::ingress::set_wifi_enabled(false);
            return Err(error);
        }
        self.started = true;

        Ok(())
    }

    pub(crate) fn stop(&mut self) -> Result<(), DriverError> {
        self.ensure_initialized()?;
        self.quiesce()
    }
}

impl Sdk {
    pub(crate) fn configure_sta(&mut self, config: &ClientConfig<'_>) -> Result<(), DriverError> {
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

    pub(crate) fn connect(&mut self) -> Result<(), DriverError> {
        self.ensure_initialized()?;
        crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_wifi_connect() })
            .map_err(map_esp_driver_error)
    }
    pub(crate) fn disconnect(&mut self) -> Result<(), DriverError> {
        self.ensure_initialized()?;
        crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_wifi_disconnect() })
            .map_err(map_esp_driver_error)
    }

    pub(crate) fn sta_mac_address(&self) -> Result<[u8; 6], DriverError> {
        self.mac_address(esp8266_wifi_sys::WIFI_IF_STA)
    }
}

impl Sdk {
    pub(crate) fn configure_ap(&mut self, config: &AccessPointConfig) -> Result<(), DriverError> {
        self.set_mode(RawMode::Ap)?;
        let mut raw = esp8266_wifi_sys::wifi_config_t {
            ap: raw_ap_config(config),
        };
        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_wifi_set_config(esp8266_wifi_sys::WIFI_IF_AP, &mut raw)
        })
        .map_err(map_esp_driver_error)
    }

    pub(crate) fn configure_mixed(
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

    pub(crate) fn ap_mac_address(&self) -> Result<[u8; 6], DriverError> {
        self.mac_address(esp8266_wifi_sys::WIFI_IF_AP)
    }

    pub(crate) fn connected_ap_stations(
        &mut self,
        stations: &mut [ApStationInfo],
    ) -> Result<usize, DriverError> {
        let mut raw: esp8266_wifi_sys::wifi_sta_list_t = unsafe { core::mem::zeroed() };
        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_wifi_ap_get_sta_list(&mut raw)
        })
        .map_err(map_esp_driver_error)?;
        let count = (raw.num as usize)
            .min(esp8266_wifi_sys::ESP_WIFI_MAX_CONN_NUM)
            .min(stations.len());
        for (dst, station) in stations.iter_mut().zip(raw.sta.iter()).take(count) {
            *dst = ApStationInfo::new(
                station.mac,
                station.phy_11b() != 0,
                station.phy_11g() != 0,
                station.phy_11n() != 0,
            );
        }
        Ok(count)
    }

    pub(crate) fn deauth_all_ap_stations(&mut self) -> Result<(), DriverError> {
        crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_wifi_deauth_sta(0) })
            .map_err(map_esp_driver_error)
    }
}

impl Sdk {
    pub(crate) fn attach_sta_rx(&mut self) -> Result<(), DriverError> {
        if self.sta_rx_registered {
            return Ok(());
        }

        self.register_sta_rx_callback()?;
        self.sta_rx_registered = true;
        Ok(())
    }

    pub(crate) fn attach_ap_rx(&mut self) -> Result<(), DriverError> {
        if self.ap_rx_registered {
            return Ok(());
        }

        self.register_ap_rx_callback()?;
        self.ap_rx_registered = true;
        Ok(())
    }

    pub(crate) fn detach_sta_rx(&mut self) {
        if self.sta_rx_registered && self.unregister_sta_rx_callback().is_ok() {
            self.sta_rx_registered = false;
        }
    }

    pub(crate) fn detach_ap_rx(&mut self) {
        if self.ap_rx_registered && self.unregister_ap_rx_callback().is_ok() {
            self.ap_rx_registered = false;
        }
    }

    pub(crate) fn transmit(interface: InterfaceId, frame: &[u8]) -> Result<(), DriverError> {
        // Interface validates the frame and holds a submission lease through
        // this call, preventing native teardown while it copies the payload.
        let interface = match interface {
            InterfaceId::Station => esp8266_wifi_sys::WIFI_IF_STA,
            InterfaceId::AccessPoint => esp8266_wifi_sys::WIFI_IF_AP,
        };
        let ret = unsafe {
            esp8266_wifi_sys::esp_wifi_internal_tx(
                interface,
                frame.as_ptr() as *mut core::ffi::c_void,
                frame.len() as u16,
            )
        };

        map_internal_tx_result(ret)
    }
}

impl Sdk {
    pub(crate) fn begin_scan(&mut self, options: &ScanOptions) -> Result<(), DriverError> {
        self.ensure_initialized()?;
        let raw = self
            .resources()
            .scan_storage
            .with(|storage| storage.prepare(options));
        crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_wifi_scan_start(raw, false) })
            .map_err(map_esp_driver_error)
    }
    pub(crate) fn cancel_scan(&mut self) -> Result<(), DriverError> {
        crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_wifi_scan_stop() })
            .map_err(map_esp_driver_error)
    }
    pub(crate) fn collect_scan(
        &mut self,
        records: &mut [AccessPointRecord],
    ) -> Result<usize, DriverError> {
        collect_scan_records_into(records)
    }
    pub(crate) fn discard_scan(&mut self) {
        discard_scan_records()
    }
}

struct RawScanRecordBuffer {
    ptr: *mut esp8266_wifi_sys::wifi_ap_record_t,
}

impl RawScanRecordBuffer {
    pub(crate) fn new(len: usize) -> Result<Self, DriverError> {
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

    pub(crate) fn as_mut_ptr(&mut self) -> *mut esp8266_wifi_sys::wifi_ap_record_t {
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
fn collect_scan_records_into(records: &mut [AccessPointRecord]) -> Result<usize, DriverError> {
    let mut count: u16 = 0;
    if crate::wifi::EspError::check(unsafe {
        esp8266_wifi_sys::esp_wifi_scan_get_ap_num(&mut count)
    })
    .is_err()
    {
        discard_scan_records();
        return Err(DriverError::InvalidState);
    }

    if count == 0 {
        discard_scan_records();
        return Ok(0);
    }

    let fetch = (count as usize).min(records.len()) as u16;
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
        return Err(DriverError::InvalidState);
    }
    if actual > fetch {
        discard_scan_records();
        return Err(DriverError::InvalidState);
    }

    let mut written = 0;
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
        records[written] = record;
        written += 1;
    }

    Ok(written)
}

fn discard_scan_records() {
    let mut count = 1;
    let mut record: esp8266_wifi_sys::wifi_ap_record_t = unsafe { core::mem::zeroed() };
    let _ = unsafe { esp8266_wifi_sys::esp_wifi_scan_get_ap_records(&mut count, &mut record) };
}

impl Sdk {
    #[cfg(any(feature = "wifi-esp-now", feature = "wifi-promiscuous"))]
    pub(crate) fn set_channel(&mut self, channel: Channel) -> Result<(), DriverError> {
        let result = crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_wifi_set_channel(
                channel.get(),
                esp8266_wifi_sys::WIFI_SECOND_CHAN_NONE,
            )
        })
        .map_err(map_esp_driver_error);
        #[cfg(feature = "wifi-promiscuous")]
        if result.is_ok() {
            PROMISCUOUS_CHANNEL.store(channel.get() as u32, Ordering::Release);
        }
        result
    }

    #[cfg(feature = "wifi-promiscuous")]
    pub(crate) fn enable_promiscuous(
        &mut self,
        filter: PromiscuousFilter,
    ) -> Result<(), DriverError> {
        let raw_filter = esp8266_wifi_sys::wifi_promiscuous_filter_t {
            filter_mask: map_promiscuous_filter(filter),
        };

        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_wifi_set_promiscuous_filter(&raw_filter)
        })
        .map_err(map_esp_driver_error)?;
        PROMISCUOUS_FILTER_BITS.store(0, Ordering::Release);

        crate::wifi::compat::port::set_wdev_tick_poll_enabled(true);
        for _ in 0..4 {
            self.spin_once();
        }
        if let Err(error) = crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_wifi_set_promiscuous(true)
        })
        .map_err(map_esp_driver_error)
        {
            crate::wifi::compat::port::set_wdev_tick_poll_enabled(false);
            PROMISCUOUS_FILTER_BITS.store(0, Ordering::Release);
            return Err(error);
        }

        self.promiscuous_enabled = true;
        if let Err(error) = crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_wifi_set_promiscuous_rx_cb(Some(promiscuous_callback))
        })
        .map_err(map_esp_driver_error)
        {
            let _ = self.disable_promiscuous();
            crate::wifi::compat::port::set_wdev_tick_poll_enabled(false);
            PROMISCUOUS_FILTER_BITS.store(0, Ordering::Release);
            return Err(error);
        }

        PROMISCUOUS_FILTER_BITS.store(filter.bits() as u32, Ordering::Release);
        self.promiscuous_enabled = true;
        Ok(())
    }

    #[cfg(feature = "wifi-promiscuous")]
    pub(crate) fn set_promiscuous_control_filter(
        &mut self,
        filter: PromiscuousControlFilter,
    ) -> Result<(), DriverError> {
        let raw_filter = esp8266_wifi_sys::wifi_promiscuous_filter_t {
            filter_mask: map_promiscuous_control_filter(filter),
        };

        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_wifi_set_promiscuous_ctrl_filter(&raw_filter)
        })
        .map_err(map_esp_driver_error)
    }

    #[cfg(feature = "wifi-promiscuous")]
    pub(crate) fn disable_promiscuous(&mut self) -> Result<(), DriverError> {
        let clear_result = crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_wifi_set_promiscuous_rx_cb(None)
        })
        .map_err(map_esp_driver_error);
        PROMISCUOUS_FILTER_BITS.store(0, Ordering::Release);
        PROMISCUOUS_CHANNEL.store(0, Ordering::Release);
        let disable_result = crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_wifi_set_promiscuous(false)
        })
        .map_err(map_esp_driver_error);
        crate::wifi::compat::port::set_wdev_tick_poll_enabled(false);

        let result = clear_result.and(disable_result);
        if result.is_ok() {
            self.promiscuous_enabled = false;
        }
        result
    }

    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn enable_esp_now(&mut self) -> Result<(), DriverError> {
        if self.esp_now_initialized || self.esp_now_recv_registered || self.esp_now_send_registered
        {
            self.disable_esp_now()?;
        }
        crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_now_init() })
            .map_err(map_esp_driver_error)?;
        self.esp_now_initialized = true;

        if self.resources().esp_now_rx_capacity() > 0 {
            if crate::wifi::EspError::check(unsafe {
                esp8266_wifi_sys::esp_now_register_recv_cb(Some(esp_now_recv_callback))
            })
            .is_err()
            {
                let _ = self.disable_esp_now();
                return Err(DriverError::InvalidState);
            }
            self.esp_now_recv_registered = true;
        }

        if self.resources().esp_now_peer_capacity() > 0 {
            if crate::wifi::EspError::check(unsafe {
                esp8266_wifi_sys::esp_now_register_send_cb(Some(esp_now_send_callback))
            })
            .is_err()
            {
                let _ = self.disable_esp_now();
                return Err(DriverError::InvalidState);
            }
            self.esp_now_send_registered = true;
        }

        Ok(())
    }

    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn disable_esp_now(&mut self) -> Result<(), DriverError> {
        let mut result = Ok(());
        if self.esp_now_send_registered {
            let unregister_send_result = crate::wifi::EspError::check(unsafe {
                esp8266_wifi_sys::esp_now_unregister_send_cb()
            })
            .map_err(map_esp_driver_error);
            if unregister_send_result.is_ok() {
                self.esp_now_send_registered = false;
            }
            result = result.and(unregister_send_result);
        }
        if self.esp_now_recv_registered {
            let unregister_recv_result = crate::wifi::EspError::check(unsafe {
                esp8266_wifi_sys::esp_now_unregister_recv_cb()
            })
            .map_err(map_esp_driver_error);
            if unregister_recv_result.is_ok() {
                self.esp_now_recv_registered = false;
            }
            result = result.and(unregister_recv_result);
        }
        if self.esp_now_send_registered || self.esp_now_recv_registered {
            return Err(DriverError::InvalidState);
        }
        if self.esp_now_initialized {
            let deinit_result =
                crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_now_deinit() })
                    .map_err(map_esp_driver_error);
            if deinit_result.is_ok() {
                self.esp_now_initialized = false;
            }
            result = result.and(deinit_result);
        }
        result
    }

    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn set_esp_now_pmk(&mut self, pmk: &[u8; 16]) -> Result<(), DriverError> {
        crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_now_set_pmk(pmk.as_ptr()) })
            .map_err(map_esp_driver_error)
    }

    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn contains_esp_now_peer(&self, peer_addr: &[u8; 6]) -> bool {
        self.esp_now_initialized
            && unsafe { esp8266_wifi_sys::esp_now_is_peer_exist(peer_addr.as_ptr()) }
    }

    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn esp_now_peer_count(&self) -> Result<usize, DriverError> {
        if !self.esp_now_initialized {
            return Ok(0);
        }
        let mut count: esp8266_wifi_sys::esp_now_peer_num_t = unsafe { core::mem::zeroed() };
        crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_now_get_peer_num(&mut count) })
            .map_err(map_esp_driver_error)?;
        Ok(count.total_num as usize)
    }

    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn add_esp_now_peer(&mut self, peer: &EspNowPeerConfig) -> Result<(), DriverError> {
        let raw = raw_esp_now_peer(peer);

        crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_now_add_peer(&raw) })
            .map_err(map_esp_driver_error)
    }

    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn modify_esp_now_peer(
        &mut self,
        peer: &EspNowPeerConfig,
    ) -> Result<(), DriverError> {
        let raw = raw_esp_now_peer(peer);

        crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_now_mod_peer(&raw) })
            .map_err(map_esp_driver_error)
    }

    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn remove_esp_now_peer(&mut self, peer_addr: &[u8; 6]) -> Result<(), DriverError> {
        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_now_del_peer(peer_addr.as_ptr())
        })
        .map_err(map_esp_driver_error)
    }

    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn send_esp_now(
        &mut self,
        peer_addr: &[u8; 6],
        payload: &[u8],
    ) -> Result<(), DriverError> {
        if payload.len() > super::ESP_NOW_MAX_PAYLOAD {
            return Err(DriverError::PayloadTooLong);
        }

        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_now_send(peer_addr.as_ptr(), payload.as_ptr(), payload.len())
        })
        .map_err(map_esp_driver_error)
    }

    #[cfg(feature = "wifi-wps")]
    pub(crate) fn enable_wps(&mut self, config: &WpsConfig<'_>) -> Result<(), DriverError> {
        super::ingress::set_wifi_enabled(true)?;
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
            .map_err(|error| DriverError::Sdk(error.code()))?;
        self.wps_enabled = true;
        Ok(())
    }

    #[cfg(feature = "wifi-wps")]
    pub(crate) fn start_wps(&mut self) -> Result<(), DriverError> {
        crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_wifi_wps_start(0) })
            .map_err(|error| DriverError::Sdk(error.code()))
    }

    #[cfg(feature = "wifi-wps")]
    pub(crate) fn disable_wps(&mut self) -> Result<(), DriverError> {
        let result =
            crate::wifi::EspError::check(unsafe { esp8266_wifi_sys::esp_wifi_wps_disable() })
                .map_err(|error| DriverError::Sdk(error.code()));
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
    pub(crate) fn set_smartconfig_type(
        &mut self,
        kind: SmartConfigType,
    ) -> Result<(), DriverError> {
        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_smartconfig_set_type(map_smartconfig_type(kind))
        })
        .map_err(map_esp_driver_error)
    }

    #[cfg(feature = "wifi-smartconfig")]
    pub(crate) fn set_smartconfig_timeout(
        &mut self,
        timeout: SmartConfigTimeout,
    ) -> Result<(), DriverError> {
        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_esptouch_set_timeout(timeout.seconds())
        })
        .map_err(map_esp_driver_error)
    }

    #[cfg(feature = "wifi-smartconfig")]
    pub(crate) fn set_smartconfig_fast_mode(&mut self, enabled: bool) -> Result<(), DriverError> {
        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_smartconfig_fast_mode(enabled)
        })
        .map_err(map_esp_driver_error)
    }

    #[cfg(feature = "wifi-smartconfig")]
    pub(crate) fn start_smartconfig(
        &mut self,
        config: &SmartConfigStartConfig,
    ) -> Result<(), DriverError> {
        super::ingress::set_smartconfig_enabled(true)?;
        // Keep config and AES key live even if native code retains either pointer.
        let raw = self
            .resources()
            .smartconfig_storage
            .with(|storage| storage.prepare(config));

        self.smartconfig_active = true;
        if crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_smartconfig_internal_start(raw)
        })
        .is_err()
        {
            let _ = self.stop_smartconfig();
            return Err(DriverError::InvalidState);
        }
        self.smartconfig_active = true;
        Ok(())
    }

    #[cfg(feature = "wifi-smartconfig")]
    pub(crate) fn stop_smartconfig(&mut self) -> Result<(), DriverError> {
        let stop_result = crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_smartconfig_internal_stop()
        })
        .map_err(map_esp_driver_error);
        if stop_result.is_ok() {
            self.smartconfig_active = false;
        }
        stop_result.and(super::ingress::set_smartconfig_enabled(false))
    }

    #[cfg(feature = "wifi-smartconfig")]
    pub(crate) fn read_smartconfig_reserved_data(
        &mut self,
        buffer: &mut [u8],
    ) -> Result<(), DriverError> {
        if buffer.is_empty() || buffer.len() > u8::MAX as usize {
            return Err(DriverError::PayloadTooLong);
        }

        crate::wifi::EspError::check(unsafe {
            esp8266_wifi_sys::esp_smartconfig_get_rvd_data(buffer.as_mut_ptr(), buffer.len() as u8)
        })
        .map_err(map_esp_driver_error)
    }
}

impl Sdk {
    pub(crate) fn now_ms(&self) -> u64 {
        // The compat scheduler advances the FreeRTOS tick reliably while the
        // current task blocks/yields in SDK WiFi calls. `esp_timer_get_time()`
        // is not a dependable deadline source in those paths on this port.
        unsafe { crate::wifi::compat::task::get_tick_count().saturating_mul(10) }
    }

    pub(crate) fn spin_once(&mut self) {
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

unsafe extern "C" fn rx_callback_sta(
    buffer: *mut core::ffi::c_void,
    len: u16,
    eb: *mut core::ffi::c_void,
) -> esp8266_wifi_sys::esp_err_t {
    unsafe { rx_callback(InterfaceId::Station, buffer, len, eb) }
}

unsafe extern "C" fn rx_callback_ap(
    buffer: *mut core::ffi::c_void,
    len: u16,
    eb: *mut core::ffi::c_void,
) -> esp8266_wifi_sys::esp_err_t {
    unsafe { rx_callback(InterfaceId::AccessPoint, buffer, len, eb) }
}

unsafe fn rx_callback(
    interface: InterfaceId,
    buffer: *mut core::ffi::c_void,
    len: u16,
    eb: *mut core::ffi::c_void,
) -> esp8266_wifi_sys::esp_err_t {
    let invocation = super::target_callback::CallbackInvocation::enter();
    if len != 0 && buffer.is_null() {
        unsafe { release_rx_ownership(buffer, eb) };
        return esp8266_wifi_sys::ESP_ERR_INVALID_ARG;
    }

    let resources = match invocation.resources() {
        Some(resources) => resources,
        None => {
            unsafe { release_rx_ownership(buffer, eb) };
            return esp8266_wifi_sys::ESP_OK;
        }
    };
    // The pinned SDK uses the same deferred-release buffer contract for AP
    // and station callbacks. The record releases it after consumption or drop.
    let record = unsafe { RawRxRecord::from_raw(buffer, len, eb, release_rx_ownership) };
    let _ = resources.enqueue_rx(interface, record);
    esp8266_wifi_sys::ESP_OK
}

#[cfg(feature = "wifi-promiscuous")]
unsafe extern "C" fn promiscuous_callback(
    buffer: *mut core::ffi::c_void,
    packet_type: esp8266_wifi_sys::wifi_promiscuous_pkt_type_t,
) {
    if buffer.is_null() {
        return;
    }

    let invocation = super::target_callback::CallbackInvocation::enter();
    let Some(resources) = invocation.resources() else {
        return;
    };

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
        let _ = resources.enqueue_promiscuous_rx(frame);
    }
}

#[cfg(feature = "wifi-esp-now")]
unsafe extern "C" fn esp_now_recv_callback(
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

    let invocation = super::target_callback::CallbackInvocation::enter();
    let Some(resources) = invocation.resources() else {
        return;
    };

    let peer_addr = unsafe { *(peer_addr as *const [u8; 6]) };
    let payload = if payload_len == 0 {
        &[]
    } else {
        unsafe { core::slice::from_raw_parts(payload, payload_len as usize) }
    };
    if let Ok(frame) = EspNowRxFrame::new(peer_addr, payload) {
        let _ = resources.enqueue_esp_now_rx(frame);
    }
}

#[cfg(feature = "wifi-esp-now")]
unsafe extern "C" fn esp_now_send_callback(
    peer_addr: *const u8,
    status: esp8266_wifi_sys::esp_now_send_status_t,
) {
    if peer_addr.is_null() {
        return;
    }

    let invocation = super::target_callback::CallbackInvocation::enter();
    let Some(resources) = invocation.resources() else {
        return;
    };

    let status = match status {
        esp8266_wifi_sys::ESP_NOW_SEND_SUCCESS => EspNowSendStatus::Success,
        _ => EspNowSendStatus::Failed,
    };
    let _ = resources.complete_esp_now_tx(status);
}

fn map_esp_init_error(error: crate::wifi::EspError) -> InitError {
    if error.code() == esp8266_wifi_sys::ESP_ERR_INVALID_STATE {
        InitError::AlreadyInitialized
    } else {
        match error.code() {
            esp8266_wifi_sys::ESP_ERR_NO_MEM => InitError::OutOfMemory,
            code => InitError::Sdk(code),
        }
    }
}

pub(super) fn map_esp_driver_error(error: crate::wifi::EspError) -> DriverError {
    match error.code() {
        esp8266_wifi_sys::ESP_ERR_NO_MEM => DriverError::OutOfMemory,
        esp8266_wifi_sys::ESP_ERR_TIMEOUT | esp8266_wifi_sys::ESP_ERR_WIFI_TIMEOUT => {
            DriverError::Timeout
        }
        esp8266_wifi_sys::ESP_ERR_WIFI_NOT_STARTED => DriverError::NotStarted,
        esp8266_wifi_sys::ESP_ERR_WIFI_WOULD_BLOCK => DriverError::WouldBlock,
        code => DriverError::Sdk(code),
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
        Err(DriverError::InvalidState)
    }
}

#[cfg(feature = "wifi-enterprise")]
fn require_enterprise_der(bytes: &[u8]) -> Result<(), DriverError> {
    if bytes.first() == Some(&0x30) {
        Ok(())
    } else {
        Err(DriverError::InvalidState)
    }
}

#[cfg(feature = "wifi-enterprise")]
fn map_enterprise_esp_error(error: crate::wifi::EspError) -> DriverError {
    if error.code() == esp8266_wifi_sys::ESP_ERR_NO_MEM {
        DriverError::OutOfMemory
    } else {
        DriverError::Sdk(error.code())
    }
}

#[cfg(any(feature = "wifi-wps", feature = "wifi-smartconfig"))]
impl Sdk {
    pub(crate) fn synchronize_events(&mut self) -> Result<(), DriverError> {
        super::ingress::synchronize()
    }
}
