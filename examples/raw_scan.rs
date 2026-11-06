//! Scan for access points without connecting or starting an IP stack.

#![no_std]
#![no_main]

mod support;

use core::fmt::Write;
use esp8266_hal::timer::TimerExt;
use esp8266_hal::wifi::{AccessPointRecord, InitConfig, ScanOptions, Wifi};
use panic_halt as _;

#[esp8266_hal::entry]
fn main() -> ! {
    let peripherals = esp8266_hal::target::Peripherals::take().unwrap();
    let mut serial = support::serial(peripherals.UART0, peripherals.GPIO);
    let (_, timer2) = peripherals.TIMER.timers();
    let runtime = support::init(timer2);
    let mut wifi = Wifi::new(runtime, InitConfig::default()).expect("Wi-Fi initialization");
    let (mut controller, _) = wifi.split();

    controller.enable_station().expect("station mode");
    controller.start_blocking(1_000).expect("start Wi-Fi");
    let mut records: [AccessPointRecord; 16] = core::array::from_fn(|_| Default::default());
    let options = ScanOptions::all().show_hidden(true);

    match controller.scan_blocking(&options, &mut records, 15_000) {
        Ok(count) => {
            writeln!(serial, "Found {count} access points:\r").unwrap();
            for record in &records[..count] {
                writeln!(
                    serial,
                    "{} {:02x?} channel={} RSSI={} authentication={:?}\r",
                    core::str::from_utf8(record.ssid()).unwrap_or("<non-UTF-8 SSID>"),
                    record.bssid(),
                    record.channel().get(),
                    record.rssi(),
                    record.authentication(),
                )
                .unwrap();
            }
        }
        Err(error) => writeln!(serial, "Scan failed: {error:?}\r").unwrap(),
    }
    loop {
        esp8266_hal::wifi::delay_ticks(100);
    }
}
