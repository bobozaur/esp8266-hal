#![no_std]
#![no_main]

use embassy_executor::Spawner;
use esp8266_hal::println;
use esp8266_hal::wifi::controller::WifiInitConfig;
use esp8266_hal::wifi::{Storage, WifiController, WifiMode};
use esp8266_wifi_sys as sys;
use xtensa_lx_rt::exception::{Context, ExceptionCause};

const WIFI_SSID: &str = include_str!("ssid");
const WIFI_PASS: &str = include_str!("pass");

#[esp8266_hal::main]
async fn main(_spawner: Spawner) {
    for _ in 0..20_000_000u32 {
        core::hint::spin_loop();
    }

    println!("scan_raw_cfg_ram_directed");

    let mut wifi = WifiController::init(WifiInitConfig::default()).expect("wifi");
    wifi.set_storage(Storage::Ram).expect("storage");
    wifi.set_mode(WifiMode::Sta).expect("mode");

    let mut raw: sys::wifi_config_t = unsafe { core::mem::zeroed() };
    let ssid = WIFI_SSID.as_bytes();
    let pass = WIFI_PASS.as_bytes();
    let ssid_len = ssid.len().min(32);
    let pass_len = pass.len().min(64);
    unsafe {
        let sta = &mut raw.sta;
        sta.ssid[..ssid_len].copy_from_slice(&ssid[..ssid_len]);
        sta.password[..pass_len].copy_from_slice(&pass[..pass_len]);
    }

    let set_ret = unsafe { sys::esp_wifi_set_config(sys::WIFI_IF_STA, &mut raw) };
    println!("set_config ret={}", set_ret);

    let mut got: sys::wifi_config_t = unsafe { core::mem::zeroed() };
    let get_ret = unsafe { sys::esp_wifi_get_config(sys::WIFI_IF_STA, &mut got) };
    println!(
        "get_config ret={} sm={} sort={} rssi={} auth={} pmf={}/{} ch={} bssid_set={}",
        get_ret,
        unsafe { got.sta.scan_method },
        unsafe { got.sta.sort_method },
        unsafe { got.sta.threshold.rssi },
        unsafe { got.sta.threshold.authmode },
        unsafe { got.sta.pmf_cfg.capable as u8 },
        unsafe { got.sta.pmf_cfg.required as u8 },
        unsafe { got.sta.channel },
        unsafe { got.sta.bssid_set as u8 },
    );

    wifi.start().expect("start");
    println!("start ok");

    let mut ssid_buf = [0u8; 33];
    ssid_buf[..ssid_len].copy_from_slice(&ssid[..ssid_len]);

    let cfg = sys::wifi_scan_config_t {
        ssid: ssid_buf.as_mut_ptr(),
        bssid: core::ptr::null_mut(),
        channel: 6,
        show_hidden: true,
        scan_type: sys::WIFI_SCAN_TYPE_ACTIVE,
        scan_time: sys::wifi_scan_time_t {
            active: sys::wifi_active_scan_time_t { min: 200, max: 500 },
        },
    };

    unsafe { sys::esp_wifi_scan_start(&cfg, true) };
    let mut count: u16 = 0;
    unsafe { sys::esp_wifi_scan_get_ap_num(&mut count) };
    println!("SCAN ch6 raw+ssid: {} APs", count);

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
