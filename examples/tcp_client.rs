//! ESP8266 TCP client example using embassy-net.
//!
//! Demonstrates WiFi station connection and a simple TCP exchange using
//! `embassy-net` for async networking, driven by an embassy executor.
//!
//! The entry point is `main()`, called by the compat entry point after
//! hardware initialization (PHY, GPIO, MAC). The embassy executor runs on
//! the main thread while WiFi blob tasks run preemptively via the compat
//! scheduler.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_net::tcp::TcpSocket;
use embassy_time::Duration;
use embedded_io_async::Write;

use esp8266_hal::gpio::GpioExt;
use esp8266_hal::prelude::*;
use esp8266_hal::target::Peripherals;
use esp8266_hal::wifi::controller::{WifiInitConfig, WifiInterface};
use esp8266_hal::wifi::device::WifiDevice;
use esp8266_hal::wifi::embassy_net::{self as wifi_net, NetDriverState};
use esp8266_hal::wifi::{AuthMode, StaConfig, WifiController, WifiMode};

// ---------------------------------------------------------------------------
// Configuration — change these to match your network.
// ---------------------------------------------------------------------------

const WIFI_SSID: &str = env!("ESP8266_SSID");
const WIFI_PASS: &str = env!("ESP8266_PASS");

/// Target server to connect to (example.com = 93.184.216.34).
const SERVER_IP: [u8; 4] = [93, 184, 216, 34];
const SERVER_PORT: u16 = 80;

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

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

#[inline(never)]
unsafe fn heap_leak_socket_buf() -> &'static mut [u8; 1460] {
    let raw = unsafe { esp8266_hal::wifi::compat::memory::zalloc(1460) as *mut [u8; 1460] };
    assert!(!raw.is_null(), "heap allocation failed");
    unsafe { &mut *raw }
}

#[esp8266_hal::main]
async fn main(spawner: Spawner) {
    // 1. Initialize WiFi with default parameters.
    let mut wifi = WifiController::init(WifiInitConfig::default()).expect("wifi init");

    // 2. Configure as station and set credentials.
    wifi.set_mode(WifiMode::Sta).expect("set_mode");
    let sta_cfg = StaConfig::builder(WIFI_SSID, WIFI_PASS)
        .auth_threshold(AuthMode::Open)
        .build();
    wifi.set_sta_config(&sta_cfg).expect("set_sta_config");

    // 3. Get the MAC address for the network interface.
    let mac = wifi.get_mac(WifiInterface::Sta).expect("get_mac");

    // 4. Start WiFi and connect asynchronously.
    wifi.start().expect("wifi start");
    wifi.connect_async().await.expect("wifi connect");

    // 5. Use HAL GPIO to signal connection success (LED on GPIO2).
    let dp = unsafe { Peripherals::steal() };
    let pins = dp.GPIO.split();
    let mut led = pins.gpio2.into_push_pull_output();
    let _ = led.set_low(); // LED on (active-low on most boards)

    // 6. Create the WiFi device and register RX callback.
    let device = WifiDevice::new(WifiInterface::Sta);
    device.register_rx_callback().expect("register RX");

    // 7. Create embassy-net driver.
    let net_state = unsafe { heap_leak_net_state() };
    let (runner, net_device) = wifi_net::new(net_state, mac, device);

    // 8. Spawn the network driver runner.
    spawner.must_spawn(net_driver_task(runner));

    // 9. Create the embassy-net stack with DHCP.
    let config = embassy_net::Config::dhcpv4(Default::default());
    let resources = unsafe { heap_leak_stack_resources() };
    let seed = u64::from_le_bytes([mac[0], mac[1], mac[2], mac[3], mac[4], mac[5], 0x42, 0x13]);
    let (stack, net_runner) = embassy_net::new(net_device, config, resources, seed);

    spawner.must_spawn(net_stack_task(net_runner));

    // 10. Wait for DHCP to assign an IP.
    stack.wait_config_up().await;

    // IP address is now assigned; stack.config_v4() can be used to read it.
    let _config = stack.config_v4();

    // 11. Open a TCP connection.
    //
    let rx_buf = unsafe { heap_leak_socket_buf() };
    let tx_buf = unsafe { heap_leak_socket_buf() };

    let mut socket = TcpSocket::new(stack, rx_buf, tx_buf);
    socket.set_timeout(Some(Duration::from_secs(10)));

    let remote = (
        embassy_net::Ipv4Address::new(SERVER_IP[0], SERVER_IP[1], SERVER_IP[2], SERVER_IP[3]),
        SERVER_PORT,
    );

    if let Err(_e) = socket.connect(remote).await {
        return;
    }

    // 12. Send an HTTP GET request.
    let request = b"GET / HTTP/1.0\r\nHost: example.com\r\n\r\n";
    if let Err(_e) = socket.write_all(request).await {
        return;
    }

    // 13. Read the response.
    let mut buf = [0u8; 128];
    loop {
        match socket.read(&mut buf).await {
            Ok(0) => break, // Connection closed.
            Ok(_n) => {
                // Process received data here.
            }
            Err(_e) => {
                break;
            }
        }
    }

    // Socket is dropped here, sending FIN.
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

// ---------------------------------------------------------------------------
// Panic handler
// ---------------------------------------------------------------------------

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe {
        extern "C" {
            fn esp_restart() -> !;
        }
        esp_restart();
    }
}
