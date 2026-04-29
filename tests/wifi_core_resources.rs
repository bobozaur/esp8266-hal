use esp8266_hal::wifi_core::{
    DriverError, EspNowPeer, EspNowRxFrame, EspNowSendStatus, EspNowTxStatus, PromiscuousFrameInfo,
    PromiscuousPacketType, PromiscuousRxFrame, WifiResources,
};

#[test]
fn resources_report_all_declared_feature_capacities() {
    let resources = WifiResources::<8, 4, 2, 3, 2, 5, 7, 1536>::new();

    assert_eq!(resources.event_capacity(), 8);
    assert_eq!(resources.rx_capacity(), 4);
    assert_eq!(resources.subscriber_capacity(), 2);
    assert_eq!(resources.esp_now_peer_capacity(), 3);
    assert_eq!(resources.esp_now_rx_capacity(), 2);
    assert_eq!(resources.promiscuous_rx_capacity(), 5);
    assert_eq!(resources.ap_rx_copy_capacity(), 7);
    assert_eq!(resources.ap_rx_copy_frame_bytes(), 1536);
}

#[test]
fn esp_now_peer_storage_is_bounded_and_deduplicated() {
    let resources = WifiResources::<4, 1, 1, 2, 0, 0>::new();

    let first = EspNowPeer::unencrypted([1, 2, 3, 4, 5, 6]);
    let second = EspNowPeer::encrypted([6, 5, 4, 3, 2, 1]);

    assert!(resources.register_esp_now_peer(first).unwrap());
    assert!(!resources.register_esp_now_peer(first).unwrap());
    assert!(resources.register_esp_now_peer(second).unwrap());
    assert_eq!(resources.esp_now_peer_count(), 2);
    assert_eq!(
        resources.register_esp_now_peer(EspNowPeer::unencrypted([9; 6])),
        Err(DriverError::PeerOverflow)
    );
    assert!(resources.contains_esp_now_peer(&[1, 2, 3, 4, 5, 6]));
    assert!(resources.remove_esp_now_peer(&[1, 2, 3, 4, 5, 6]));
    assert!(!resources.contains_esp_now_peer(&[1, 2, 3, 4, 5, 6]));
}

#[test]
fn esp_now_rx_queue_is_caller_owned_and_bounded() {
    let resources = WifiResources::<4, 1, 1, 0, 1, 0>::new();
    let frame = EspNowRxFrame::new([1, 2, 3, 4, 5, 6], b"hello").unwrap();

    resources.enqueue_esp_now_rx(frame).unwrap();
    assert_eq!(resources.queued_esp_now_rx(), 1);
    assert_eq!(
        resources.enqueue_esp_now_rx(EspNowRxFrame::new([6; 6], b"again").unwrap()),
        Err(DriverError::PacketOverflow)
    );

    let received = resources.dequeue_esp_now_rx().unwrap();
    assert_eq!(received.peer_addr(), &[1, 2, 3, 4, 5, 6]);
    assert_eq!(received.payload(), b"hello");
    assert_eq!(resources.queued_esp_now_rx(), 0);
}

#[test]
fn esp_now_send_status_queue_is_caller_owned_and_bounded() {
    let resources = WifiResources::<4, 1, 1, 0, 1, 0>::new();
    let status = EspNowTxStatus::new([1, 2, 3, 4, 5, 6], EspNowSendStatus::Success);

    resources.enqueue_esp_now_tx_status(status).unwrap();
    assert_eq!(resources.queued_esp_now_tx_status(), 1);
    assert_eq!(
        resources.enqueue_esp_now_tx_status(EspNowTxStatus::new([6; 6], EspNowSendStatus::Failed)),
        Err(DriverError::PacketOverflow)
    );

    let received = resources.dequeue_esp_now_tx_status().unwrap();
    assert_eq!(received.peer_addr(), &[1, 2, 3, 4, 5, 6]);
    assert_eq!(received.status(), EspNowSendStatus::Success);
    assert_eq!(resources.queued_esp_now_tx_status(), 0);
}

#[test]
fn promiscuous_rx_queue_preserves_metadata_and_payload_prefix() {
    let resources = WifiResources::<4, 1, 1, 0, 0, 1>::new();
    let info = PromiscuousFrameInfo {
        rssi: -42,
        channel: 6,
        packet_type: PromiscuousPacketType::Management,
    };

    resources
        .enqueue_promiscuous_rx(PromiscuousRxFrame::new(info, b"beacon").unwrap())
        .unwrap();
    let received = resources.dequeue_promiscuous_rx().unwrap();

    assert_eq!(received.info(), info);
    assert_eq!(received.payload(), b"beacon");
}

#[test]
fn ap_rx_copy_rejects_non_empty_null_source() {
    let resources = WifiResources::<4, 1, 1, 0, 0, 0, 1, 64>::new();

    assert_eq!(
        resources.enqueue_ap_rx_copy(core::ptr::null_mut(), 1),
        Err(DriverError::BackendRejected)
    );
    assert_eq!(resources.queued_ap_rx(), 0);
}

#[test]
fn default_ap_rx_copy_storage_is_disabled() {
    let resources = WifiResources::<4, 1, 1>::new();
    let bytes = [1_u8; 8];

    assert_eq!(
        resources.enqueue_ap_rx_copy(bytes.as_ptr().cast_mut().cast(), bytes.len() as u16),
        Err(DriverError::PacketOverflow)
    );
    assert_eq!(resources.queued_ap_rx(), 0);
}

#[test]
fn zero_capacity_feature_queues_are_valid_and_count_drops() {
    let resources = WifiResources::<4, 1, 1, 0, 0, 0>::new();

    assert_eq!(
        resources.enqueue_esp_now_rx(EspNowRxFrame::new([1; 6], b"rx").unwrap()),
        Err(DriverError::PacketOverflow)
    );
    assert_eq!(resources.esp_now_rx_drops(), 1);
    assert_eq!(
        resources.enqueue_esp_now_tx_status(EspNowTxStatus::new([1; 6], EspNowSendStatus::Failed)),
        Err(DriverError::PacketOverflow)
    );
    assert_eq!(resources.esp_now_tx_status_drops(), 1);

    let info = PromiscuousFrameInfo {
        rssi: -1,
        channel: 1,
        packet_type: PromiscuousPacketType::Management,
    };
    assert_eq!(
        resources.enqueue_promiscuous_rx(PromiscuousRxFrame::new(info, b"x").unwrap()),
        Err(DriverError::PacketOverflow)
    );
    assert_eq!(resources.promiscuous_rx_drops(), 1);
}
