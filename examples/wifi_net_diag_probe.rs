//! Bring up the STA + embassy-net stack and print RX/TX counters periodically.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};

use esp8266_hal::println;
use esp8266_hal::wifi::controller::{WifiInitConfig, WifiInterface};
use esp8266_hal::wifi::device::{sta_net_diag_snapshot, WifiDevice};
use esp8266_hal::wifi::embassy_net::{self as wifi_net, NetDriverState};
use esp8266_hal::wifi::event;
use esp8266_hal::wifi::{AuthMode, StaConfig, WifiController, WifiError, WifiMode};
use static_cell::StaticCell;

const WIFI_SSID: &str = include_str!("ssid");
const WIFI_PASS: &str = include_str!("pass");
const MAX_CONNECT_RETRIES: u32 = 10;
static NET_STATE: StaticCell<NetDriverState> = StaticCell::new();

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

#[embassy_executor::task]
async fn net_stack_task(
    mut runner: embassy_net::Runner<'static, esp8266_hal::wifi::embassy_net::NetDevice<'static>>,
) {
    runner.run().await;
}

#[esp8266_hal::main]
async fn main(spawner: Spawner) {
    for _ in 0..20_000_000u32 {
        core::hint::spin_loop();
    }

    println!("wifi_net_diag_probe start");

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

    let net_state = NET_STATE.init(NetDriverState::new());
    let (driver_runner, net_device) = wifi_net::new(net_state, mac, device);

    let config = embassy_net::Config::dhcpv4(Default::default());
    let resources = unsafe { heap_leak_stack_resources() };
    let seed = u64::from_le_bytes([mac[0], mac[1], mac[2], mac[3], mac[4], mac[5], 0x12, 0x34]);
    let (stack, net_runner) = embassy_net::new(net_device, config, resources, seed);

    spawner.must_spawn(net_driver_task(driver_runner));
    spawner.must_spawn(net_stack_task(net_runner));
    println!("stack runner armed free=0x{:X}", free_heap());

    stack.wait_config_up().await;
    if let Some(cfg) = stack.config_v4() {
        println!("dhcp up {:?}", cfg.address);
    }

    loop {
        let diag = sta_net_diag_snapshot();
        println!(
            "diag rx={} arp={} ip4={} drop={} tx={} tarp={} tip4={} fail={}",
            diag.rx_total,
            diag.rx_arp,
            diag.rx_ipv4,
            diag.rx_drops,
            diag.tx_total,
            diag.tx_arp,
            diag.tx_ipv4,
            diag.tx_fail,
        );
        Timer::after(Duration::from_secs(1)).await;
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    esp8266_hal::logger::write_byte(b'?');
    loop {}
}
