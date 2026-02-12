#![no_std]
#![no_main]

use embassy_executor::Spawner;
use esp8266_hal::println;
use esp8266_hal::wifi::controller::WifiInitConfig;
use esp8266_hal::wifi::{AuthMode, StaConfig, Storage, WifiController, WifiEvent, WifiMode};
use esp8266_wifi_sys as sys;

const WIFI_SSID: &str = include_str!("ssid");
const WIFI_PASS: &str = include_str!("pass");

#[esp8266_hal::main]
async fn main(_spawner: Spawner) {
    for _ in 0..20_000_000u32 {
        core::hint::spin_loop();
    }

    println!("scan_pin_connect");

    let mut wifi = WifiController::init(WifiInitConfig::default()).expect("wifi");
    wifi.set_storage(Storage::Ram).expect("storage");
    wifi.set_mode(WifiMode::Sta).expect("mode");
    wifi.start().expect("start");

    let scan_cfg = sys::wifi_scan_config_t {
        ssid: core::ptr::null_mut(),
        bssid: core::ptr::null_mut(),
        channel: 6,
        show_hidden: true,
        scan_type: sys::WIFI_SCAN_TYPE_PASSIVE,
        scan_time: sys::wifi_scan_time_t { passive: 500 },
    };

    unsafe { sys::esp_wifi_scan_start(&scan_cfg, true) };
    let mut count: u16 = 0;
    unsafe { sys::esp_wifi_scan_get_ap_num(&mut count) };
    println!("passive count={}", count);

    let mut recs: [sys::wifi_ap_record_t; 8] = unsafe { core::mem::zeroed() };
    let mut actual = count.min(recs.len() as u16);
    if actual != 0 {
        unsafe { sys::esp_wifi_scan_get_ap_records(&mut actual, recs.as_mut_ptr()) };
    }

    let mut target = None;
    for rec in &recs[..actual as usize] {
        let ssid_end = rec
            .ssid
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(rec.ssid.len());
        if &rec.ssid[..ssid_end] != WIFI_SSID.as_bytes() {
            continue;
        }
        target = Some((rec.bssid, rec.primary, rec.rssi));
        break;
    }

    let Some((bssid, channel, rssi)) = target else {
        println!("target not found");
        loop {
            embassy_time::Timer::after(embassy_time::Duration::from_secs(60)).await;
        }
    };

    println!(
        "pin ch={} rssi={} bssid={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        channel, rssi, bssid[0], bssid[1], bssid[2], bssid[3], bssid[4], bssid[5]
    );

    let mut sta_cfg_builder = StaConfig::builder(WIFI_SSID, WIFI_PASS)
        .pmf(false, false)
        .bssid(bssid)
        .channel(channel);
    if !WIFI_PASS.is_empty() {
        sta_cfg_builder = sta_cfg_builder.auth_threshold(AuthMode::Wpa2Psk);
    }
    let sta_cfg = sta_cfg_builder.build();
    wifi.set_sta_config(&sta_cfg).expect("sta cfg");

    let mut sub = esp8266_hal::wifi::event::event_subscriber().expect("event_subscriber");
    wifi.connect().expect("connect");
    println!("connect start");

    loop {
        match sub.next_message_pure().await {
            WifiEvent::StaConnected { ssid, channel, .. } => {
                println!(
                    "connected ssid={} ch={}",
                    core::str::from_utf8(&ssid).unwrap_or("?"),
                    channel
                );
                break;
            }
            WifiEvent::StaDisconnected { reason, .. } => {
                println!("disconnect {:?}", reason);
            }
            _ => {}
        }
    }

    println!("done");
    loop {
        embassy_time::Timer::after(embassy_time::Duration::from_secs(60)).await;
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    esp8266_hal::logger::write_byte(b'?');
    loop {}
}
