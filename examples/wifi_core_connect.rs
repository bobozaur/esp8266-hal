//! Associate a station using the radio controller, without an IP stack.
//! Set ESP8266_HAL_WIFI_SSID and ESP8266_HAL_WIFI_PASS when building.
//! An empty password selects an open network.

#![no_std]
#![no_main]

mod support;

use core::fmt::Write;
use esp8266_hal::timer::TimerExt;
use esp8266_hal::wifi::{
    ClientConfig, Configuration, InitConfig, Passphrase, Ssid, StationCredential, Wifi,
};
use panic_halt as _;

const SSID: &str = match option_env!("ESP8266_HAL_WIFI_SSID") {
    Some(value) => value,
    None => "your-ssid",
};
const PASSWORD: &str = match option_env!("ESP8266_HAL_WIFI_PASS") {
    Some(value) => value,
    None => "your-password",
};

#[esp8266_hal::entry]
fn main() -> ! {
    let peripherals = esp8266_hal::target::Peripherals::take().unwrap();
    let mut serial = support::serial(peripherals.UART0, peripherals.GPIO);
    let (_, timer2) = peripherals.TIMER.timers();
    let runtime = support::init(timer2);
    let mut init = InitConfig::default();
    init.queues.station_rx = 0; // This example does not consume Ethernet frames.
    let mut wifi = Wifi::new(runtime, init).expect("Wi-Fi initialization");
    let (mut controller, interfaces) = wifi.split();

    let credential = if PASSWORD.is_empty() {
        StationCredential::open()
    } else {
        StationCredential::wpa2(Passphrase::wpa2(PASSWORD).expect("password"))
    };
    let config = ClientConfig::new(Ssid::try_from(SSID).expect("SSID"), credential);
    controller
        .set_configuration(&Configuration::Station(config))
        .expect("station configuration");
    controller.start_blocking(1_000).expect("start Wi-Fi");

    match controller.connect_blocking(30_000) {
        Ok(()) => writeln!(
            serial,
            "Associated with {SSID}; station MAC {:02x?}\r",
            interfaces.station.mac_address()
        )
        .unwrap(),
        Err(error) => writeln!(serial, "Connection failed: {error:?}\r").unwrap(),
    }
    loop {
        esp8266_hal::wifi::delay_ticks(100);
    }
}
