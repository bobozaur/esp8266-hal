//! WiFi scan test — reports all visible APs.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use esp8266_hal::println;
use esp8266_hal::wifi::controller::WifiInitConfig;
use esp8266_hal::wifi::{WifiController, WifiMode};

#[esp8266_hal::main]
async fn main(_spawner: Spawner) {
    for _ in 0..20_000_000u32 {
        core::hint::spin_loop();
    }
    println!("Scan test");

    let mut wifi = WifiController::init(WifiInitConfig::default()).expect("wifi");
    println!("init OK");
    wifi.set_mode(WifiMode::Sta).expect("mode");
    wifi.start().expect("start");
    println!("scanning...");

    let config = esp8266_hal::wifi::scan::ScanConfig::default();
    match wifi.scan_async(&config).await {
        Ok(results) => {
            println!("Found {} APs:", results.len());
            for ap in &results {
                let ssid = core::str::from_utf8(&ap.ssid).unwrap_or("?");
                println!("  {} ch={} rssi={}", ssid, ap.channel, ap.rssi);
            }
        }
        Err(e) => println!("Scan err: {:?}", e),
    }

    println!("Done");
    loop {
        embassy_time::Timer::after(embassy_time::Duration::from_secs(60)).await;
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    esp8266_hal::logger::write_byte(b'?');
    loop {}
}
