#![no_std]
#![no_main]

use embassy_executor::Spawner;
use esp8266_hal::println;
use esp8266_hal::wifi::controller::WifiInitConfig;
use esp8266_hal::wifi::{WifiController, WifiMode};
use esp8266_wifi_sys as sys;
use xtensa_lx_rt::exception::{Context, ExceptionCause};

#[esp8266_hal::main]
async fn main(_spawner: Spawner) {
    for _ in 0..20_000_000u32 {
        core::hint::spin_loop();
    }

    println!("scan_ch6_list");
    let mut wifi = WifiController::init(WifiInitConfig::default()).expect("wifi");
    wifi.set_mode(WifiMode::Sta).expect("mode");
    wifi.start().expect("start");

    for (mode, name) in [
        (sys::WIFI_SCAN_TYPE_ACTIVE, "active"),
        (sys::WIFI_SCAN_TYPE_PASSIVE, "passive"),
    ] {
        let cfg = sys::wifi_scan_config_t {
            ssid: core::ptr::null_mut(),
            bssid: core::ptr::null_mut(),
            channel: 6,
            show_hidden: true,
            scan_type: mode,
            scan_time: if mode == sys::WIFI_SCAN_TYPE_PASSIVE {
                sys::wifi_scan_time_t { passive: 500 }
            } else {
                sys::wifi_scan_time_t {
                    active: sys::wifi_active_scan_time_t { min: 200, max: 500 },
                }
            },
        };
        unsafe { sys::esp_wifi_scan_start(&cfg, true) };
        let mut count: u16 = 0;
        unsafe { sys::esp_wifi_scan_get_ap_num(&mut count) };
        println!("ch6 {}: {} APs", name, count);
        if count > 0 {
            let mut recs: [sys::wifi_ap_record_t; 8] = unsafe { core::mem::zeroed() };
            let mut n = count.min(recs.len() as u16);
            unsafe { sys::esp_wifi_scan_get_ap_records(&mut n, recs.as_mut_ptr()) };
            for rec in &recs[..n as usize] {
                let sl = rec.ssid.iter().position(|&b| b == 0).unwrap_or(32);
                let s = core::str::from_utf8(&rec.ssid[..sl]).unwrap_or("?");
                println!(
                    "  {} ch={} rssi={} bssid={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                    s,
                    rec.primary,
                    rec.rssi,
                    rec.bssid[0],
                    rec.bssid[1],
                    rec.bssid[2],
                    rec.bssid[3],
                    rec.bssid[4],
                    rec.bssid[5],
                );
            }
        }
    }

    println!("done");
    loop {
        embassy_time::Timer::after(embassy_time::Duration::from_secs(60)).await;
    }
}

fn hex_nibble(n: u8) -> u8 {
    match n & 0x0f {
        0..=9 => b'0' + (n & 0x0f),
        _ => b'a' + ((n & 0x0f) - 10),
    }
}

fn write_hex32(value: u32) {
    for shift in (0..8).rev() {
        esp8266_hal::logger::write_byte(hex_nibble((value >> (shift * 4)) as u8));
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    esp8266_hal::logger::write_byte(b'?');
    esp8266_hal::logger::write_byte(b'\n');
    loop {}
}

#[link_section = ".rwtext"]
#[xtensa_lx_rt::exception]
fn exception(cause: ExceptionCause, frame: Context) -> ! {
    let words = &frame as *const Context as *const u32;
    let pc = unsafe { core::ptr::read(words) };
    let a0 = unsafe { core::ptr::read(words.add(2)) };
    let a1 = unsafe { core::ptr::read(words.add(3)) };

    esp8266_hal::logger::write_byte(b'E');
    write_hex32(cause as u32);
    esp8266_hal::logger::write_byte(b'P');
    write_hex32(pc);
    esp8266_hal::logger::write_byte(b'A');
    write_hex32(a0);
    esp8266_hal::logger::write_byte(b'S');
    write_hex32(a1);
    esp8266_hal::logger::write_byte(b'\n');

    loop {}
}
