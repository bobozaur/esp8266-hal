use esp8266_hal::wifi_core::WifiResources;

#[test]
fn resources_report_base_capacities() {
    let resources = WifiResources::<8, 4, 3, 2, 5, 7, 1536>::new();

    assert_eq!(resources.event_capacity(), 8);
    assert_eq!(resources.rx_capacity(), 4);
    #[cfg(feature = "wifi-promiscuous")]
    assert_eq!(resources.promiscuous_rx_capacity(), 5);
    assert_eq!(resources.ap_rx_copy_capacity(), 7);
    assert_eq!(resources.ap_rx_copy_frame_bytes(), 1536);
}

#[test]
#[cfg(feature = "wifi-esp-now")]
fn resources_report_esp_now_capacities() {
    let resources = WifiResources::<8, 4, 3, 2>::new();

    assert_eq!(resources.esp_now_peer_capacity(), 3);
    assert_eq!(resources.esp_now_rx_capacity(), 2);
}
