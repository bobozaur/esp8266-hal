use esp8266_hal::wifi_core::{
    Credential, DriverError, InitConfig, Ipv4Addr, RawMode, SmartConfigCredentials,
    SmartConfigStartConfig, SmartConfigTimeout, SmartConfigType, WifiApBackend, WifiDriver,
    WifiEvent, WifiFeatureBackend, WifiLifecycleBackend, WifiPacketBackend, WifiResources,
    WifiRuntimeBackend, WifiScanBackend, WifiStaBackend, WpsConfig,
};

#[derive(Default)]
struct FakeProvisioningBackend {
    wps_enabled: bool,
    wps_started: bool,
    wps_disabled: bool,
    smartconfig_started: bool,
    smartconfig_stopped: bool,
    smartconfig_type: Option<SmartConfigType>,
    smartconfig_timeout: Option<SmartConfigTimeout>,
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
    fn enable_wps(&mut self, _config: &WpsConfig) -> Result<(), DriverError> {
        self.wps_enabled = true;
        Ok(())
    }

    fn start_wps(&mut self, _timeout_ms: i32) -> Result<(), DriverError> {
        self.wps_started = true;
        Ok(())
    }

    fn disable_wps(&mut self) -> Result<(), DriverError> {
        self.wps_disabled = true;
        Ok(())
    }

    fn set_smartconfig_type(&mut self, kind: SmartConfigType) -> Result<(), DriverError> {
        self.smartconfig_type = Some(kind);
        Ok(())
    }

    fn set_smartconfig_timeout(&mut self, timeout: SmartConfigTimeout) -> Result<(), DriverError> {
        self.smartconfig_timeout = Some(timeout);
        Ok(())
    }

    fn set_smartconfig_fast_mode(&mut self, enabled: bool) -> Result<(), DriverError> {
        self.smartconfig_fast = enabled;
        Ok(())
    }

    fn start_smartconfig(&mut self, _config: &SmartConfigStartConfig) -> Result<(), DriverError> {
        self.smartconfig_started = true;
        Ok(())
    }

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
fn wps_session_owns_lifecycle_and_reads_events() {
    let resources = WifiResources::<8, 1, 1>::new();
    let mut driver = WifiDriver::with_backend(&resources, FakeProvisioningBackend::default())
        .initialize(InitConfig::default())
        .unwrap()
        .enable_sta()
        .unwrap()
        .start()
        .unwrap();

    let mut session = driver.wps().unwrap();
    session.start(&WpsConfig::pbc(), 0).unwrap();
    assert!(session.is_active());

    resources.publish_event(WifiEvent::WpsSucceeded).unwrap();
    assert_eq!(session.next_event(), Some(WifiEvent::WpsSucceeded));

    session.stop().unwrap();
    assert!(!session.is_active());
}

#[test]
fn smartconfig_session_owns_lifecycle_and_reads_credentials() {
    let resources = WifiResources::<8, 1, 1>::new();
    let mut driver = WifiDriver::with_backend(&resources, FakeProvisioningBackend::default())
        .initialize(InitConfig::default())
        .unwrap()
        .enable_sta()
        .unwrap()
        .start()
        .unwrap();

    let credentials = SmartConfigCredentials::new(
        b"network",
        b"password123",
        Some([1, 2, 3, 4, 5, 6]),
        SmartConfigType::EspTouch,
        7,
        Ipv4Addr::new(192, 168, 0, 2),
    )
    .unwrap();

    let mut session = driver.smart_config().unwrap();
    session.set_type(SmartConfigType::EspTouch).unwrap();
    session
        .set_timeout(SmartConfigTimeout::try_from(15).unwrap())
        .unwrap();
    session.set_fast_mode(true).unwrap();
    session.start(&SmartConfigStartConfig::default()).unwrap();

    resources
        .publish_event(WifiEvent::SmartConfigGotCredentials { credentials })
        .unwrap();
    assert_eq!(
        session.next_event(),
        Some(WifiEvent::SmartConfigGotCredentials { credentials })
    );
    session.stop().unwrap();
}

#[test]
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
    assert!(matches!(client.credential(), Credential::Wpa2(_)));

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
    assert!(matches!(client.credential(), Credential::Open));
}
