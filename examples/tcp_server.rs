//! ESP8266 TCP echo server using embassy-net.
//!
//! This is the Rust/embassy equivalent of the ESP8266 RTOS SDK's
//! `examples/protocols/sockets/tcp_server` flow:
//!
//! 1. bring up WiFi station mode
//! 2. wait until the station is connected
//! 3. start the network stack and wait for DHCP
//! 4. enter a TCP accept loop

#![feature(panic_info_message)]
#![no_std]
#![no_main]

use core::future::Future;

use embassy_executor::Spawner;
use embassy_net::tcp::TcpSocket;
use embassy_time::Duration;
use embedded_io_async::Write as IoWrite;

use esp8266_hal::println;
use esp8266_hal::wifi::controller::{WifiInitConfig, WifiInterface};
use esp8266_hal::wifi::device::{clear_sta_net_diag, WifiDevice};
use esp8266_hal::wifi::embassy_net::{self as wifi_net, NetDriverState};
use esp8266_hal::wifi::event;
use esp8266_hal::wifi::{AuthMode, StaConfig, WifiController, WifiError, WifiMode};
use static_cell::StaticCell;
use xtensa_lx_rt::exception::{Context, ExceptionCause};

const WIFI_SSID: &str = include_str!("ssid");
const WIFI_PASS: &str = include_str!("pass");
const MAX_CONNECT_RETRIES: u32 = 10;
const LISTEN_PORT: u16 = 1234;
// The RTOS SDK socket example uses a 128-byte application RX buffer.
// Keep the embassy TCP buffers in that same range to stay within ESP8266 DRAM.
const TCP_BUF_BYTES: usize = 128;
#[no_mangle]
static mut g_tcp_server_stage: u32 = 0;
static NET_STATE: StaticCell<NetDriverState> = StaticCell::new();

#[inline(always)]
fn record_tcp_server_stage(stage: u8) {
    unsafe { g_tcp_server_stage = stage as u32 };
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

#[inline]
fn write_hex_u16(value: u32) {
    write_hex_byte((value >> 8) & 0xff);
    write_hex_byte(value & 0xff);
}

#[inline]
fn write_hex_u32(value: u32) {
    write_hex_u16(value >> 16);
    write_hex_u16(value);
}

#[inline(never)]
unsafe fn heap_leak_stack_resources() -> &'static mut embassy_net::StackResources<2> {
    let align = core::mem::align_of::<embassy_net::StackResources<2>>();
    let size = core::mem::size_of::<embassy_net::StackResources<2>>();
    let total = size.saturating_add(align.saturating_sub(1));
    esp8266_hal::wifi::compat::memory::clear_alloc_failure_debug();
    let raw = unsafe { esp8266_hal::wifi::compat::memory::malloc(total) as *mut u8 };
    if raw.is_null() {
        let (_, _, hi_free, _, _, lo_free) = esp8266_hal::wifi::compat::memory::debug_region_info();
        crate::println!(
            "stack_res alloc fail size=0x{:X} align=0x{:X} hi=0x{:X} lo=0x{:X} total=0x{:X}",
            size,
            align,
            hi_free,
            lo_free,
            hi_free + lo_free,
        );
        if let Some((kind, fail_size, free, task, count)) =
            esp8266_hal::wifi::compat::memory::debug_last_alloc_failure()
        {
            crate::println!(
                "stack_res alloc dbg kind={} size=0x{:X} free=0x{:X} task={} count={}",
                kind as char,
                fail_size,
                free,
                task as char,
                count,
            );
        }
    }
    assert!(!raw.is_null(), "heap allocation failed");
    let aligned = ((raw as usize) + (align - 1)) & !(align - 1);
    let ptr = aligned as *mut embassy_net::StackResources<2>;
    unsafe { ptr.write(embassy_net::StackResources::<2>::new()) };
    unsafe { &mut *ptr }
}

#[inline(never)]
unsafe fn heap_leak_value<T>(value: T) -> &'static mut T {
    let align = core::mem::align_of::<T>();
    let size = core::mem::size_of::<T>();
    let total = size.saturating_add(align.saturating_sub(1));
    let raw = unsafe { esp8266_hal::wifi::compat::memory::malloc(total) as *mut u8 };
    assert!(!raw.is_null(), "heap allocation failed");
    let aligned = ((raw as usize) + (align - 1)) & !(align - 1);
    let ptr = aligned as *mut T;
    unsafe { ptr.write(value) };
    unsafe { &mut *ptr }
}

#[inline(never)]
unsafe fn heap_leak_socket_buf() -> &'static mut [u8; TCP_BUF_BYTES] {
    let raw = unsafe {
        esp8266_hal::wifi::compat::memory::zalloc(TCP_BUF_BYTES) as *mut [u8; TCP_BUF_BYTES]
    };
    assert!(!raw.is_null(), "heap allocation failed");
    unsafe { &mut *raw }
}

#[inline(never)]
fn free_heap() -> u32 {
    unsafe { esp8266_hal::wifi::compat::memory::esp_get_free_heap_size() }
}

#[inline(never)]
fn init_wifi() -> (WifiController, [u8; 6]) {
    let mut wifi = WifiController::init(WifiInitConfig::default()).expect("wifi init");
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

    (wifi, mac)
}

#[inline(never)]
async fn connect_wifi(wifi: &mut WifiController) -> Result<(), WifiError> {
    println!("wifi connect pre free=0x{:X}", free_heap());
    wifi.wait_for_auto_connect_result_blocking(5)?;
    println!("wifi connected free=0x{:X}", free_heap());
    Ok(())
}

#[inline(never)]
async fn run_network(spawner: Spawner, mac: [u8; 6]) -> ! {
    let device = WifiDevice::new(WifiInterface::Sta);
    device.register_rx_callback().expect("register_rx_callback");
    let net_state = NET_STATE.init(NetDriverState::new());
    let (driver_runner, net_device) = wifi_net::new(net_state, mac, device);

    let config = embassy_net::Config::dhcpv4(Default::default());
    let resources = unsafe { heap_leak_stack_resources() };
    let seed = u64::from_le_bytes([mac[0], mac[1], mac[2], mac[3], mac[4], mac[5], 0x12, 0x34]);

    let (stack, net_runner) = embassy_net::new(net_device, config, resources, seed);
    // `embassy_executor::task` stores large by-value task arguments twice in
    // the generated future state here. Keep the runners themselves on heap
    // and use raw TaskStorage so the executor arena only needs the small main
    // task on this memory-constrained target.
    let driver_runner = unsafe { heap_leak_value(driver_runner) };
    let net_runner = unsafe { heap_leak_value(net_runner) };
    spawn_heap_task(spawner, move || driver_runner.run());
    spawn_heap_task(spawner, move || net_runner.run());
    wait_for_dhcp_and_serve(stack).await
}

#[inline(never)]
fn spawn_heap_task<F, FutFn>(spawner: Spawner, future_fn: FutFn)
where
    F: Future + 'static,
    FutFn: FnOnce() -> F,
{
    let task = unsafe { heap_leak_value(embassy_executor::raw::TaskStorage::<F>::new()) };
    spawner.must_spawn(task.spawn(future_fn));
}

#[esp8266_hal::main]
async fn main(spawner: Spawner) {
    println!("tcp_server start");

    let (mut wifi, mac) = init_wifi();

    wifi.start().expect("wifi start");
    println!("wifi started");
    if let Err(_e) = connect_wifi(&mut wifi).await {
        loop {
            embassy_time::Timer::after(Duration::from_secs(60)).await;
        }
    }
    let _wifi_guard = wifi;
    run_network(spawner, mac).await
}

async fn wait_for_dhcp_and_serve(stack: embassy_net::Stack<'static>) -> ! {
    record_tcp_server_stage(b'd');
    println!("waiting dhcp");
    stack.wait_config_up().await;
    record_tcp_server_stage(b'D');
    if let Some(cfg) = stack.config_v4() {
        esp8266_hal::logger::write_byte(b'i');
        println!("dhcp up {:?}", cfg.address);
        record_tcp_server_stage(b'T');
        tcp_server_loop(stack).await;
    }

    loop {
        embassy_time::Timer::after(Duration::from_secs(60)).await;
    }
}

async fn tcp_server_loop(stack: embassy_net::Stack<'static>) -> ! {
    loop {
        let rx_buf = unsafe { heap_leak_socket_buf() };
        let tx_buf = unsafe { heap_leak_socket_buf() };
        let mut socket = TcpSocket::new(stack, rx_buf, tx_buf);
        socket.set_timeout(Some(Duration::from_secs(10)));

        record_tcp_server_stage(b'l');
        clear_sta_net_diag();
        esp8266_hal::logger::write_byte(b'l');
        println!("listen {}", LISTEN_PORT);
        match socket.accept(LISTEN_PORT).await {
            Ok(()) => {
                record_tcp_server_stage(b'L');
                println!("client connected");
            }
            Err(e) => {
                record_tcp_server_stage(b'E');
                println!("accept err {:?}", e);
                continue;
            }
        }

        let mut buf = [0u8; TCP_BUF_BYTES];
        loop {
            record_tcp_server_stage(b'r');
            match socket.read(&mut buf).await {
                Ok(0) => {
                    record_tcp_server_stage(b'c');
                    println!("client closed");
                    break;
                }
                Ok(n) => {
                    record_tcp_server_stage(b'w');
                    if let Err(e) = socket.write_all(&buf[..n]).await {
                        record_tcp_server_stage(b'E');
                        println!("write err {:?}", e);
                        break;
                    }
                    if let Err(e) = socket.flush().await {
                        record_tcp_server_stage(b'E');
                        println!("flush err {:?}", e);
                        break;
                    }
                    record_tcp_server_stage(b'W');
                }
                Err(e) => {
                    record_tcp_server_stage(b'E');
                    println!("read err {:?}", e);
                    break;
                }
            }
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    esp8266_hal::logger::write_byte(b'?');
    loop {}
}

#[link_section = ".rwtext"]
#[xtensa_lx_rt::exception]
fn exception(cause: ExceptionCause, frame: Context) -> ! {
    let _ = cause;
    let _ = frame;
    esp8266_hal::logger::write_byte(b'E');
    loop {}
}
