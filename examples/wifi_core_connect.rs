#![no_std]
#![no_main]

use panic_halt as _;

mod support;
use esp8266_hal::timer::TimerExt;

use esp8266_hal::wifi::{
    Channel, ClientConfig, Configuration, InitConfig, Passphrase, Ssid, StationCredential, Wifi,
};
use static_cell::StaticCell;

const WIFI_SSID: &str = match option_env!("ESP8266_HAL_WIFI_SSID") {
    Some(ssid) => ssid,
    None => "esp8266-hal",
};
const WIFI_PASS: &str = match option_env!("ESP8266_HAL_WIFI_PASS") {
    Some(pass) => pass,
    None => "esp8266-pass",
};
const WIFI_CHANNEL: Option<&str> = option_env!("ESP8266_HAL_WIFI_CHANNEL");
const WIFI_CONNECT_TIMEOUT_MS: Option<&str> = option_env!("ESP8266_HAL_WIFI_CONNECT_TIMEOUT_MS");

static WIFI: StaticCell<Wifi> = StaticCell::new();

#[esp8266_hal::main]
fn main() -> ! {
    let peripherals = esp8266_hal::target::Peripherals::take().expect("peripherals");
    let (_, timer2) = peripherals.TIMER.timers();
    let runtime = support::init(timer2);
    esp8266_hal::logger::write_bytes(b"wifi_core_connect start");
    esp8266_hal::logger::write_newline();

    let wifi = WIFI.init(Wifi::new(runtime, InitConfig::default()).expect("wifi init"));
    let (mut driver, interfaces) = wifi.split();
    esp8266_hal::logger::write_bytes(b"wifi core initialized");
    esp8266_hal::logger::write_newline();

    let credential = if WIFI_PASS.trim().is_empty() {
        StationCredential::open()
    } else {
        StationCredential::wpa2(Passphrase::wpa2(WIFI_PASS.trim()).expect("pass"))
    };

    let mut config = ClientConfig::new(Ssid::try_from(WIFI_SSID.trim()).expect("ssid"), credential);
    if let Some(channel) = WIFI_CHANNEL
        .and_then(|value| value.trim().parse::<u8>().ok())
        .and_then(|channel| Channel::try_from(channel).ok())
    {
        config = config.with_channel(channel);
    }

    driver
        .set_configuration(&Configuration::Station(config))
        .expect("configure");
    driver.start_blocking(1_000).expect("start");
    esp8266_hal::logger::write_bytes(b"wifi core started");
    esp8266_hal::logger::write_newline();
    let connect_timeout_ms = WIFI_CONNECT_TIMEOUT_MS
        .and_then(|value| value.trim().parse::<u32>().ok())
        .unwrap_or(60_000);
    {
        let sta = &mut driver;
        if let Err(error) = sta.connect_blocking(connect_timeout_ms) {
            esp8266_hal::logger::write_bytes(b"wifi-connect-error=");
            support::log_debug(&error);
            esp8266_hal::logger::write_bytes(b" reason=");
            esp8266_hal::logger::write_dec_u32(driver.last_disconnect_reason().unwrap_or(0) as u32);
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

    let link = interfaces.station;
    let _state = link.link_state();

    esp8266_hal::logger::write_bytes(b"wifi core connected");
    esp8266_hal::logger::write_newline();

    loop {
        esp8266_hal::wifi::delay_ticks(100);
    }
}
