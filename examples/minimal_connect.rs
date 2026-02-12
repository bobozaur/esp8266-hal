//! Small connect-only comparison probe.
//!
//! This keeps the same WiFi init/config and example-local event-driven
//! connect wait as `tcp_server.rs`, but intentionally does not create
//! `WifiDevice` or `embassy_net` state. It exists to separate:
//!
//! 1. generic station-connect/runtime failure
//! 2. failure that only appears once the larger network-stack image is linked

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::Duration;

use esp8266_hal::println;
use esp8266_hal::wifi::controller::{WifiInitConfig, WifiInterface};
use esp8266_hal::wifi::{
    AuthMode, StaConfig, Storage, WifiController, WifiError, WifiEvent, WifiMode,
};
use esp8266_wifi_sys as sys;
use xtensa_lx_rt::exception::{Context, ExceptionCause};

const WIFI_SSID: &str = include_str!("ssid");
const WIFI_PASS: &str = include_str!("pass");

fn hex_nibble(n: u8) -> u8 {
    match n & 0x0f {
        0..=9 => b'0' + (n & 0x0f),
        _ => b'a' + ((n & 0x0f) - 10),
    }
}

fn write_hex16(value: u16) {
    esp8266_hal::logger::write_byte(hex_nibble((value >> 12) as u8));
    esp8266_hal::logger::write_byte(hex_nibble((value >> 8) as u8));
    esp8266_hal::logger::write_byte(hex_nibble((value >> 4) as u8));
    esp8266_hal::logger::write_byte(hex_nibble(value as u8));
}

fn write_hex32(value: u32) {
    write_hex16((value >> 16) as u16);
    write_hex16(value as u16);
}

#[inline(never)]
fn free_heap() -> u32 {
    unsafe { esp8266_hal::wifi::compat::memory::esp_get_free_heap_size() }
}

#[inline(never)]
fn init_wifi() -> (WifiController, [u8; 6]) {
    let mut init_config = WifiInitConfig::default();
    init_config.rx_buf_num = 14;
    init_config.tx_buf_num = 4;

    let mut wifi = WifiController::init(init_config).expect("wifi init");
    println!("wifi init ok");

    wifi.set_storage(Storage::Ram).expect("set_storage");
    wifi.set_mode(WifiMode::Sta).expect("set_mode");

    let mac = wifi.get_mac(WifiInterface::Sta).expect("get_mac");
    println!(
        "mac {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    );

    wifi.start().expect("wifi start");
    println!("wifi started free=0x{:X}", free_heap());

    (wifi, mac)
}

#[inline(never)]
fn configure_sta_from_passive_scan(wifi: &mut WifiController) -> Result<(), WifiError> {
    let cfg = sys::wifi_scan_config_t {
        ssid: core::ptr::null_mut(),
        bssid: core::ptr::null_mut(),
        channel: 6,
        show_hidden: true,
        scan_type: sys::WIFI_SCAN_TYPE_PASSIVE,
        scan_time: sys::wifi_scan_time_t { passive: 500 },
    };

    unsafe { sys::esp_wifi_scan_start(&cfg, true) };

    let mut count: u16 = 0;
    unsafe { sys::esp_wifi_scan_get_ap_num(&mut count) };
    println!("passive scan count={} free=0x{:X}", count, free_heap());

    let mut recs: [sys::wifi_ap_record_t; 8] = unsafe { core::mem::zeroed() };
    let mut actual = count.min(recs.len() as u16);
    if actual != 0 {
        unsafe { sys::esp_wifi_scan_get_ap_records(&mut actual, recs.as_mut_ptr()) };
    }

    for rec in &recs[..actual as usize] {
        let ssid_end = rec
            .ssid
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(rec.ssid.len());
        if &rec.ssid[..ssid_end] != WIFI_SSID.as_bytes() {
            continue;
        }

        println!(
            "pin {} ch={} bssid={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} rssi={}",
            WIFI_SSID,
            rec.primary,
            rec.bssid[0],
            rec.bssid[1],
            rec.bssid[2],
            rec.bssid[3],
            rec.bssid[4],
            rec.bssid[5],
            rec.rssi
        );

        let mut sta_cfg_builder = StaConfig::builder(WIFI_SSID, WIFI_PASS)
            .pmf(false, false)
            .bssid(rec.bssid)
            .channel(rec.primary);
        if !WIFI_PASS.is_empty() {
            sta_cfg_builder = sta_cfg_builder.auth_threshold(AuthMode::Wpa2Psk);
        }
        let sta_cfg = sta_cfg_builder.build();
        wifi.set_sta_config(&sta_cfg).expect("set_sta_config");
        return Ok(());
    }

    println!("target ssid not found in passive scan");
    Err(WifiError::Internal(esp8266_hal::wifi::EspError(
        sys::ESP_ERR_NOT_FOUND,
    )))
}

#[inline(never)]
async fn connect_wifi(wifi: &mut WifiController) -> Result<(), WifiError> {
    println!("wifi connect pre free=0x{:X}", free_heap());
    let mut sub = esp8266_hal::wifi::event::event_subscriber().expect("event_subscriber");
    wifi.connect()?;

    loop {
        match sub.next_message_pure().await {
            WifiEvent::StaConnected { ssid, channel, .. } => {
                println!(
                    "wifi connected ssid={} ch={} free=0x{:X}",
                    core::str::from_utf8(&ssid).unwrap_or("?"),
                    channel,
                    free_heap()
                );
                return Ok(());
            }
            WifiEvent::StaDisconnected { reason, .. } => {
                println!("wifi disconnect {:?} free=0x{:X}", reason, free_heap());
                let _ = wifi.connect();
            }
            _ => {}
        }
    }
}

#[esp8266_hal::main]
async fn main(_spawner: Spawner) {
    for _ in 0..20_000_000u32 {
        core::hint::spin_loop();
    }

    println!("minimal_connect start");

    let (mut wifi, _mac) = init_wifi();
    match configure_sta_from_passive_scan(&mut wifi) {
        Ok(()) => println!("pin cfg ok"),
        Err(e) => {
            println!("pin cfg err {:?}", e);
            loop {
                embassy_time::Timer::after(Duration::from_secs(60)).await;
            }
        }
    }

    match connect_wifi(&mut wifi).await {
        Ok(()) => println!("minimal_connect ok"),
        Err(e) => println!("minimal_connect err {:?}", e),
    }

    loop {
        embassy_time::Timer::after(Duration::from_secs(60)).await;
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    esp8266_hal::logger::write_byte(b'P');
    esp8266_hal::logger::write_byte(b'C');
    esp8266_hal::logger::write_byte(esp8266_hal::wifi::compat::task::debug_current_task_marker());
    if let Some(location) = info.location() {
        let path = location.file().as_bytes();
        let mut start = 0usize;
        let mut end = path.len();
        for (idx, &byte) in path.iter().enumerate() {
            if byte == b'/' {
                start = idx + 1;
                end = path.len();
            } else if byte == b'.' {
                end = idx;
            }
        }
        let stop = core::cmp::min(start + 6, end);
        for &byte in &path[start..stop] {
            esp8266_hal::logger::write_byte(byte);
        }
        esp8266_hal::logger::write_byte(b':');
        write_hex16(location.line() as u16);
    } else {
        esp8266_hal::logger::write_byte(b'n');
        esp8266_hal::logger::write_byte(b'o');
        esp8266_hal::logger::write_byte(b'l');
        esp8266_hal::logger::write_byte(b'o');
        esp8266_hal::logger::write_byte(b'c');
    }
    esp8266_hal::logger::write_byte(b'?');
    loop {}
}

#[link_section = ".rwtext"]
#[xtensa_lx_rt::exception]
fn exception(cause: ExceptionCause, frame: Context) -> ! {
    extern "C" {
        static g_ui_zero_save_pc: u32;
        static g_wait_stage: u32;
        static g_wait_task: u32;
    }

    let words = &frame as *const Context as *const u32;
    let pc = unsafe { core::ptr::read(words) };
    let a0 = unsafe { core::ptr::read(words.add(2)) };
    let a1 = unsafe { core::ptr::read(words.add(3)) };

    esp8266_hal::logger::write_byte(b'E');
    write_hex16(cause as u32 as u16);
    esp8266_hal::logger::write_byte(b'P');
    write_hex32(pc);
    esp8266_hal::logger::write_byte(b'A');
    write_hex32(a0);
    esp8266_hal::logger::write_byte(b'O');
    esp8266_hal::logger::write_byte(esp8266_hal::wifi::compat::task::debug_task_marker_for_sp(
        a1,
    ));
    esp8266_hal::logger::write_byte(b'C');
    esp8266_hal::logger::write_byte(esp8266_hal::wifi::compat::task::debug_current_task_marker());
    let wait_stage = unsafe { g_wait_stage };
    if wait_stage != 0 {
        esp8266_hal::logger::write_byte(b'W');
        esp8266_hal::logger::write_byte(wait_stage as u8);
        let wait_task = unsafe { g_wait_task };
        esp8266_hal::logger::write_byte(wait_task as u8);
    }
    let save_pc = unsafe { g_ui_zero_save_pc };
    if save_pc != 0 {
        esp8266_hal::logger::write_byte(b'Q');
        write_hex32(save_pc);
    }
    esp8266_hal::logger::write_byte(b'S');
    write_hex32(a1);
    esp8266_hal::logger::write_byte(b'\n');
    loop {}
}
