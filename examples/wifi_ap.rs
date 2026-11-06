//! Start an open access point and list its associated stations.
//! IP addressing and services are application responsibilities; this example
//! operates the radio without a DHCP server or IP stack.

#![no_std]
#![no_main]

mod support;

use core::fmt::Write;
use esp8266_hal::timer::TimerExt;
use esp8266_hal::wifi::{
    AccessPointConfig, ApClientLimit, ApStationInfo, Channel, Configuration, InitConfig, Ssid, Wifi,
};
use panic_halt as _;

#[esp8266_hal::entry]
fn main() -> ! {
    let peripherals = esp8266_hal::target::Peripherals::take().unwrap();
    let mut serial = support::serial(peripherals.UART0, peripherals.GPIO);
    let (_, timer2) = peripherals.TIMER.timers();
    let runtime = support::init(timer2);
    let mut init = InitConfig::default();
    init.queues.station_rx = 0;
    let mut wifi = Wifi::new(runtime, init).expect("Wi-Fi initialization");
    let (mut controller, _) = wifi.split();

    let config = AccessPointConfig::open(
        Ssid::try_from("esp8266-hal-ap").unwrap(),
        Channel::try_from(6).unwrap(),
    )
    .with_max_clients(ApClientLimit::try_from(4).unwrap());
    controller
        .set_configuration(&Configuration::AccessPoint(config))
        .expect("AP configuration");
    controller
        .start_blocking(1_000)
        .expect("start access point");
    writeln!(
        serial,
        "Access point esp8266-hal-ap is running on channel 6\r"
    )
    .unwrap();

    let mut stations = [ApStationInfo::default(); 4];
    loop {
        match controller.connected_stations(&mut stations) {
            Ok(count) => {
                writeln!(serial, "Associated stations: {count}\r").unwrap();
                for station in &stations[..count] {
                    writeln!(serial, "  {:02x?}\r", station.mac()).unwrap();
                }
            }
            Err(error) => writeln!(serial, "Station list failed: {error:?}\r").unwrap(),
        }
        esp8266_hal::wifi::delay_ticks(500);
    }
}
