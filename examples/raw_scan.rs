#![no_std]
#![no_main]

use panic_halt as _;

mod support;
use esp8266_hal::timer::TimerExt;
use support::log_line;

use esp8266_hal::wifi::{AccessPointRecord, InitConfig, ScanOptions, ScanTimings, Wifi};
use static_cell::StaticCell;

static WIFI: StaticCell<Wifi> = StaticCell::new();

#[esp8266_hal::main]
fn main() -> ! {
    let peripherals = esp8266_hal::target::Peripherals::take().expect("peripherals");
    let (_, timer2) = peripherals.TIMER.timers();
    let runtime = support::init(timer2);
    log_line(b"wifi_core_scan start");

    let wifi = WIFI.init(Wifi::new(runtime, InitConfig::default()).expect("wifi init"));
    let (mut driver, _) = wifi.split();
    log_line(b"wifi core scan initialized");

    driver.enable_station().expect("configure");
    driver.start_blocking(1_000).expect("start");
    log_line(b"wifi core scan started");

    let options = ScanOptions::all()
        .show_hidden(true)
        .timings(ScanTimings::active(200, 500));

    let mut records: [AccessPointRecord; 16] =
        core::array::from_fn(|_| AccessPointRecord::default());
    match driver.scan_blocking(&options, &mut records, 15_000) {
        Ok(count) => {
            esp8266_hal::logger::write_bytes(b"wifi core scan done count=");
            esp8266_hal::logger::write_dec_u32(count as u32);
            esp8266_hal::logger::write_newline();
            for record in records[..count].iter() {
                esp8266_hal::logger::write_bytes(b"ap ssid=");
                esp8266_hal::logger::write_bytes(record.ssid());
                esp8266_hal::logger::write_bytes(b" bssid=");
                for (index, byte) in record.bssid().iter().enumerate() {
                    if index != 0 {
                        esp8266_hal::logger::write_byte(b':');
                    }
                    esp8266_hal::logger::write_hex_u8(*byte);
                }
                esp8266_hal::logger::write_bytes(b" ch=");
                esp8266_hal::logger::write_dec_u32(record.channel().get() as u32);
                esp8266_hal::logger::write_bytes(b" rssi=");
                if record.rssi() < 0 {
                    esp8266_hal::logger::write_byte(b'-');
                    esp8266_hal::logger::write_dec_u32(record.rssi().unsigned_abs() as u32);
                } else {
                    esp8266_hal::logger::write_dec_u32(record.rssi() as u32);
                }
                esp8266_hal::logger::write_bytes(b" auth=");
                esp8266_hal::logger::write_dec_u32(record.authentication().as_raw() as u32);
                esp8266_hal::logger::write_bytes(b" wps=");
                esp8266_hal::logger::write_dec_u32(record.wps() as u32);
                esp8266_hal::logger::write_newline();
            }
            log_line(b"scan-ok");
        }
        Err(error) => {
            esp8266_hal::logger::write_bytes(b"wifi core scan error ");
            support::log_debug(&error);
            esp8266_hal::logger::write_newline();
        }
    }

    loop {
        esp8266_hal::wifi::delay_ticks(100);
    }
}
