//! Minimal heap/construction probe for embassy-net WiFi bring-up.
//!
//! This follows the same post-connect object construction path as
//! `tcp_server`, but stops before any embassy executor task spawn.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::Duration;

use esp8266_hal::println;
use esp8266_hal::wifi::controller::{WifiInitConfig, WifiInterface};
use esp8266_hal::wifi::device::WifiDevice;
use esp8266_hal::wifi::embassy_net::{self as wifi_net, NetDriverState};
use esp8266_hal::wifi::{AuthMode, StaConfig, WifiController, WifiMode};

const WIFI_SSID: &str = include_str!("ssid");
const WIFI_PASS: &str = include_str!("pass");

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
unsafe fn heap_leak_stack_resources() -> &'static mut embassy_net::StackResources<2> {
    let align = core::mem::align_of::<embassy_net::StackResources<2>>();
    let size = core::mem::size_of::<embassy_net::StackResources<2>>();
    let total = size.saturating_add(align.saturating_sub(1));
    let raw = unsafe { esp8266_hal::wifi::compat::memory::malloc(total) as *mut u8 };
    assert!(!raw.is_null(), "heap allocation failed");
    let aligned = ((raw as usize) + (align - 1)) & !(align - 1);
    let ptr = aligned as *mut embassy_net::StackResources<2>;
    unsafe { ptr.write(embassy_net::StackResources::<2>::new()) };
    unsafe { &mut *ptr }
}

#[esp8266_hal::main]
async fn main(_spawner: Spawner) {
    for _ in 0..20_000_000u32 {
        core::hint::spin_loop();
    }

    println!("wifi_alloc_probe start");

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

    let net_state = unsafe { heap_leak_net_state() };
    esp8266_hal::logger::write_byte(b'N');
    let (_runner, net_device) = wifi_net::new(net_state, mac, device);
    esp8266_hal::logger::write_byte(b'W');

    let config = embassy_net::Config::dhcpv4(Default::default());
    let resources = unsafe { heap_leak_stack_resources() };
    esp8266_hal::logger::write_byte(b'R');
    let seed = u64::from_le_bytes([mac[0], mac[1], mac[2], mac[3], mac[4], mac[5], 0x12, 0x34]);
    let (_stack, _net_runner) = embassy_net::new(net_device, config, resources, seed);
    esp8266_hal::logger::write_byte(b'E');

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
