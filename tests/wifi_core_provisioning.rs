#![cfg(feature = "wifi-smartconfig")]
use esp8266_hal::wifi::{Ipv4Addr, SmartConfigCredentials, SmartConfigType};
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

#[test]
fn smartconfig_client_preserves_sender_selected_bssid() {
    let bssid = [1, 2, 3, 4, 5, 6];
    let credentials = SmartConfigCredentials::new(
        b"network",
        b"password123",
        Some(bssid),
        SmartConfigType::EspTouch,
        0,
        Ipv4Addr::UNSPECIFIED,
    )
    .unwrap();
    assert_eq!(credentials.client_config().unwrap().bssid(), Some(bssid));
}
