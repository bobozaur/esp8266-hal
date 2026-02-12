//! Probe the event-trampoline start path with task-delay polling instead of
//! the old `ets_delay_us` busy wait.
//!
//! This keeps WiFi bring-up in example space and mirrors the SDK station helper's
//! reconnect-on-disconnect behavior closely enough to separate:
//! 1. raw WiFi connect-path instability
//! 2. instability introduced by the event-trampoline / local state-tracking path

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::Duration;

use esp8266_hal::println;
use esp8266_hal::wifi::controller::{current_sta_state, StaConnectionState, WifiInitConfig};
use esp8266_hal::wifi::{AuthMode, StaConfig, Storage, WifiController, WifiMode};
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

#[esp8266_hal::main]
async fn main(_spawner: Spawner) {
    for _ in 0..20_000_000u32 {
        core::hint::spin_loop();
    }

    println!("wifi_connect_poll_probe start");

    let mut init_config = WifiInitConfig::default();
    init_config.rx_buf_num = 14;
    init_config.tx_buf_num = 4;

    let mut wifi = WifiController::init(init_config).expect("wifi init");
    println!("wifi init ok");

    wifi.set_storage(Storage::Ram).expect("set_storage");
    wifi.set_mode(WifiMode::Sta).expect("set_mode");
    let mut sta_cfg_builder = StaConfig::builder(WIFI_SSID, WIFI_PASS).pmf(false, false);
    if !WIFI_PASS.trim().is_empty() {
        sta_cfg_builder = sta_cfg_builder.auth_threshold(AuthMode::Wpa2Psk);
    }
    let sta_cfg = sta_cfg_builder.build();
    wifi.set_sta_config(&sta_cfg).expect("set_sta_config");

    wifi.start().expect("wifi start");
    println!("wifi started");

    esp8266_hal::wifi::compat::wifi_os_adapter::arm_pp_sig_trace(4);
    wifi.connect().expect("wifi connect");
    esp8266_hal::logger::write_byte(b'c');

    for _ in 0..120u32 {
        unsafe { esp8266_hal::wifi::compat::task::vTaskDelay(5) };
        match current_sta_state() {
            StaConnectionState::Connected => {
                esp8266_hal::logger::write_byte(b'C');
                println!("connected");
                loop {
                    embassy_time::Timer::after(Duration::from_secs(60)).await;
                }
            }
            StaConnectionState::Disconnected => {
                esp8266_hal::logger::write_byte(b'D');
                let _ = wifi.connect();
            }
            StaConnectionState::Connecting => {}
        }
    }

    esp8266_hal::logger::write_byte(b'T');
    println!("timeout");

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
    esp8266_hal::logger::write_byte(b'\n');
    loop {}
}

#[link_section = ".rwtext"]
#[xtensa_lx_rt::exception]
fn exception(cause: ExceptionCause, frame: Context) -> ! {
    extern "C" {
        static g_ui_zero_save_pc: u32;
        static g_pp_stage: u32;
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
    let pp_stage = unsafe { g_pp_stage };
    if pp_stage != 0 {
        esp8266_hal::logger::write_byte(b'G');
        esp8266_hal::logger::write_byte(pp_stage as u8);
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
