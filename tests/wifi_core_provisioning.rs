#![cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps"))]

#[cfg(feature = "wifi-wps")]
use esp8266_hal::wifi_core::WpsConfig;
use esp8266_hal::wifi_core::{
    DriverError, InitConfig, RawMode, WifiApBackend, WifiDriver, WifiFeatureBackend,
    WifiLifecycleBackend, WifiPacketBackend, WifiResources, WifiRuntimeBackend, WifiScanBackend,
    WifiStaBackend,
};
#[cfg(feature = "wifi-smartconfig")]
use esp8266_hal::wifi_core::{
    Ipv4Addr, SmartConfigCredentials, SmartConfigStartConfig, SmartConfigTimeout, SmartConfigType,
};

#[derive(Default)]
struct FakeProvisioningBackend {
    #[cfg(feature = "wifi-wps")]
    wps_enabled: bool,
    #[cfg(feature = "wifi-wps")]
    wps_started: bool,
    #[cfg(feature = "wifi-wps")]
    wps_disabled: bool,
    #[cfg(feature = "wifi-smartconfig")]
    smartconfig_started: bool,
    #[cfg(feature = "wifi-smartconfig")]
    smartconfig_stopped: bool,
    #[cfg(feature = "wifi-smartconfig")]
    smartconfig_type: Option<SmartConfigType>,
    #[cfg(feature = "wifi-smartconfig")]
    smartconfig_timeout: Option<SmartConfigTimeout>,
    #[cfg(feature = "wifi-smartconfig")]
    smartconfig_fast: bool,
}

impl WifiRuntimeBackend for FakeProvisioningBackend {
    fn now_ms(&self) -> u64 {
        0
    }

    fn spin_once(&mut self) {}
}

impl WifiLifecycleBackend for FakeProvisioningBackend {
    fn initialize(
        &mut self,
        _config: &InitConfig,
    ) -> Result<(), esp8266_hal::wifi_core::InitError> {
        Ok(())
    }

    fn set_mode(&mut self, _mode: RawMode) -> Result<(), DriverError> {
        Ok(())
    }

    fn start(&mut self) -> Result<(), DriverError> {
        Ok(())
    }

    fn stop(&mut self) -> Result<(), DriverError> {
        Ok(())
    }
}

impl WifiFeatureBackend for FakeProvisioningBackend {
    #[cfg(feature = "wifi-wps")]
    fn enable_wps(&mut self, _config: &WpsConfig<'_>) -> Result<(), DriverError> {
        self.wps_enabled = true;
        Ok(())
    }

    #[cfg(feature = "wifi-wps")]
    fn start_wps(&mut self) -> Result<(), DriverError> {
        self.wps_started = true;
        Ok(())
    }

    #[cfg(feature = "wifi-wps")]
    fn disable_wps(&mut self) -> Result<(), DriverError> {
        self.wps_disabled = true;
        Ok(())
    }

    #[cfg(feature = "wifi-smartconfig")]
    fn set_smartconfig_type(&mut self, kind: SmartConfigType) -> Result<(), DriverError> {
        self.smartconfig_type = Some(kind);
        Ok(())
    }

    #[cfg(feature = "wifi-smartconfig")]
    fn set_smartconfig_timeout(&mut self, timeout: SmartConfigTimeout) -> Result<(), DriverError> {
        self.smartconfig_timeout = Some(timeout);
        Ok(())
    }

    #[cfg(feature = "wifi-smartconfig")]
    fn set_smartconfig_fast_mode(&mut self, enabled: bool) -> Result<(), DriverError> {
        self.smartconfig_fast = enabled;
        Ok(())
    }

    #[cfg(feature = "wifi-smartconfig")]
    fn start_smartconfig(&mut self, _config: &SmartConfigStartConfig) -> Result<(), DriverError> {
        self.smartconfig_started = true;
        Ok(())
    }

    #[cfg(feature = "wifi-smartconfig")]
    fn stop_smartconfig(&mut self) -> Result<(), DriverError> {
        self.smartconfig_stopped = true;
        Ok(())
    }
}

impl WifiStaBackend for FakeProvisioningBackend {}
impl WifiApBackend for FakeProvisioningBackend {}
impl WifiPacketBackend for FakeProvisioningBackend {}
impl WifiScanBackend for FakeProvisioningBackend {}

#[test]
#[cfg(feature = "wifi-wps")]
fn wps_session_owns_lifecycle() {
    let resources = WifiResources::<8, 1>::new();
    let mut driver = WifiDriver::with_backend(&resources, FakeProvisioningBackend::default())
        .initialize(InitConfig::default())
        .unwrap()
        .enable_sta()
        .unwrap()
        .start()
        .unwrap();

    let mut session = driver.wps().unwrap();
    session.start(&WpsConfig::pbc()).unwrap();
    assert!(session.is_active());

    session.stop().unwrap();
    session.start(&WpsConfig::pbc()).unwrap();

    session.stop().unwrap();
    assert!(!session.is_active());
}

#[test]
#[cfg(feature = "wifi-smartconfig")]
fn smartconfig_session_owns_lifecycle() {
    let resources = WifiResources::<8, 1>::new();
    let mut driver = WifiDriver::with_backend(&resources, FakeProvisioningBackend::default())
        .initialize(InitConfig::default())
        .unwrap()
        .enable_sta()
        .unwrap()
        .start()
        .unwrap();

    let mut session = driver.smart_config().unwrap();
    session.set_type(SmartConfigType::EspTouch).unwrap();
    session
        .set_timeout(SmartConfigTimeout::try_from(15).unwrap())
        .unwrap();
    session.set_fast_mode(true).unwrap();
    session.start(&SmartConfigStartConfig::default()).unwrap();

    session.stop().unwrap();
    session.start(&SmartConfigStartConfig::default()).unwrap();
    session.stop().unwrap();
}

#[test]
#[cfg(feature = "wifi-smartconfig")]
fn smartconfig_credentials_convert_to_client_config() {
    let credentials = SmartConfigCredentials::new(
        b"network",
        b"password123",
        None,
        SmartConfigType::EspTouch,
        0,
        Ipv4Addr::UNSPECIFIED,
    )
    .unwrap();

    let client = credentials.client_config().unwrap();
    assert_eq!(client.ssid().as_bytes(), b"network");
    assert!(client.credential().passphrase().is_some());

    let open = SmartConfigCredentials::new(
        b"open",
        b"",
        None,
        SmartConfigType::AirKiss,
        0,
        Ipv4Addr::UNSPECIFIED,
    )
    .unwrap();
    let client = open.client_config().unwrap();
    assert_eq!(client.ssid().as_bytes(), b"open");
    assert!(client.credential().is_open());
}

#[test]
#[cfg(feature = "wifi-smartconfig")]
fn smartconfig_credentials_reject_invalid_network_configuration() {
    assert!(SmartConfigCredentials::new(
        b"",
        b"",
        None,
        SmartConfigType::EspTouch,
        0,
        Ipv4Addr::UNSPECIFIED,
    )
    .is_err());
    assert!(SmartConfigCredentials::new(
        b"network",
        b"short",
        None,
        SmartConfigType::EspTouch,
        0,
        Ipv4Addr::UNSPECIFIED,
    )
    .is_err());
}
