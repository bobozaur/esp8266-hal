//! Minimal Timer::after test — no ets_delay_us, no CCOUNT measurement.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};
use esp8266_hal::println;
use esp8266_hal::wifi::controller::WifiInitConfig;
use esp8266_hal::wifi::WifiController;

#[esp8266_hal::main]
async fn main(_spawner: Spawner) {
    for _ in 0..20_000_000u32 {
        core::hint::spin_loop();
    }
    println!("Timer test");

    let _wifi = WifiController::init(WifiInitConfig::default()).expect("wifi");
    println!("init done");

    // Debug: print now() to check FRC2 timing
    let n0 = Instant::now().as_micros();
    println!("now={}", n0);

    // Test Timer::after directly
    for i in 0..10u32 {
        let before = Instant::now().as_micros();
        Timer::after(Duration::from_millis(500)).await;
        let after = Instant::now().as_micros();
        println!("t{} dt={}", i, after - before);
    }

    println!("PASSED");
    loop {}
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    esp8266_hal::logger::write_byte(b'?');
    esp8266_hal::logger::write_byte(b'\n');
    loop {}
}
