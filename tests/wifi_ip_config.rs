use esp8266_hal::wifi_core::{Ipv4Addr, StaticIpv4Config, StaticIpv4Error};

#[test]
fn static_ipv4_accepts_gateway_on_same_subnet() {
    let config = StaticIpv4Config::new(
        Ipv4Addr::new(192, 168, 0, 116),
        24,
        Some(Ipv4Addr::new(192, 168, 0, 1)),
    )
    .expect("valid static IPv4 config");

    assert_eq!(config.address(), Ipv4Addr::new(192, 168, 0, 116));
    assert_eq!(config.prefix_len(), 24);
    assert_eq!(config.gateway(), Some(Ipv4Addr::new(192, 168, 0, 1)));
}

#[test]
fn static_ipv4_rejects_invalid_prefix() {
    let error = StaticIpv4Config::new(Ipv4Addr::new(192, 168, 0, 116), 33, None)
        .expect_err("invalid prefix should fail");

    assert_eq!(error, StaticIpv4Error::InvalidPrefix);
}

#[test]
fn static_ipv4_rejects_gateway_outside_subnet() {
    let error = StaticIpv4Config::new(
        Ipv4Addr::new(192, 168, 0, 116),
        24,
        Some(Ipv4Addr::new(192, 168, 1, 1)),
    )
    .expect_err("gateway outside subnet should fail");

    assert_eq!(error, StaticIpv4Error::GatewayOutsideSubnet);
}
