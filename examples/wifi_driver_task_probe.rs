//! Probe that keeps the current auto-connect path but spawns only the
//! embassy-net driver runner, not the DHCP/net stack task.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};

use esp8266_hal::println;
use esp8266_hal::wifi::controller::{WifiInitConfig, WifiInterface};
use esp8266_hal::wifi::device::WifiDevice;
use esp8266_hal::wifi::embassy_net::{self as wifi_net, NetDriverState};
use esp8266_hal::wifi::event::{self, WifiEvent};
use esp8266_hal::wifi::{AuthMode, StaConfig, WifiController, WifiError, WifiMode};
use xtensa_lx_rt::exception::{Context, ExceptionCause};

const WIFI_SSID: &str = include_str!("ssid");
const WIFI_PASS: &str = include_str!("pass");
const MAX_CONNECT_RETRIES: u32 = 10;

fn event_handler(event: WifiEvent) {
    match event {
        WifiEvent::StaStart => esp8266_hal::logger::write_byte(b's'),
        WifiEvent::StaConnected { .. } => esp8266_hal::logger::write_byte(b'g'),
        WifiEvent::StaDisconnected { .. } => esp8266_hal::logger::write_byte(b'd'),
        _ => {}
    }
}

#[inline(never)]
unsafe fn heap_leak_net_state() -> &'static mut NetDriverState {
    let align = core::mem::align_of::<NetDriverState>();
    let size = core::mem::size_of::<NetDriverState>();
    let total = size.saturating_add(align.saturating_sub(1));
    let raw = unsafe { esp8266_hal::wifi::compat::memory::malloc(total) as *mut u8 };
    assert!(!raw.is_null(), "heap allocation failed");
    let aligned = ((raw as usize) + (align - 1)) & !(align - 1);
    let ptr = aligned as *mut NetDriverState;
    unsafe { ptr.write(NetDriverState::new()) };
    unsafe { &mut *ptr }
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

    wifi.set_storage(esp8266_hal::wifi::Storage::Ram)
        .expect("set_storage");
    wifi.set_mode(WifiMode::Sta).expect("set_mode");
    let mut sta_cfg_builder = StaConfig::builder(WIFI_SSID, WIFI_PASS).pmf(false, false);
    if !WIFI_PASS.is_empty() {
        sta_cfg_builder = sta_cfg_builder.auth_threshold(AuthMode::Wpa2Psk);
    }
    let sta_cfg = sta_cfg_builder.build();
    wifi.set_sta_config(&sta_cfg).expect("set_sta_config");

    let mac = wifi.get_mac(WifiInterface::Sta).expect("get_mac");
    println!(
        "mac {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    );

    event::set_event_handler(event_handler).expect("set_event_handler");
    event::enable_sdk_sta_auto_connect(MAX_CONNECT_RETRIES).expect("enable_sdk_sta_auto_connect");
    wifi.start().expect("wifi start");
    println!("wifi started");

    (wifi, mac)
}

#[inline(never)]
async fn connect_wifi(wifi: &mut WifiController) -> Result<(), WifiError> {
    println!("wifi connect pre free=0x{:X}", free_heap());
    wifi.wait_for_auto_connect_result_blocking(5)?;
    println!("wifi connected free=0x{:X}", free_heap());
    Ok(())
}

#[embassy_executor::task]
async fn net_driver_task(mut runner: esp8266_hal::wifi::embassy_net::Runner<'static>) {
    runner.run().await;
}

#[esp8266_hal::main]
async fn main(spawner: Spawner) {
    for _ in 0..20_000_000u32 {
        core::hint::spin_loop();
    }

    println!("wifi_driver_task_probe start");

    let (mut wifi, mac) = init_wifi();
    if let Err(e) = connect_wifi(&mut wifi).await {
        println!("wifi connect err {:?}", e);
        loop {
            Timer::after(Duration::from_secs(60)).await;
        }
    }

    let _wifi_guard = wifi;
    let device = WifiDevice::new(WifiInterface::Sta);
    device.register_rx_callback().expect("register_rx_callback");
    println!("wifi rx callback ok");

    let net_state = unsafe { heap_leak_net_state() };
    let (driver_runner, _net_device) = wifi_net::new(net_state, mac, device);
    spawner.must_spawn(net_driver_task(driver_runner));
    println!("driver runner armed free=0x{:X}", free_heap());

    for i in 0..20u32 {
        Timer::after(Duration::from_millis(500)).await;
        println!("alive {}", i);
    }

    println!("PASSED");
    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    extern "C" {
        static g_net_runner_stage: u32;
    }

    esp8266_hal::logger::write_byte(b'?');
    esp8266_hal::logger::write_byte(esp8266_hal::wifi::compat::task::debug_current_task_marker());
    let net_stage = unsafe { g_net_runner_stage };
    if net_stage != 0 {
        esp8266_hal::logger::write_byte(net_stage as u8);
    }
    esp8266_hal::logger::write_byte(b'\n');
    loop {}
}

#[link_section = ".rwtext"]
#[xtensa_lx_rt::exception]
fn exception(cause: ExceptionCause, frame: Context) -> ! {
    extern "C" {
        static g_net_runner_stage: u32;
    }
    let _ = cause;

    let words = &frame as *const Context as *const u32;
    let a1 = unsafe { core::ptr::read(words.add(3)) };

    esp8266_hal::logger::write_byte(b'E');
    esp8266_hal::logger::write_byte(esp8266_hal::wifi::compat::task::debug_task_marker_for_sp(
        a1,
    ));
    esp8266_hal::logger::write_byte(esp8266_hal::wifi::compat::task::debug_current_task_marker());
    let net_stage = unsafe { g_net_runner_stage };
    if net_stage != 0 {
        esp8266_hal::logger::write_byte(net_stage as u8);
    }
    esp8266_hal::logger::write_byte(b'?');
    esp8266_hal::logger::write_byte(b'\n');

    loop {}
}
