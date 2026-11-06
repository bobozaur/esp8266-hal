//! Run an access point while associating with an upstream Wi-Fi network.
//! Both interfaces share one radio channel: the AP follows the upstream channel
//! after association. Applications choose IP addressing and services separately.

#![no_std]
#![no_main]

mod support;

use core::fmt::Write;
use esp8266_hal::timer::TimerExt;
use esp8266_hal::wifi::{
    AccessPointConfig, ApClientLimit, ApStationInfo, Channel, ClientConfig, Configuration,
    InitConfig, MixedConfig, Passphrase, Ssid, StationCredential, Wifi,
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
    let mut init = InitConfig::new(14, 4, 4).unwrap();
    init.queues.station_rx = 0;
    let mut wifi = Wifi::new(runtime, init).expect("Wi-Fi initialization");
    let (mut controller, interfaces) = wifi.split();

    let credential = if PASSWORD.is_empty() {
        StationCredential::open()
    } else {
        StationCredential::wpa2(Passphrase::wpa2(PASSWORD).expect("password"))
    };
    let station = ClientConfig::new(Ssid::try_from(SSID).expect("SSID"), credential);
    let access_point = AccessPointConfig::open(
        Ssid::try_from("esp8266-hal-apsta").unwrap(),
        Channel::try_from(6).unwrap(),
    )
    .with_max_clients(ApClientLimit::try_from(4).unwrap());
    controller
        .set_configuration(&Configuration::Mixed(MixedConfig::new(
            station,
            access_point,
        )))
        .expect("mixed configuration");
    controller.start_blocking(1_000).expect("start Wi-Fi");
    writeln!(serial, "Access point esp8266-hal-apsta is running\r").unwrap();
    match controller.connect_blocking(30_000) {
        Ok(()) => writeln!(serial, "Station associated with {SSID}\r").unwrap(),
        Err(error) => writeln!(serial, "Station connection failed: {error:?}\r").unwrap(),
    }

    let mut stations = [ApStationInfo::default(); 4];
    loop {
        writeln!(
            serial,
            "Upstream link: {:?}\r",
            interfaces.station.link_state()
        )
        .unwrap();
        match controller.connected_stations(&mut stations) {
            Ok(count) => {
                writeln!(serial, "Downstream stations: {count}\r").unwrap();
                for station in &stations[..count] {
                    writeln!(serial, "  {:02x?}\r", station.mac()).unwrap();
                }
            }
            Err(error) => writeln!(serial, "Station list failed: {error:?}\r").unwrap(),
        }
        esp8266_hal::wifi::delay_ticks(500);
    }
}
