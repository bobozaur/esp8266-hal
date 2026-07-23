#![no_main]
#![no_std]

use esp8266_hal::wifi::{
    ClientConfig, InitConfig, Ssid, StationCredential, WifiDriver, WifiResources,
};
use panic_halt as _;
use static_cell::StaticCell;

static RESOURCES: StaticCell<WifiResources<1, 1>> = StaticCell::new();

#[esp8266_hal::main]
fn main() -> ! {
    let resources = RESOURCES.init_with(WifiResources::new);
    let driver = WifiDriver::new(resources)
        .initialize(InitConfig::default())
        .unwrap();
    let config = ClientConfig::new(
        Ssid::try_from("downstream-link").unwrap(),
        StationCredential::open(),
    );
    let _driver = driver.configure_sta(config).unwrap().start().unwrap();

    loop {
        esp8266_hal::wifi::delay_ticks(100);
    }
}
