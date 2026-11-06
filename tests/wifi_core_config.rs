#[cfg(feature = "wifi-smartconfig")]
use esp8266_hal::wifi::SmartConfigTimeout;
use esp8266_hal::wifi::{
    AccessPointConfig, ApClientLimit, BeaconInterval, Channel, ClientConfig, ConfigError,
    InitConfig, Passphrase, Ssid, StationCredential, StationPowerSave,
};
#[cfg(feature = "wifi-enterprise")]
use esp8266_hal::wifi::{
    EnterpriseIdentity, EnterprisePassword, EnterpriseUsername, Wpa2EnterpriseClientCertificate,
    Wpa2EnterpriseCredentials, Wpa2EnterpriseMaterialFormat,
};
#[cfg(feature = "wifi-wps")]
use esp8266_hal::wifi::{WpsConfig, WpsFactoryInfo, WpsMode, WpsPin};

#[test]
fn ssid_rejects_more_than_32_bytes() {
    assert!(matches!(
        Ssid::try_from("123456789012345678901234567890123"),
        Err(ConfigError::SsidTooLong)
    ));
}

#[test]
fn ssid_accepts_exactly_32_bytes_and_rejects_unrepresentable_values() {
    let bytes = b"12345678901234567890123456789012";
    assert_eq!(Ssid::from_bytes(bytes).unwrap().as_bytes(), bytes);
    assert_eq!(Ssid::from_bytes(&[]), Err(ConfigError::SsidEmpty));
    assert_eq!(
        Ssid::from_bytes(&[0xff, 0x00, b'a']),
        Err(ConfigError::SsidContainsNul)
    );
}

#[test]
fn wpa2_passphrase_rejects_short_values() {
    assert!(matches!(
        Passphrase::wpa2("short"),
        Err(ConfigError::PassphraseTooShort)
    ));
}

#[test]
fn wpa2_passphrase_accepts_text_or_hex_psk_and_rejects_invalid_bytes() {
    let bytes = b"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    assert_eq!(Passphrase::wpa2_bytes(bytes).unwrap().as_bytes(), bytes);
    assert!(matches!(
        Passphrase::wpa2_bytes(&[0xff, b'1', b'2', b'3', b'4', b'5', b'6', b'7']),
        Err(ConfigError::PassphraseInvalidCharacter)
    ));
    assert!(matches!(
        Passphrase::wpa2_bytes(&[b'z'; 64]),
        Err(ConfigError::PskInvalidHex)
    ));
}

#[test]
#[cfg(feature = "wifi-enterprise")]
fn enterprise_credentials_validate_documented_identity_limits() {
    assert_eq!(
        EnterpriseIdentity::new(""),
        Err(ConfigError::EnterpriseIdentityEmpty)
    );
    assert_eq!(
        EnterpriseIdentity::from_bytes(&[b'a'; EnterpriseIdentity::MAX_LEN + 1]),
        Err(ConfigError::EnterpriseIdentityTooLong)
    );
    assert_eq!(
        EnterpriseIdentity::from_bytes(&[b'a'; EnterpriseIdentity::MAX_LEN])
            .unwrap()
            .as_bytes()
            .len(),
        EnterpriseIdentity::MAX_LEN
    );
}

#[test]
#[cfg(feature = "wifi-enterprise")]
fn enterprise_credentials_validate_username_and_password_limits() {
    assert_eq!(
        EnterpriseUsername::new(""),
        Err(ConfigError::EnterpriseUsernameEmpty)
    );
    assert_eq!(
        EnterpriseUsername::from_bytes(&[b'u'; EnterpriseUsername::MAX_LEN + 1]),
        Err(ConfigError::EnterpriseUsernameTooLong)
    );
    assert_eq!(
        EnterpriseUsername::from_bytes(&[b'u'; EnterpriseUsername::MAX_LEN])
            .unwrap()
            .as_bytes()
            .len(),
        EnterpriseUsername::MAX_LEN
    );

    assert!(matches!(
        EnterprisePassword::new(""),
        Err(ConfigError::EnterprisePasswordEmpty)
    ));
    assert!(matches!(
        EnterprisePassword::from_bytes(&[b'p'; EnterprisePassword::MAX_LEN + 1]),
        Err(ConfigError::EnterprisePasswordTooLong)
    ));
    assert_eq!(
        EnterprisePassword::from_bytes(&[b'p'; EnterprisePassword::MAX_LEN])
            .unwrap()
            .as_bytes()
            .len(),
        EnterprisePassword::MAX_LEN
    );
}

#[test]
#[cfg(feature = "wifi-enterprise")]
fn enterprise_credentials_preserve_identity_username_and_password() {
    let credentials = Wpa2EnterpriseCredentials::new(
        EnterpriseIdentity::new("anonymous").unwrap(),
        EnterpriseUsername::new("esp8266-user").unwrap(),
        EnterprisePassword::new("esp8266-password").unwrap(),
    );

    assert_eq!(credentials.identity().as_bytes(), b"anonymous");
    assert_eq!(credentials.username().as_bytes(), b"esp8266-user");
    assert_eq!(credentials.password().as_bytes(), b"esp8266-password");

    let config = ClientConfig::new(
        Ssid::try_from("enterprise-net").unwrap(),
        StationCredential::enterprise(&credentials),
    );
    assert_eq!(
        config
            .credential()
            .enterprise_credentials()
            .unwrap()
            .username()
            .as_bytes(),
        b"esp8266-user"
    );
}

#[test]
#[cfg(feature = "wifi-enterprise")]
fn enterprise_credentials_preserve_optional_certificate_material() {
    let certificate = Wpa2EnterpriseClientCertificate::new(b"client-cert\0", b"client-key\0")
        .with_private_key_password(b"key-pass\0");
    let credentials = Wpa2EnterpriseCredentials::new(
        EnterpriseIdentity::new("anonymous").unwrap(),
        EnterpriseUsername::new("esp8266-user").unwrap(),
        EnterprisePassword::new("esp8266-password").unwrap(),
    )
    .with_ca_cert(b"ca-cert\0")
    .with_client_certificate(certificate);

    assert_eq!(credentials.ca_cert(), Some(&b"ca-cert\0"[..]));
    assert_eq!(
        credentials.ca_cert_format(),
        Some(Wpa2EnterpriseMaterialFormat::Pem)
    );
    let stored = credentials.client_certificate().unwrap();
    assert_eq!(stored.client_cert(), b"client-cert\0");
    assert_eq!(stored.private_key(), b"client-key\0");
    assert_eq!(stored.private_key_password(), Some(&b"key-pass\0"[..]));
    assert_eq!(
        stored.client_cert_format(),
        Wpa2EnterpriseMaterialFormat::Pem
    );
    assert_eq!(
        stored.private_key_format(),
        Wpa2EnterpriseMaterialFormat::Pem
    );
}

#[test]
#[cfg(feature = "wifi-enterprise")]
fn enterprise_credentials_preserve_der_certificate_material() {
    let certificate =
        Wpa2EnterpriseClientCertificate::new_der(b"\x30\x03client-cert", b"\x30\x03client-key");
    let credentials = Wpa2EnterpriseCredentials::new(
        EnterpriseIdentity::new("anonymous").unwrap(),
        EnterpriseUsername::new("esp8266-user").unwrap(),
        EnterprisePassword::new("esp8266-password").unwrap(),
    )
    .with_ca_cert_der(b"\x30\x03ca-cert")
    .with_client_certificate(certificate);

    assert_eq!(credentials.ca_cert(), Some(&b"\x30\x03ca-cert"[..]));
    assert_eq!(
        credentials.ca_cert_format(),
        Some(Wpa2EnterpriseMaterialFormat::Der)
    );
    let stored = credentials.client_certificate().unwrap();
    assert_eq!(stored.client_cert(), b"\x30\x03client-cert");
    assert_eq!(stored.private_key(), b"\x30\x03client-key");
    assert_eq!(
        stored.client_cert_format(),
        Wpa2EnterpriseMaterialFormat::Der
    );
    assert_eq!(
        stored.private_key_format(),
        Wpa2EnterpriseMaterialFormat::Der
    );
}

#[test]
fn channel_rejects_zero() {
    assert!(matches!(
        Channel::try_from(0u8),
        Err(ConfigError::InvalidChannel(0))
    ));
}

#[test]
fn open_client_config_preserves_ssid() {
    let ssid = Ssid::try_from("core-net").unwrap();
    let client = ClientConfig::new(ssid.clone(), StationCredential::open());
    assert_eq!(client.ssid(), &ssid);
    assert_eq!(client.power_save(), StationPowerSave::None);
}

#[test]
fn client_config_preserves_station_power_save() {
    let client = ClientConfig::new(
        Ssid::try_from("core-net").unwrap(),
        StationCredential::open(),
    )
    .with_power_save(StationPowerSave::MinModem);
    assert_eq!(client.power_save(), StationPowerSave::MinModem);
}

#[test]
fn open_ap_config_preserves_channel() {
    let ap = AccessPointConfig::open(
        Ssid::try_from("core-ap").unwrap(),
        Channel::try_from(6u8).unwrap(),
    );
    assert_eq!(ap.channel().get(), 6);
}

#[test]
fn ap_config_setters_preserve_validated_limits() {
    let ap = AccessPointConfig::open(
        Ssid::try_from("core-ap").unwrap(),
        Channel::try_from(11u8).unwrap(),
    )
    .with_max_clients(ApClientLimit::try_from(3u8).unwrap())
    .with_beacon_interval(BeaconInterval::try_from(250u16).unwrap());

    assert_eq!(ap.max_clients().get(), 3);
    assert_eq!(ap.beacon_interval().millis(), 250);
}

#[test]
fn ap_client_limit_rejects_values_outside_sdk_range() {
    assert_eq!(
        ApClientLimit::try_from(0u8),
        Err(ConfigError::InvalidApClientLimit(0))
    );
    assert_eq!(
        ApClientLimit::try_from(5u8),
        Err(ConfigError::InvalidApClientLimit(5))
    );
    assert_eq!(ApClientLimit::try_from(4u8).unwrap().get(), 4);
}

#[test]
fn beacon_interval_rejects_values_outside_sdk_range() {
    assert_eq!(
        BeaconInterval::try_from(99u16),
        Err(ConfigError::InvalidBeaconInterval(99))
    );
    assert_eq!(
        BeaconInterval::try_from(60_001u16),
        Err(ConfigError::InvalidBeaconInterval(60_001))
    );
}

#[test]
#[cfg(feature = "wifi-smartconfig")]
fn smartconfig_timeout_rejects_values_below_sdk_range() {
    assert_eq!(
        SmartConfigTimeout::try_from(14u8),
        Err(ConfigError::InvalidSmartConfigTimeout(14))
    );
    assert_eq!(SmartConfigTimeout::try_from(15u8).unwrap().seconds(), 15);
}

#[test]
#[cfg(feature = "wifi-wps")]
fn wps_pin_requires_exactly_eight_ascii_digits_with_valid_checksum() {
    assert_eq!(WpsPin::try_from("1234567"), Err(ConfigError::InvalidWpsPin));
    assert_eq!(
        WpsPin::try_from("1234567x"),
        Err(ConfigError::InvalidWpsPin)
    );
    assert_eq!(
        WpsPin::try_from("12345671"),
        Err(ConfigError::InvalidWpsPin)
    );
    assert_eq!(
        WpsPin::try_from("00000000"),
        Err(ConfigError::InvalidWpsPin)
    );
    assert_eq!(
        WpsPin::try_from("12345670").unwrap().as_bytes(),
        b"12345670"
    );
}

#[test]
#[cfg(feature = "wifi-wps")]
fn wps_config_borrows_validated_factory_info() {
    let pin = WpsPin::try_from("12345670").unwrap();
    let target = Ssid::try_from("esp8266-hal-wps").unwrap();
    let mut manufacturer = [b'm'; 64];
    let mut model_number = [b'n'; 32];
    let mut model_name = [b'o'; 32];
    let mut device_name = [b'd'; 32];
    // Factory text is copied literally by the native backend, including percent.
    manufacturer[0] = b'%';
    model_number[0] = b'%';
    model_name[0] = b'%';
    device_name[0] = b'%';
    let factory =
        WpsFactoryInfo::new(&manufacturer, &model_number, &model_name, &device_name).unwrap();

    assert_eq!(
        WpsFactoryInfo::new(&[b'm'; 65], b"model", b"name", b"device"),
        Err(ConfigError::WpsManufacturerTooLong)
    );
    assert_eq!(
        WpsFactoryInfo::new(b"maker\0suffix", b"model", b"name", b"device"),
        Err(ConfigError::WpsFactoryInfoContainsNul)
    );

    let config = WpsConfig::pbc()
        .with_fixed_pin(pin)
        .with_target_ssid(&target)
        .with_factory_info(factory);

    assert_eq!(config.mode(), WpsMode::Pin);
    assert_eq!(config.pin_override(), Some(pin));
    assert_eq!(config.target_ssid().unwrap().as_bytes(), target.as_bytes());
    assert_eq!(config.factory_info().manufacturer_bytes(), &manufacturer);
    assert_eq!(config.factory_info().model_number_bytes(), &model_number);
    assert_eq!(config.factory_info().model_name_bytes(), &model_name);
    assert_eq!(config.factory_info().device_name_bytes(), &device_name);
}

#[test]
fn init_config_defaults_match_current_buffer_counts() {
    let init = InitConfig::default();
    assert_eq!(init.rx_buf_num(), 16);
    assert_eq!(init.rx_pkt_num(), 7);
    assert_eq!(init.tx_buf_num(), 6);
}

#[test]
fn init_config_rejects_counts_outside_sdk_ranges() {
    assert_eq!(
        InitConfig::new(13, 7, 6),
        Err(ConfigError::InvalidRxBufferCount(13))
    );
    assert_eq!(
        InitConfig::new(16, 3, 6),
        Err(ConfigError::InvalidRxPacketCount(3))
    );
    assert_eq!(
        InitConfig::new(16, 7, 3),
        Err(ConfigError::InvalidTxBufferCount(3))
    );
    assert_eq!(InitConfig::new(14, 4, 4).unwrap().rx_buf_num(), 14);
}
