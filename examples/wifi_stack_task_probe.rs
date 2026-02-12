//! Probe that matches tcp_server's post-connect setup but only spawns the
//! driver and stack runner tasks, then prints short heartbeats.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_futures::select::{select, Either};
use embassy_time::{Duration, Timer};

use esp8266_hal::println;
use esp8266_hal::wifi::controller::{WifiInitConfig, WifiInterface};
use esp8266_hal::wifi::device::WifiDevice;
use esp8266_hal::wifi::embassy_net::{self as wifi_net, NetDriverState};
use esp8266_hal::wifi::event;
use esp8266_hal::wifi::{AuthMode, StaConfig, WifiController, WifiError, WifiMode};
use static_cell::StaticCell;
use xtensa_lx_rt::exception::{Context, ExceptionCause};

const WIFI_SSID: &str = include_str!("ssid");
const WIFI_PASS: &str = include_str!("pass");
const MAX_CONNECT_RETRIES: u32 = 10;
static NET_STATE: StaticCell<NetDriverState> = StaticCell::new();

#[inline]
fn stage_marker(stage: u32) -> u8 {
    if stage == 0 {
        b'-'
    } else {
        stage as u8
    }
}

#[inline]
fn hex_nibble(value: u8) -> u8 {
    match value & 0x0f {
        0..=9 => b'0' + (value & 0x0f),
        nibble => b'a' + (nibble - 10),
    }
}

#[inline]
fn write_hex_byte(value: u32) {
    esp8266_hal::logger::write_byte(hex_nibble((value >> 4) as u8));
    esp8266_hal::logger::write_byte(hex_nibble(value as u8));
}

#[inline(never)]
fn net_runner_stage() -> u32 {
    extern "C" {
        static g_net_runner_stage: u32;
    }

    unsafe { g_net_runner_stage }
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
    esp8266_hal::wifi::compat::wifi_os_adapter::arm_pp_sig_trace(2);
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

    println!("wifi_stack_task_probe start");

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
    let diag_device = WifiDevice::new(WifiInterface::Sta);

    let net_state = NET_STATE.init(NetDriverState::new());
    let (driver_runner, net_device) = wifi_net::new(net_state, mac, device);

    let config = embassy_net::Config::dhcpv4(Default::default());
    let resources = unsafe { heap_leak_stack_resources() };
    let seed = u64::from_le_bytes([mac[0], mac[1], mac[2], mac[3], mac[4], mac[5], 0x12, 0x34]);
    let (stack, net_runner) = embassy_net::new(net_device, config, resources, seed);

    spawner.must_spawn(net_driver_task(driver_runner));
    spawner.must_spawn(net_stack_task(net_runner));
    println!("stack runner armed free=0x{:X}", free_heap());

    match select(stack.wait_config_up(), Timer::after(Duration::from_secs(4))).await {
        Either::First(()) => {
            if let Some(cfg) = stack.config_v4() {
                println!(
                    "dhcp up {:?} link={} drops=0x{:X} stage=0x{:X}",
                    cfg.address,
                    stack.is_link_up() as u8,
                    diag_device.rx_drops(),
                    net_runner_stage()
                );
            } else {
                println!(
                    "cfg wake no-v4 link={} drops=0x{:X} stage=0x{:X}",
                    stack.is_link_up() as u8,
                    diag_device.rx_drops(),
                    net_runner_stage()
                );
            }
        }
        Either::Second(()) => {
            println!(
                "dhcp timeout link={} cfg={} drops=0x{:X} stage=0x{:X}",
                stack.is_link_up() as u8,
                stack.is_config_up() as u8,
                diag_device.rx_drops(),
                net_runner_stage()
            );
        }
    }

    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    extern "C" {
        static g_net_runner_stage: u32;
        static g_pp_last_sig: u32;
        static g_pp_stage: u32;
        static g_queue_recv_stage: u32;
        static g_queue_block_stage: u32;
    }

    esp8266_hal::logger::write_byte(b'?');
    esp8266_hal::logger::write_byte(esp8266_hal::wifi::compat::task::debug_current_task_marker());
    let net_stage = unsafe { g_net_runner_stage };
    if net_stage != 0 {
        esp8266_hal::logger::write_byte(net_stage as u8);
    }
    let pp_sig = unsafe { g_pp_last_sig };
    if pp_sig != 0 {
        esp8266_hal::logger::write_byte(b'Q');
        write_hex_byte(pp_sig);
    }
    let pp_stage = unsafe { g_pp_stage };
    if pp_stage != 0 {
        esp8266_hal::logger::write_byte(b'G');
        esp8266_hal::logger::write_byte(stage_marker(pp_stage));
    }
    let queue_recv = unsafe { g_queue_recv_stage };
    if queue_recv != 0 {
        esp8266_hal::logger::write_byte(b'R');
        esp8266_hal::logger::write_byte(stage_marker(queue_recv));
    }
    let queue_block = unsafe { g_queue_block_stage };
    if queue_block != 0 {
        esp8266_hal::logger::write_byte(b'B');
        esp8266_hal::logger::write_byte(stage_marker(queue_block));
    }
    esp8266_hal::logger::write_byte(b'\n');
    loop {}
}

#[link_section = ".rwtext"]
#[xtensa_lx_rt::exception]
fn exception(cause: ExceptionCause, frame: Context) -> ! {
    extern "C" {
        static g_net_runner_stage: u32;
        static g_pp_last_sig: u32;
        static g_pp_stage: u32;
        static g_queue_recv_stage: u32;
        static g_queue_block_stage: u32;
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
    let pp_sig = unsafe { g_pp_last_sig };
    if pp_sig != 0 {
        esp8266_hal::logger::write_byte(b'Q');
        write_hex_byte(pp_sig);
    }
    let pp_stage = unsafe { g_pp_stage };
    if pp_stage != 0 {
        esp8266_hal::logger::write_byte(b'G');
        esp8266_hal::logger::write_byte(stage_marker(pp_stage));
    }
    let queue_recv = unsafe { g_queue_recv_stage };
    if queue_recv != 0 {
        esp8266_hal::logger::write_byte(b'R');
        esp8266_hal::logger::write_byte(stage_marker(queue_recv));
    }
    let queue_block = unsafe { g_queue_block_stage };
    if queue_block != 0 {
        esp8266_hal::logger::write_byte(b'B');
        esp8266_hal::logger::write_byte(stage_marker(queue_block));
    }
    esp8266_hal::logger::write_byte(b'?');
    esp8266_hal::logger::write_byte(b'\n');

    loop {}
}
