use esp8266_hal::wifi_core::{
    AccessPointConfig, Channel, ClientConfig, Credential, DriverError, EspNowPeerConfig,
    EspNowRxFrame, EspNowSendStatus, EspNowTxStatus, InitConfig, MixedConfig, RawMode, Ssid,
    WifiApBackend, WifiDriver, WifiFeatureBackend, WifiLifecycleBackend, WifiPacketBackend,
    WifiResources, WifiRuntimeBackend, WifiScanBackend, WifiStaBackend, ESP_NOW_MAX_PAYLOAD,
};

#[derive(Default)]
struct FakeEspNowBackend {
    enabled: bool,
    disabled: bool,
    channel: Option<Channel>,
    peers: heapless::Vec<[u8; 6], 4>,
    modified: heapless::Vec<[u8; 6], 4>,
    sent: heapless::Vec<usize, 4>,
}

impl WifiRuntimeBackend for FakeEspNowBackend {
    fn now_ms(&self) -> u64 {
        0
    }

    fn spin_once(&mut self) {}
}

impl WifiLifecycleBackend for FakeEspNowBackend {
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

impl WifiApBackend for FakeEspNowBackend {
    fn configure_mixed(
        &mut self,
        _client: &ClientConfig,
        _access_point: &AccessPointConfig,
    ) -> Result<(), DriverError> {
        Ok(())
    }
}

impl WifiFeatureBackend for FakeEspNowBackend {
    fn enable_esp_now(&mut self) -> Result<(), DriverError> {
        self.enabled = true;
        Ok(())
    }

    fn disable_esp_now(&mut self) -> Result<(), DriverError> {
        self.disabled = true;
        Ok(())
    }

    fn set_channel(&mut self, channel: Channel) -> Result<(), DriverError> {
        self.channel = Some(channel);
        Ok(())
    }

    fn add_esp_now_peer(&mut self, peer: &EspNowPeerConfig) -> Result<(), DriverError> {
        self.peers
            .push(*peer.peer_addr())
            .map_err(|_| DriverError::PeerOverflow)
    }

    fn modify_esp_now_peer(&mut self, peer: &EspNowPeerConfig) -> Result<(), DriverError> {
        self.modified
            .push(*peer.peer_addr())
            .map_err(|_| DriverError::PeerOverflow)
    }

    fn remove_esp_now_peer(&mut self, peer_addr: &[u8; 6]) -> Result<(), DriverError> {
        self.peers.retain(|peer| peer != peer_addr);
        Ok(())
    }

    fn send_esp_now(&mut self, _peer_addr: &[u8; 6], payload: &[u8]) -> Result<(), DriverError> {
        self.sent
            .push(payload.len())
            .map_err(|_| DriverError::PacketOverflow)
    }
}

impl WifiStaBackend for FakeEspNowBackend {}
impl WifiPacketBackend for FakeEspNowBackend {}
impl WifiScanBackend for FakeEspNowBackend {}

#[test]
fn esp_now_session_tracks_peers_rx_and_send_lifecycle() {
    let resources = WifiResources::<8, 4, 2, 2, 2, 0>::new();
    let mut driver = WifiDriver::with_backend(&resources, FakeEspNowBackend::default())
        .initialize(InitConfig::default())
        .unwrap()
        .enable_sta()
        .unwrap()
        .start()
        .unwrap();

    let mut session = driver.esp_now().unwrap();
    session.set_channel(Channel::try_from(6).unwrap()).unwrap();
    session.start().unwrap();
    assert!(session.is_active());

    let peer_addr = [1, 2, 3, 4, 5, 6];
    assert!(session
        .add_station_peer(EspNowPeerConfig::unencrypted(peer_addr).for_station())
        .unwrap());
    assert_eq!(session.peer_count(), 1);

    assert!(session
        .update_station_peer(EspNowPeerConfig::encrypted(peer_addr, [0x42; 16]).for_station())
        .unwrap());
    assert_eq!(session.peer_count(), 1);

    session.send(&peer_addr, b"hello").unwrap();
    resources
        .enqueue_esp_now_rx(EspNowRxFrame::new(peer_addr, b"pong").unwrap())
        .unwrap();
    resources
        .enqueue_esp_now_tx_status(EspNowTxStatus::new(peer_addr, EspNowSendStatus::Success))
        .unwrap();

    let frame = session.receive().expect("esp-now rx frame");
    assert_eq!(frame.peer_addr(), &peer_addr);
    assert_eq!(frame.payload(), b"pong");

    let status = session.send_status().expect("esp-now send status");
    assert_eq!(status.peer_addr(), &peer_addr);
    assert_eq!(status.status(), EspNowSendStatus::Success);

    assert!(session.remove_peer(&peer_addr).unwrap());
    assert_eq!(session.peer_count(), 0);
    session.stop().unwrap();
    assert!(!session.is_active());
}

#[test]
fn esp_now_mixed_mode_accepts_ap_interface_peers() {
    let resources = WifiResources::<8, 4, 2, 2, 2, 0>::new();
    let mut driver = WifiDriver::with_backend(&resources, FakeEspNowBackend::default())
        .initialize(InitConfig::default())
        .unwrap()
        .configure_mixed(MixedConfig::new(
            ClientConfig::new(Ssid::try_from("upstream").unwrap(), Credential::Open),
            AccessPointConfig::open(
                Ssid::try_from("core-ap").unwrap(),
                Channel::try_from(6).unwrap(),
            ),
        ))
        .unwrap()
        .start()
        .unwrap();

    let mut session = driver.esp_now().unwrap();
    let peer_addr = [6, 5, 4, 3, 2, 1];
    session.start().unwrap();

    assert!(session
        .add_access_point_peer(EspNowPeerConfig::unencrypted(peer_addr).for_access_point())
        .unwrap());
    assert_eq!(session.peer_count(), 1);
}

#[test]
fn esp_now_duplicate_peer_add_is_idempotent() {
    let resources = WifiResources::<8, 4, 2, 2, 2, 0>::new();
    let mut driver = WifiDriver::with_backend(&resources, FakeEspNowBackend::default())
        .initialize(InitConfig::default())
        .unwrap()
        .enable_sta()
        .unwrap()
        .start()
        .unwrap();
    let mut session = driver.esp_now().unwrap();
    let peer_addr = [1, 3, 3, 7, 0, 1];
    let peer = EspNowPeerConfig::unencrypted(peer_addr).for_station();
    session.start().unwrap();

    assert_eq!(session.add_station_peer(peer).unwrap(), true);
    assert_eq!(session.add_station_peer(peer).unwrap(), false);
    assert_eq!(session.peer_count(), 1);
}

#[test]
fn esp_now_peer_capacity_is_checked_before_backend_mutation() {
    let resources = WifiResources::<8, 4, 2, 1, 2, 0>::new();
    let mut driver = WifiDriver::with_backend(&resources, FakeEspNowBackend::default())
        .initialize(InitConfig::default())
        .unwrap()
        .enable_sta()
        .unwrap()
        .start()
        .unwrap();
    let mut session = driver.esp_now().unwrap();
    session.start().unwrap();

    assert_eq!(
        session
            .add_station_peer(EspNowPeerConfig::unencrypted([1, 1, 1, 1, 1, 1]).for_station())
            .unwrap(),
        true
    );
    assert_eq!(
        session.add_station_peer(EspNowPeerConfig::unencrypted([2; 6]).for_station()),
        Err(DriverError::PeerOverflow)
    );
    assert_eq!(session.peer_count(), 1);
}

#[test]
fn esp_now_update_requires_known_peer() {
    let resources = WifiResources::<8, 4, 2, 2, 2, 0>::new();
    let mut driver = WifiDriver::with_backend(&resources, FakeEspNowBackend::default())
        .initialize(InitConfig::default())
        .unwrap()
        .enable_sta()
        .unwrap()
        .start()
        .unwrap();
    let mut session = driver.esp_now().unwrap();
    session.start().unwrap();

    assert_eq!(
        session.update_station_peer(EspNowPeerConfig::encrypted([9; 6], [0x55; 16]).for_station()),
        Err(DriverError::BackendRejected)
    );
    assert_eq!(session.peer_count(), 0);
}

#[test]
fn esp_now_peer_mutation_requires_started_session() {
    let resources = WifiResources::<8, 4, 2, 2, 2, 0>::new();
    let mut driver = WifiDriver::with_backend(&resources, FakeEspNowBackend::default())
        .initialize(InitConfig::default())
        .unwrap()
        .enable_sta()
        .unwrap()
        .start()
        .unwrap();
    let mut session = driver.esp_now().unwrap();

    assert_eq!(
        session.add_station_peer(EspNowPeerConfig::unencrypted([1; 6]).for_station()),
        Err(DriverError::NotStarted)
    );
    assert_eq!(session.peer_count(), 0);
}

#[test]
fn esp_now_send_requires_started_and_registered_peer() {
    let resources = WifiResources::<8, 4, 2, 2, 2, 0>::new();
    let mut driver = WifiDriver::with_backend(&resources, FakeEspNowBackend::default())
        .initialize(InitConfig::default())
        .unwrap()
        .enable_sta()
        .unwrap()
        .start()
        .unwrap();
    let mut session = driver.esp_now().unwrap();
    let peer_addr = [4, 4, 4, 4, 4, 4];

    assert_eq!(
        session.send(&peer_addr, b"before-start"),
        Err(DriverError::NotStarted)
    );

    session.start().unwrap();
    assert_eq!(
        session.send(&peer_addr, b"unknown"),
        Err(DriverError::BackendRejected)
    );
    assert_eq!(
        session
            .add_station_peer(EspNowPeerConfig::unencrypted(peer_addr).for_station())
            .unwrap(),
        true
    );
    assert_eq!(session.send(&peer_addr, b"ok"), Ok(()));
}

#[test]
fn esp_now_send_rejects_oversized_payload_before_backend_send() {
    let resources = WifiResources::<8, 4, 2, 2, 2, 0>::new();
    let mut driver = WifiDriver::with_backend(&resources, FakeEspNowBackend::default())
        .initialize(InitConfig::default())
        .unwrap()
        .enable_sta()
        .unwrap()
        .start()
        .unwrap();
    let mut session = driver.esp_now().unwrap();
    let peer_addr = [5, 5, 5, 5, 5, 5];
    let payload = [0u8; ESP_NOW_MAX_PAYLOAD + 1];

    session.start().unwrap();
    session
        .add_station_peer(EspNowPeerConfig::unencrypted(peer_addr).for_station())
        .unwrap();

    assert_eq!(
        session.send(&peer_addr, &payload),
        Err(DriverError::PayloadTooLong)
    );
}

#[test]
fn esp_now_rx_frame_accepts_empty_payload() {
    let frame = EspNowRxFrame::new([7; 6], &[]).unwrap();

    assert_eq!(frame.peer_addr(), &[7; 6]);
    assert_eq!(frame.payload(), b"");
}
