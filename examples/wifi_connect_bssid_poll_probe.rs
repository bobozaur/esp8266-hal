#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::Duration;

use esp8266_hal::println;
use esp8266_hal::wifi::controller::{current_sta_state, StaConnectionState, WifiInitConfig};
use esp8266_hal::wifi::{AuthMode, PowerSave, StaConfig, Storage, WifiController, WifiMode};
use xtensa_lx_rt::exception::{Context, ExceptionCause};

const WIFI_SSID: &str = include_str!("ssid");
const WIFI_PASS: &str = include_str!("pass");
const WIFI_BSSID: [u8; 6] = [0x28, 0xEE, 0x52, 0xAF, 0xF6, 0x1F];
const WIFI_CHANNEL: u8 = 6;

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

fn write_hex8(value: u8) {
    esp8266_hal::logger::write_byte(hex_nibble(value >> 4));
    esp8266_hal::logger::write_byte(hex_nibble(value));
}

fn write_hex32(value: u32) {
    write_hex16((value >> 16) as u16);
    write_hex16(value as u16);
}

#[inline(never)]
fn write_profile_summary() {
    extern "C" {
        static g_wifi_nvs: *const u8;
    }

    unsafe {
        let profile = g_wifi_nvs;
        if profile.is_null() {
            return;
        }

        esp8266_hal::logger::write_byte(b'N');
        write_hex8(core::ptr::read(profile.add(4)));
        write_hex8(core::ptr::read(profile.add(0x90)));
        write_hex8(core::ptr::read(profile.add(0x92)));
        for offset in 0..6usize {
            write_hex8(core::ptr::read(profile.add(0x93 + offset)));
        }
    }
}

fn write_power_save_marker(prefix: u8, ps: PowerSave) {
    esp8266_hal::logger::write_byte(prefix);
    let raw = match ps {
        PowerSave::None => 0u8,
        PowerSave::MinModem => 1u8,
        PowerSave::MaxModem => 2u8,
    };
    write_hex8(raw);
}

#[inline(never)]
fn write_stage_hits() {
    extern "C" {
        static g_pp_stage_hits: u32;
        static g_scan_parse_beacon_calls: u32;
        static g_scan_parse_beacon_returns: u32;
        static g_scan_parse_beacon_bssid_match_calls: u32;
        static g_ieee80211_match_security_calls: u32;
        static g_ieee80211_send_probereq_calls: u32;
        static g_scan_remove_bssid_calls: u32;
        static g_scan_add_probe_ssid_calls: u32;
        static g_scan_add_probe_ssid_len: u32;
        static g_scan_add_probe_ssid_flag: u32;
        static g_scan_prefer_chan_calls: u32;
        static g_scan_prefer_chan_last_ret: u32;
        static g_scan_build_chan_list_calls: u32;
        static g_scan_build_chan_list_last_mode: u32;
        static g_scan_build_chan_list_last_chan: u32;
        static g_scan_hidden_ssid_calls: u32;
        static g_cnx_rc_search_calls: u32;
        static g_cnx_bss_alloc_calls: u32;
        static g_cnx_bss_alloc_accepts: u32;
        static g_cnx_add_rc_calls: u32;
    }

    let hits = unsafe { g_pp_stage_hits };
    if hits != 0 {
        esp8266_hal::logger::write_byte(b'B');
        write_hex32(hits);
    }
    let scan_calls = unsafe { g_scan_parse_beacon_calls };
    let scan_returns = unsafe { g_scan_parse_beacon_returns };
    let scan_bssid_matches = unsafe { g_scan_parse_beacon_bssid_match_calls };
    let match_calls = unsafe { g_ieee80211_match_security_calls };
    if scan_calls != 0 || scan_returns != 0 || scan_bssid_matches != 0 || match_calls != 0 {
        esp8266_hal::logger::write_byte(b'X');
        write_hex32(scan_calls);
        write_hex32(scan_returns);
        write_hex32(match_calls);
        write_hex32(scan_bssid_matches);
    }
    let probe_req_calls = unsafe { g_ieee80211_send_probereq_calls };
    if probe_req_calls != 0 {
        esp8266_hal::logger::write_byte(b'Y');
        write_hex32(probe_req_calls);
    }
    let remove_bssid_calls = unsafe { g_scan_remove_bssid_calls };
    let add_probe_ssid_calls = unsafe { g_scan_add_probe_ssid_calls };
    let add_probe_ssid_len = unsafe { g_scan_add_probe_ssid_len };
    let add_probe_ssid_flag = unsafe { g_scan_add_probe_ssid_flag };
    let prefer_chan_calls = unsafe { g_scan_prefer_chan_calls };
    let prefer_chan_last_ret = unsafe { g_scan_prefer_chan_last_ret };
    let build_chan_list_calls = unsafe { g_scan_build_chan_list_calls };
    let build_chan_list_last_mode = unsafe { g_scan_build_chan_list_last_mode };
    let build_chan_list_last_chan = unsafe { g_scan_build_chan_list_last_chan };
    let hidden_ssid_calls = unsafe { g_scan_hidden_ssid_calls };
    if remove_bssid_calls != 0
        || add_probe_ssid_calls != 0
        || prefer_chan_calls != 0
        || build_chan_list_calls != 0
        || hidden_ssid_calls != 0
    {
        esp8266_hal::logger::write_byte(b'Z');
        write_hex32(remove_bssid_calls);
        write_hex32(add_probe_ssid_calls);
        write_hex32(add_probe_ssid_len);
        write_hex32(add_probe_ssid_flag);
        write_hex32(prefer_chan_calls);
        write_hex32(prefer_chan_last_ret);
        write_hex32(build_chan_list_calls);
        write_hex32(build_chan_list_last_mode);
        write_hex32(build_chan_list_last_chan);
        write_hex32(hidden_ssid_calls);
    }
    let rc_search_calls = unsafe { g_cnx_rc_search_calls };
    let alloc_calls = unsafe { g_cnx_bss_alloc_calls };
    let alloc_accepts = unsafe { g_cnx_bss_alloc_accepts };
    let add_rc_calls = unsafe { g_cnx_add_rc_calls };
    if rc_search_calls != 0 || alloc_calls != 0 || alloc_accepts != 0 || add_rc_calls != 0 {
        esp8266_hal::logger::write_byte(b'R');
        write_hex32(rc_search_calls);
        write_hex32(alloc_calls);
        write_hex32(alloc_accepts);
        write_hex32(add_rc_calls);
    }
}

#[esp8266_hal::main]
async fn main(_spawner: Spawner) {
    for _ in 0..20_000_000u32 {
        core::hint::spin_loop();
    }

    println!("wifi_connect_bssid_poll_probe start");

    let mut init_config = WifiInitConfig::default();
    init_config.rx_buf_num = 14;
    init_config.tx_buf_num = 4;

    let mut wifi = WifiController::init(init_config).expect("wifi init");
    println!("wifi init ok");

    wifi.set_storage(Storage::Ram).expect("set_storage");
    wifi.set_mode(WifiMode::Sta).expect("set_mode");

    let mut sta_cfg_builder = StaConfig::builder(WIFI_SSID, WIFI_PASS)
        .pmf(false, false)
        .bssid(WIFI_BSSID)
        .channel(WIFI_CHANNEL);
    if !WIFI_PASS.trim().is_empty() {
        sta_cfg_builder = sta_cfg_builder.auth_threshold(AuthMode::Wpa2Psk);
    }
    let sta_cfg = sta_cfg_builder.build();
    wifi.set_sta_config(&sta_cfg).expect("set_sta_config");

    wifi.start().expect("wifi start");
    println!("wifi started pinned");
    if let Ok(ps) = wifi.get_power_save() {
        write_power_save_marker(b'W', ps);
    }
    wifi.set_power_save(PowerSave::None)
        .expect("set_power_save");
    if let Ok(ps) = wifi.get_power_save() {
        write_power_save_marker(b'w', ps);
    }

    esp8266_hal::wifi::compat::wifi_os_adapter::arm_pp_sig_trace(8);
    wifi.connect().expect("wifi connect");
    esp8266_hal::logger::write_byte(b'c');

    for _ in 0..200u32 {
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
    write_stage_hits();
    write_profile_summary();
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
    write_stage_hits();
    let save_pc = unsafe { g_ui_zero_save_pc };
    if save_pc != 0 {
        esp8266_hal::logger::write_byte(b'Q');
        write_hex32(save_pc);
    }
    write_profile_summary();
    esp8266_hal::logger::write_byte(b'S');
    write_hex32(a1);
    esp8266_hal::logger::write_byte(b'\n');

    loop {}
}
