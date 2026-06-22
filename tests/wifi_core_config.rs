use esp8266_hal::wifi_core::{
    AccessPointConfig, ApClientLimit, BeaconInterval, Channel, ClientConfig, ConfigError,
    Credential, EnterpriseIdentity, EnterprisePassword, EnterpriseUsername, InitConfig, Passphrase,
    SmartConfigTimeout, Ssid, StationPowerSave, Wpa2EnterpriseClientCertificate,
    Wpa2EnterpriseCredentials, Wpa2EnterpriseMaterialFormat, WpsConfig, WpsFactoryInfo, WpsMode,
    WpsPin,
};

#[test]
fn ssid_rejects_more_than_32_bytes() {
    assert!(matches!(
        Ssid::try_from("123456789012345678901234567890123"),
        Err(ConfigError::SsidTooLong)
    ));
}

#[test]
fn ssid_accepts_exactly_32_bytes_and_binary_values() {
    let bytes = b"12345678901234567890123456789012";
    assert_eq!(Ssid::from_bytes(bytes).unwrap().as_bytes(), bytes);
    assert_eq!(
        Ssid::from_bytes(&[0xff, 0x00, b'a']).unwrap().as_bytes(),
        &[0xff, 0x00, b'a']
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
fn wpa2_passphrase_accepts_exactly_64_bytes_and_binary_values() {
    let bytes = b"1234567890123456789012345678901212345678901234567890123456789012";
    assert_eq!(Passphrase::wpa2_bytes(bytes).unwrap().as_bytes(), bytes);
    assert_eq!(
        Passphrase::wpa2_bytes(&[0xff, b'1', b'2', b'3', b'4', b'5', b'6', b'7'])
            .unwrap()
            .as_bytes(),
        &[0xff, b'1', b'2', b'3', b'4', b'5', b'6', b'7']
    );
}

#[test]
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

    assert_eq!(
        EnterprisePassword::new(""),
        Err(ConfigError::EnterprisePasswordEmpty)
    );
    assert_eq!(
        EnterprisePassword::from_bytes(&[b'p'; EnterprisePassword::MAX_LEN + 1]),
        Err(ConfigError::EnterprisePasswordTooLong)
    );
    assert_eq!(
        EnterprisePassword::from_bytes(&[b'p'; EnterprisePassword::MAX_LEN])
            .unwrap()
            .as_bytes()
            .len(),
        EnterprisePassword::MAX_LEN
    );
}

#[test]
fn enterprise_credentials_preserve_username_password_and_defaults() {
    let credentials = Wpa2EnterpriseCredentials::new(
        EnterpriseIdentity::new("anonymous").unwrap(),
        EnterpriseUsername::new("esp8266-user").unwrap(),
        EnterprisePassword::new("esp8266-password").unwrap(),
    )
    .with_disable_time_check(false);

    assert_eq!(credentials.identity().as_bytes(), b"anonymous");
    assert_eq!(credentials.username().as_bytes(), b"esp8266-user");
    assert_eq!(credentials.password().as_bytes(), b"esp8266-password");
    assert!(!credentials.disable_time_check());

    let config = ClientConfig::new(
        Ssid::try_from("enterprise-net").unwrap(),
        Credential::Wpa2Enterprise(credentials),
    );
    assert!(matches!(
        config.credential(),
        Credential::Wpa2Enterprise(credentials)
            if credentials.username().as_bytes() == b"esp8266-user"
    ));
}

#[test]
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
    let client = ClientConfig::new(ssid.clone(), Credential::Open);
    assert_eq!(client.ssid(), &ssid);
    assert_eq!(client.power_save(), StationPowerSave::None);
}

#[test]
fn client_config_preserves_station_power_save() {
    let client = ClientConfig::new(Ssid::try_from("core-net").unwrap(), Credential::Open)
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
fn ap_config_builder_preserves_validated_limits() {
    let ap = AccessPointConfig::builder(
        Ssid::try_from("core-ap").unwrap(),
        Channel::try_from(11u8).unwrap(),
    )
    .max_clients(ApClientLimit::try_from(7u8).unwrap())
    .beacon_interval(BeaconInterval::try_from(250u16).unwrap())
    .build();

    assert_eq!(ap.max_clients().get(), 7);
    assert_eq!(ap.beacon_interval().millis(), 250);
}

#[test]
fn ap_client_limit_rejects_values_outside_sdk_range() {
    assert_eq!(
        ApClientLimit::try_from(0u8),
        Err(ConfigError::InvalidApClientLimit(0))
    );
    assert_eq!(
        ApClientLimit::try_from(11u8),
        Err(ConfigError::InvalidApClientLimit(11))
    );
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
fn smartconfig_timeout_rejects_values_below_sdk_range() {
    assert_eq!(
        SmartConfigTimeout::try_from(14u8),
        Err(ConfigError::InvalidSmartConfigTimeout(14))
    );
    assert_eq!(SmartConfigTimeout::try_from(15u8).unwrap().seconds(), 15);
}

#[test]
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
fn wps_config_preserves_pin_target_ssid_and_truncates_factory_info() {
    let pin = WpsPin::try_from("12345670").unwrap();
    let target = Ssid::try_from("esp8266-hal-wps").unwrap();
    let factory = WpsFactoryInfo::empty()
        .manufacturer(&[b'm'; 65])
        .model_number(&[b'n'; 33])
        .model_name(&[b'o'; 33])
        .device_name(&[b'd'; 33]);

    let config = WpsConfig::pbc()
        .with_fixed_pin(pin)
        .with_target_ssid(target.clone())
        .with_factory_info(factory);

    assert_eq!(config.mode(), WpsMode::Pin);
    assert_eq!(config.pin_override(), Some(pin));
    assert_eq!(config.target_ssid().unwrap().as_bytes(), target.as_bytes());
    assert_eq!(config.factory_info().manufacturer_bytes().len(), 64);
    assert_eq!(config.factory_info().model_number_bytes().len(), 32);
    assert_eq!(config.factory_info().model_name_bytes().len(), 32);
    assert_eq!(config.factory_info().device_name_bytes().len(), 32);
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
