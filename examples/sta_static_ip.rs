//! Bring up a station with an application-selected static IPv4 configuration.
//! Set ESP8266_HAL_WIFI_SSID and ESP8266_HAL_WIFI_PASS when building, and replace
//! the documentation addresses below with unused addresses on your network.
//! The TCP examples show how to use the resulting Embassy stack for sockets.

#![no_std]
#![no_main]

mod support;

use core::fmt::Write;
use embassy_net::{Ipv4Address, Ipv4Cidr, StaticConfigV4};
use embassy_time::Timer;
use esp8266_hal::ehal::blocking::rng::Read as _;
use esp8266_hal::rng::RngExt;
use esp8266_hal::timer::TimerExt;
use esp8266_hal::uart::UART0Serial;
use esp8266_hal::wifi::embassy_net::{self as wifi_net, NetDriverStateWithMtu};
use esp8266_hal::wifi::{
    ClientConfig, Configuration, InitConfig, Passphrase, Ssid, StationCredential, Wifi,
};
use panic_halt as _;
use static_cell::StaticCell;

const SSID: &str = match option_env!("ESP8266_HAL_WIFI_SSID") {
    Some(value) => value,
    None => "your-ssid",
};
const PASSWORD: &str = match option_env!("ESP8266_HAL_WIFI_PASS") {
    Some(value) => value,
    None => "your-password",
};
// A 672-byte frame capacity bounds DRAM use while supporting IPv4 DHCP.
const NET_MTU: usize = 672;
type NetState = NetDriverStateWithMtu<NET_MTU, 1, 1>;
type WifiRunner = wifi_net::RunnerWithMtu<'static, 'static, NET_MTU>;
type NetRunner = embassy_net::Runner<'static, wifi_net::NetDeviceWithMtu<'static, NET_MTU>>;

static WIFI: StaticCell<Wifi> = StaticCell::new();
static NET_STATE: StaticCell<NetState> = StaticCell::new();
static STACK_RESOURCES: StaticCell<embassy_net::StackResources<2>> = StaticCell::new();
static WIFI_RUNNER: StaticCell<WifiRunner> = StaticCell::new();
static NET_RUNNER: StaticCell<NetRunner> = StaticCell::new();
static EXECUTOR: StaticCell<esp8266_hal::wifi::executor::Executor> = StaticCell::new();

const ADDRESS: Ipv4Address = Ipv4Address::new(192, 0, 2, 2);
const PREFIX_LEN: u8 = 24;
const GATEWAY: Ipv4Address = Ipv4Address::new(192, 0, 2, 1);

#[esp8266_hal::entry]
fn main() -> ! {
    let peripherals = esp8266_hal::target::Peripherals::take().unwrap();
    let mut serial = support::serial(peripherals.UART0, peripherals.GPIO);
    let (_, timer2) = peripherals.TIMER.timers();
    let runtime = support::init(timer2);
    let mut init = InitConfig::new(14, 4, 4).unwrap();
    init.queues.frame_bytes = NET_MTU;
    let wifi = WIFI.init(Wifi::new(runtime, init).expect("Wi-Fi initialization"));
    let (mut controller, interfaces) = wifi.split();

    let credential = if PASSWORD.is_empty() {
        StationCredential::open()
    } else {
        StationCredential::wpa2(Passphrase::wpa2(PASSWORD).expect("password"))
    };
    let station = ClientConfig::new(Ssid::try_from(SSID).expect("SSID"), credential);
    controller
        .set_configuration(&Configuration::Station(station))
        .expect("station configuration");
    controller.start_blocking(1_000).expect("start Wi-Fi");
    if let Err(error) = controller.connect_blocking(30_000) {
        writeln!(serial, "Connection failed: {error:?}\r").unwrap();
        loop {
            esp8266_hal::wifi::delay_ticks(100);
        }
    }

    let (wifi_runner, device) =
        wifi_net::new_with_mtu(NET_STATE.init(NetState::new()), interfaces.station)
            .expect("network adapter");
    let mut seed = [0; 8];
    peripherals.RNG.rng().read(&mut seed).unwrap();
    let config = embassy_net::Config::ipv4_static(StaticConfigV4 {
        address: Ipv4Cidr::new(ADDRESS, PREFIX_LEN),
        gateway: Some(GATEWAY),
        dns_servers: heapless::Vec::new(),
    });
    let (stack, net_runner) = embassy_net::new(
        device,
        config,
        STACK_RESOURCES.init(embassy_net::StackResources::new()),
        u64::from_le_bytes(seed),
    );
    EXECUTOR
        .init(esp8266_hal::wifi::executor::Executor::new())
        .run(runtime, |spawner| {
            spawner.must_spawn(wifi_task(WIFI_RUNNER.init(wifi_runner)));
            spawner.must_spawn(network_task(NET_RUNNER.init(net_runner)));
            spawner.must_spawn(application(serial, stack));
        })
}

#[embassy_executor::task]
async fn wifi_task(runner: &'static mut WifiRunner) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn network_task(runner: &'static mut NetRunner) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn application(mut serial: UART0Serial, stack: embassy_net::Stack<'static>) -> ! {
    stack.wait_config_up().await;
    writeln!(
        serial,
        "Static IPv4 address: {}\r",
        stack.config_v4().unwrap().address
    )
    .unwrap();
    loop {
        writeln!(serial, "Station link up: {}\r", stack.is_link_up()).unwrap();
        Timer::after_secs(5).await;
    }
}
