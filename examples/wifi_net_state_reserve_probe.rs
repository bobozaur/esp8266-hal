//! Probe that reserves NetDriverState memory before connect and initializes it after connect.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::Duration;

use esp8266_hal::println;
use esp8266_hal::wifi::controller::{WifiInitConfig, WifiInterface};
use esp8266_hal::wifi::device::WifiDevice;
use esp8266_hal::wifi::embassy_net::NetDriverState;
use esp8266_hal::wifi::{AuthMode, StaConfig, WifiController, WifiMode};

const WIFI_SSID: &str = include_str!("ssid");
const WIFI_PASS: &str = include_str!("pass");

#[inline(never)]
unsafe fn reserve_net_state() -> *mut NetDriverState {
    let align = core::mem::align_of::<NetDriverState>();
    let size = core::mem::size_of::<NetDriverState>();
    let total = size.saturating_add(align.saturating_sub(1));
    let raw = unsafe { esp8266_hal::wifi::compat::memory::malloc(total) as *mut u8 };
    assert!(!raw.is_null(), "heap allocation failed");
    let aligned = ((raw as usize) + (align - 1)) & !(align - 1);
    aligned as *mut NetDriverState
}

#[esp8266_hal::main]
async fn main(_spawner: Spawner) {
    for _ in 0..20_000_000u32 {
        core::hint::spin_loop();
    }

    println!("wifi_net_state_reserve_probe start");

    let mut init_config = WifiInitConfig::default();
    init_config.rx_buf_num = 14;
    init_config.tx_buf_num = 4;

    let mut wifi = WifiController::init(init_config).expect("wifi init");
    println!("wifi init ok");

    wifi.set_mode(WifiMode::Sta).expect("set_mode");
    let sta_cfg = StaConfig::builder(WIFI_SSID, WIFI_PASS)
        .auth_threshold(AuthMode::Open)
        .build();
    wifi.set_sta_config(&sta_cfg).expect("set_sta_config");

    let mac = wifi.get_mac(WifiInterface::Sta).expect("get_mac");
    println!(
        "mac {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    );

    let net_state = unsafe { reserve_net_state() };
    println!("reserve ok");

    wifi.start().expect("wifi start");
    println!("wifi started");

    match wifi.connect_async().await {
        Ok(()) => println!("wifi connected"),
        Err(e) => {
            println!("wifi connect err {:?}", e);
            loop {
                embassy_time::Timer::after(Duration::from_secs(60)).await;
            }
        }
    }

    let device = WifiDevice::new(WifiInterface::Sta);
    device.register_rx_callback().expect("register_rx_callback");
    println!("wifi rx callback ok");

    unsafe { net_state.write(NetDriverState::new()) };
    println!("net state init ok");

    loop {
        embassy_time::Timer::after(Duration::from_secs(60)).await;
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    esp8266_hal::logger::write_byte(b'?');
    esp8266_hal::logger::write_byte(b'\n');
    loop {}
}
