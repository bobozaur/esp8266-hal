#![no_std]
#![no_main]

use panic_halt as _;

use esp8266_hal::wifi::{
    Channel, ClientConfig, Credential, Esp8266TargetBackend, InitConfig, Passphrase, Ssid,
    WifiDriver, WifiResources,
};
use static_cell::StaticCell;

const WIFI_SSID: &str = match option_env!("ESP8266_HAL_WIFI_SSID") {
    Some(ssid) => ssid,
    None => include_str!("ssid"),
};
const WIFI_PASS: &str = match option_env!("ESP8266_HAL_WIFI_PASS") {
    Some(pass) => pass,
    None => include_str!("pass"),
};
const WIFI_CHANNEL: Option<&str> = option_env!("ESP8266_HAL_WIFI_CHANNEL");
const WIFI_CONNECT_TIMEOUT_MS: Option<&str> = option_env!("ESP8266_HAL_WIFI_CONNECT_TIMEOUT_MS");

static RESOURCES: StaticCell<WifiResources<8, 4, 2>> = StaticCell::new();

#[esp8266_hal::main]
fn main() -> ! {
    esp8266_hal::logger::write_bytes(b"wifi_core_connect start");
    esp8266_hal::logger::write_newline();

    let resources = RESOURCES.init_with(WifiResources::new);
    let backend = Esp8266TargetBackend::new(resources);
    let driver = WifiDriver::with_backend(resources, backend);
    let driver = driver.initialize(InitConfig::default()).expect("init");
    esp8266_hal::logger::write_bytes(b"wifi core initialized");
    esp8266_hal::logger::write_newline();

    let credential = if WIFI_PASS.trim().is_empty() {
        Credential::Open
    } else {
        Credential::Wpa2(Passphrase::wpa2(WIFI_PASS.trim()).expect("pass"))
    };

    let mut config = ClientConfig::new(Ssid::try_from(WIFI_SSID.trim()).expect("ssid"), credential);
    if let Some(channel) = WIFI_CHANNEL
        .and_then(parse_u8_literal)
        .and_then(|channel| Channel::try_from(channel).ok())
    {
        config = config.with_channel(channel);
    }

    let mut driver = driver
        .configure_sta(config)
        .expect("configure")
        .start()
        .expect("start");
    esp8266_hal::logger::write_bytes(b"wifi core started");
    esp8266_hal::logger::write_newline();
    let connect_timeout_ms = WIFI_CONNECT_TIMEOUT_MS
        .and_then(parse_u32_literal)
        .unwrap_or(60_000);
    {
        let mut sta = driver.sta().expect("sta");
        if let Err(error) = sta.connect_blocking(connect_timeout_ms) {
            esp8266_hal::logger::write_bytes(b"wifi-connect-error=");
            esp8266_hal::logger::write_bytes(driver_error_name(error).as_bytes());
            esp8266_hal::logger::write_bytes(b" reason=");
            esp8266_hal::logger::write_dec_u32(
                resources.last_disconnect_reason().unwrap_or(0) as u32
            );
            esp8266_hal::logger::write_newline();
            esp8266_hal::logger::write_bytes(b"wifi core connect err");
            esp8266_hal::logger::write_newline();
            loop {
                esp8266_hal::wifi::delay_ticks(100);
            }
        }
    }
    esp8266_hal::logger::write_bytes(b"wifi core associated");
    esp8266_hal::logger::write_newline();

    let link = driver.sta_link().expect("sta link");
    let _mac = link.mac_address().expect("mac");
    let _state = link.link_state();

    esp8266_hal::logger::write_bytes(b"wifi core connected");
    esp8266_hal::logger::write_newline();

    loop {
        esp8266_hal::wifi::delay_ticks(100);
    }
}

fn parse_u8_literal(value: &str) -> Option<u8> {
    let mut result = 0u8;
    for byte in value.trim().bytes() {
        if !byte.is_ascii_digit() {
            return None;
        }
        result = result.checked_mul(10)?.checked_add(byte - b'0')?;
    }
    Some(result)
}

fn parse_u32_literal(value: &str) -> Option<u32> {
    let mut result = 0u32;
    for byte in value.trim().bytes() {
        if !byte.is_ascii_digit() {
            return None;
        }
        result = result.checked_mul(10)?.checked_add((byte - b'0') as u32)?;
    }
    Some(result)
}

fn driver_error_name(error: esp8266_hal::wifi::DriverError) -> &'static str {
    match error {
        esp8266_hal::wifi::DriverError::Timeout => "timeout",
        esp8266_hal::wifi::DriverError::StaStartTimeout => "sta-start-timeout",
        esp8266_hal::wifi::DriverError::Disconnected => "disconnected",
        esp8266_hal::wifi::DriverError::EventOverflow => "event-overflow",
        esp8266_hal::wifi::DriverError::PacketOverflow => "packet-overflow",
        esp8266_hal::wifi::DriverError::PeerOverflow => "peer-overflow",
        esp8266_hal::wifi::DriverError::PayloadTooLong => "payload-too-long",
        esp8266_hal::wifi::DriverError::OutOfMemory => "out-of-memory",
        esp8266_hal::wifi::DriverError::NotStarted => "not-started",
        esp8266_hal::wifi::DriverError::WouldBlock => "would-block",
        esp8266_hal::wifi::DriverError::BackendRejected => "backend-rejected",
        esp8266_hal::wifi::DriverError::BackendError(_) => "backend-error",
    }
}
