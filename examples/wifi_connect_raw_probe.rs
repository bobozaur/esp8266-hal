//! Probe that bypasses `WifiController::start()` to avoid registering the Rust event trampoline.
//!
//! The goal is to test whether the immediate post-`esp_wifi_connect()` crash still occurs
//! when WiFi is started through the raw SDK API and no local event state tracking is used.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::Duration;

use esp8266_hal::println;
use esp8266_hal::wifi::controller::WifiInitConfig;
use esp8266_hal::wifi::{AuthMode, EspError, StaConfig, Storage, WifiController, WifiMode};
use esp8266_wifi_sys as sys;
use xtensa_lx_rt::exception::{Context, ExceptionCause};

const WIFI_SSID: &str = include_str!("ssid");
const WIFI_PASS: &str = include_str!("pass");
const USE_CONTROLLER_START: bool = true;

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

    println!("wifi_connect_raw_probe start");

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

    if USE_CONTROLLER_START {
        wifi.start().expect("wifi start");
        esp8266_hal::logger::write_byte(b'S');
    } else {
        EspError::check(unsafe { sys::esp_wifi_start() }).expect("raw wifi start");
        esp8266_hal::logger::write_byte(b's');
    }

    wifi.connect().expect("wifi connect");
    esp8266_hal::logger::write_byte(b'c');
    esp8266_hal::wifi::compat::wifi_os_adapter::arm_pp_sig_trace(8);

    for _ in 0..60u32 {
        unsafe { esp8266_hal::wifi::compat::task::vTaskDelay(10) };
    }

    esp8266_hal::logger::write_byte(b'Z');
    println!("survived");

    loop {
        embassy_time::Timer::after(Duration::from_secs(60)).await;
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    extern "C" {
        static g_ui_zero_save_pc: u32;
        static g_pp_stage: u32;
        static g_wait_stage: u32;
        static g_wait_task: u32;
        static g_ctx_stage: u32;
        static g_queue_recv_stage: u32;
        static g_last_saved_ret_slot: u32;
    }

    esp8266_hal::logger::write_byte(b'P');
    esp8266_hal::logger::write_byte(b'C');
    esp8266_hal::logger::write_byte(esp8266_hal::wifi::compat::task::debug_current_task_marker());
    let pp_stage = unsafe { g_pp_stage };
    if pp_stage != 0 {
        esp8266_hal::logger::write_byte(b'G');
        esp8266_hal::logger::write_byte(pp_stage as u8);
    }
    let wait_stage = unsafe { g_wait_stage };
    if wait_stage != 0 {
        esp8266_hal::logger::write_byte(b'W');
        esp8266_hal::logger::write_byte(wait_stage as u8);
        let wait_task = unsafe { g_wait_task };
        esp8266_hal::logger::write_byte(wait_task as u8);
    }
    let ctx_stage = unsafe { g_ctx_stage };
    if ctx_stage != 0 {
        esp8266_hal::logger::write_byte(b'H');
        esp8266_hal::logger::write_byte(ctx_stage as u8);
    }
    let queue_recv_stage = unsafe { g_queue_recv_stage };
    if queue_recv_stage != 0 {
        esp8266_hal::logger::write_byte(b'J');
        esp8266_hal::logger::write_byte(queue_recv_stage as u8);
    }
    let save_pc = unsafe { g_ui_zero_save_pc };
    if save_pc != 0 {
        esp8266_hal::logger::write_byte(b'Q');
        write_hex32(save_pc);
    }
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
        static g_wait_stage: u32;
        static g_wait_task: u32;
        static g_ctx_stage: u32;
        static g_queue_recv_stage: u32;
        static g_last_saved_ret_slot: u32;
    }

    let words = &frame as *const Context as *const u32;
    let pc = unsafe { core::ptr::read(words) };
    let exc_cause = unsafe { core::ptr::read(words.add(19)) };
    let exc_vaddr = unsafe { core::ptr::read(words.add(20)) };

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
    let wait_stage = unsafe { g_wait_stage };
    if wait_stage != 0 {
        esp8266_hal::logger::write_byte(b'W');
        esp8266_hal::logger::write_byte(wait_stage as u8);
        let wait_task = unsafe { g_wait_task };
        esp8266_hal::logger::write_byte(wait_task as u8);
    }
    let ctx_stage = unsafe { g_ctx_stage };
    if ctx_stage != 0 {
        esp8266_hal::logger::write_byte(b'H');
        esp8266_hal::logger::write_byte(ctx_stage as u8);
    }
    let queue_recv_stage = unsafe { g_queue_recv_stage };
    if queue_recv_stage != 0 {
        esp8266_hal::logger::write_byte(b'J');
        esp8266_hal::logger::write_byte(queue_recv_stage as u8);
    }
    let save_pc = unsafe { g_ui_zero_save_pc };
    if save_pc != 0 {
        esp8266_hal::logger::write_byte(b'Q');
        write_hex32(save_pc);
    }
    let ret_slot = unsafe { g_last_saved_ret_slot };
    if ret_slot != 0 {
        esp8266_hal::logger::write_byte(b'R');
        write_hex32(ret_slot);
    }
    esp8266_hal::logger::write_byte(b'S');
    write_hex32(a1);
    esp8266_hal::logger::write_byte(b'\n');

    loop {}
}
