use esp8266_hal::wifi_core::{
    AccessPointConfig, ApStationInfo, Channel, DriverError, InitConfig, LinkState, RawMode, Ssid,
    WifiApBackend, WifiDriver, WifiFeatureBackend, WifiLifecycleBackend, WifiPacketBackend,
    WifiResources, WifiRuntimeBackend, WifiScanBackend, WifiStaBackend,
};

#[derive(Default)]
struct FakeApBackend {
    deauth_all: bool,
    ap_rx_attached: bool,
    ap_rx_detached: bool,
    ap_tx_len: Option<usize>,
}

impl WifiRuntimeBackend for FakeApBackend {
    fn now_ms(&self) -> u64 {
        0
    }

    fn spin_once(&mut self) {}
}

impl WifiLifecycleBackend for FakeApBackend {
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

impl WifiApBackend for FakeApBackend {
    fn configure_ap(&mut self, _config: &AccessPointConfig) -> Result<(), DriverError> {
        Ok(())
    }

    fn connected_ap_stations<const N: usize>(
        &mut self,
    ) -> Result<heapless::Vec<ApStationInfo, N>, DriverError> {
        let mut stations = heapless::Vec::new();
        stations
            .push(ApStationInfo::new([1, 2, 3, 4, 5, 6], true, true, false))
            .unwrap();
        Ok(stations)
    }

    fn deauth_all_ap_stations(&mut self) -> Result<(), DriverError> {
        self.deauth_all = true;
        Ok(())
    }

    fn ap_mac_address(&self) -> Result<[u8; 6], DriverError> {
        Ok([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff])
    }
}

impl WifiPacketBackend for FakeApBackend {
    fn attach_ap_rx(&mut self) -> Result<(), DriverError> {
        self.ap_rx_attached = true;
        Ok(())
    }

    fn detach_ap_rx(&mut self) {
        self.ap_rx_detached = true;
    }

    fn transmit_ap(&mut self, frame: &[u8]) -> Result<(), DriverError> {
        self.ap_tx_len = Some(frame.len());
        Ok(())
    }
}

impl WifiStaBackend for FakeApBackend {}
impl WifiScanBackend for FakeApBackend {}
impl WifiFeatureBackend for FakeApBackend {}

#[test]
fn ap_handle_lists_stations() {
    let resources = WifiResources::<8, 4>::new();
    let mut driver = WifiDriver::with_backend(&resources, FakeApBackend::default())
        .initialize(InitConfig::default())
        .unwrap()
        .configure_ap(AccessPointConfig::open(
            Ssid::try_from("core-ap").unwrap(),
            Channel::try_from(6).unwrap(),
        ))
        .unwrap()
        .start()
        .unwrap();

    let mut ap = driver.ap();
    let stations = ap.connected_stations::<4>().unwrap();

    assert_eq!(stations[0].mac(), [1, 2, 3, 4, 5, 6]);
    assert!(stations[0].supports_11b());
    assert!(stations[0].supports_11g());
    assert!(!stations[0].supports_11n());
}

#[test]
fn ap_link_uses_ap_tx_interface() {
    let resources = WifiResources::<8, 2, 0, 0, 0, 2, 1514>::new();
    let driver = WifiDriver::with_backend(&resources, FakeApBackend::default())
        .initialize(InitConfig::default())
        .unwrap()
        .configure_ap(AccessPointConfig::open(
            Ssid::try_from("core-ap").unwrap(),
            Channel::try_from(6).unwrap(),
        ))
        .unwrap()
        .start()
        .unwrap();

    let mut link = driver.ap_link().unwrap();
    assert_eq!(link.link_state(), LinkState::Down);
    assert_eq!(
        link.mac_address().unwrap(),
        [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]
    );

    let mut frame = [0u8; 64];
    assert_eq!(link.receive(&mut frame), Ok(None));
    link.transmit(&[1, 2, 3, 4]).unwrap();

    let mut driver = link.into_driver().unwrap();
    let mut ap = driver.ap();
    ap.deauth_all().unwrap();
}

#[test]
fn ap_link_rejects_zero_copied_rx_capacity() {
    let resources = WifiResources::<8, 2>::new();
    let driver = WifiDriver::with_backend(&resources, FakeApBackend::default())
        .initialize(InitConfig::default())
        .unwrap()
        .configure_ap(AccessPointConfig::open(
            Ssid::try_from("core-ap").unwrap(),
            Channel::try_from(6).unwrap(),
        ))
        .unwrap()
        .start()
        .unwrap();

    assert!(matches!(driver.ap_link(), Err(DriverError::PacketOverflow)));
}
