#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_futures::select::{select, Either};
use embassy_time::Duration;

use esp8266_hal::wifi::embassy_net::{self as wifi_net, NetDriverState};
mod support;
use esp8266_hal::timer::TimerExt;
use support::log_line;

use esp8266_hal::wifi::{
    Channel, ClientConfig, Configuration, Controller, InitConfig, Interface as WifiInterface,
    Passphrase, Ssid, StationCredential, Wifi,
};
use static_cell::StaticCell;

const WIFI_SSID: &str = match option_env!("ESP8266_HAL_WIFI_SSID") {
    Some(ssid) => ssid,
    None => "esp8266-hal",
};
const WIFI_PASS: &str = match option_env!("ESP8266_HAL_WIFI_PASS") {
    Some(pass) => pass,
    None => "esp8266-pass",
};
const WIFI_CHANNEL: Option<&str> = option_env!("ESP8266_HAL_WIFI_CHANNEL");
const NET_RX_PACKETS: usize = 1;
const NET_TX_PACKETS: usize = 1;

type ProbeNetState = NetDriverState<NET_RX_PACKETS, NET_TX_PACKETS>;
type WifiNetRunner = wifi_net::Runner<'static, 'static>;
type EmbassyNetRunner = embassy_net::Runner<'static, wifi_net::NetDevice<'static>>;

static NET_STATE: StaticCell<ProbeNetState> = StaticCell::new();
static WIFI: StaticCell<Wifi> = StaticCell::new();
static STACK_RESOURCES: StaticCell<embassy_net::StackResources<2>> = StaticCell::new();
static WIFI_RUNNER: StaticCell<WifiNetRunner> = StaticCell::new();
static NET_RUNNER: StaticCell<EmbassyNetRunner> = StaticCell::new();

#[inline(never)]
fn log_dhcp_up(config: embassy_net::StaticConfigV4) {
    esp8266_hal::logger::write_bytes(b"dhcp up Cidr { address: ");
    esp8266_hal::logger::write_ipv4_octets(config.address.address().octets());
    esp8266_hal::logger::write_bytes(b", prefix_len: ");
    esp8266_hal::logger::write_dec_u32(config.address.prefix_len() as u32);
    esp8266_hal::logger::write_bytes(b" }");
    esp8266_hal::logger::write_newline();
}

#[inline(never)]
fn log_dhcp_timeout(stack: &embassy_net::Stack<'static>) {
    esp8266_hal::logger::write_bytes(b"dhcp timeout link=");
    esp8266_hal::logger::write_dec_u32(stack.is_link_up() as u32);
    esp8266_hal::logger::write_bytes(b" cfg=");
    esp8266_hal::logger::write_dec_u32(stack.is_config_up() as u32);
    esp8266_hal::logger::write_newline();
}

fn init_wifi(
    runtime: &'static esp8266_hal::wifi::Runtime,
) -> (Controller<'static>, esp8266_hal::wifi::Interfaces<'static>) {
    let mut init = InitConfig::new(14, 4, 4).expect("init config");
    init.queues.station_rx = 4;
    let wifi = WIFI.init(Wifi::new(runtime, init).expect("wifi init"));
    let (mut driver, interfaces) = wifi.split();

    let credential = if WIFI_PASS.trim().is_empty() {
        StationCredential::open()
    } else {
        StationCredential::wpa2(Passphrase::wpa2(WIFI_PASS.trim()).expect("passphrase"))
    };

    let mut config = ClientConfig::new(Ssid::try_from(WIFI_SSID.trim()).expect("ssid"), credential);
    if let Some(channel) = WIFI_CHANNEL
        .and_then(|value| value.parse::<u8>().ok())
        .and_then(|channel| Channel::try_from(channel).ok())
    {
        config = config.with_channel(channel);
    }
    driver
        .set_configuration(&Configuration::Station(config))
        .expect("configure sta");
    driver.start_blocking(1_000).expect("wifi start");

    log_line(b"wifi started");
    (driver, interfaces)
}

fn log_connect_error(error: esp8266_hal::wifi::DriverError, driver: &Controller<'_>) {
    esp8266_hal::logger::write_bytes(b"wifi connect err kind=");
    support::log_debug(&error);
    esp8266_hal::logger::write_bytes(b" reason=");
    esp8266_hal::logger::write_dec_u32(driver.last_disconnect_reason().unwrap_or(0) as u32);
    esp8266_hal::logger::write_newline();
}

#[embassy_executor::task]
async fn wifi_driver_task(runner: &'static mut WifiNetRunner) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn net_runner_task(runner: &'static mut EmbassyNetRunner) -> ! {
    runner.run().await
}

fn start_network(
    spawner: Spawner,
    interface: WifiInterface<'static>,
) -> embassy_net::Stack<'static> {
    let link = interface;
    let net_state = NET_STATE.init(ProbeNetState::new());
    let (driver_runner, net_device) = wifi_net::new(net_state, link).expect("net driver");

    let config = embassy_net::Config::dhcpv4(Default::default());
    let resources = STACK_RESOURCES.init(embassy_net::StackResources::<2>::new());
    let seed = 0x1234_5678_9abc_def0u64;

    let (stack, net_runner) = embassy_net::new(net_device, config, resources, seed);
    let driver_runner = WIFI_RUNNER.init(driver_runner);
    let net_runner = NET_RUNNER.init(net_runner);
    spawner.must_spawn(wifi_driver_task(driver_runner));
    spawner.must_spawn(net_runner_task(net_runner));
    stack
}

#[esp8266_hal::main]
fn main() -> ! {
    let peripherals = esp8266_hal::target::Peripherals::take().expect("peripherals");
    let (_, timer2) = peripherals.TIMER.timers();
    let runtime = support::init(timer2);
    static EXECUTOR: StaticCell<esp8266_hal::wifi::executor::Executor> = StaticCell::new();
    EXECUTOR
        .init(esp8266_hal::wifi::executor::Executor::new())
        .run(runtime, |spawner| {
            spawner.must_spawn(app_main(spawner, runtime));
        })
}

#[embassy_executor::task]
async fn app_main(spawner: Spawner, runtime: &'static esp8266_hal::wifi::Runtime) {
    log_line(b"wifi_core_dhcp_probe start");

    let (mut driver, interfaces) = init_wifi(runtime);
    if let Err(error) = support::connect(&mut driver) {
        log_connect_error(error, &driver);
        loop {
            embassy_time::Timer::after(Duration::from_secs(60)).await;
        }
    }

    let stack = start_network(spawner, interfaces.station);
    log_line(b"wait dhcp");

    match select(
        stack.wait_config_up(),
        embassy_time::Timer::after(Duration::from_secs(4)),
    )
    .await
    {
        Either::First(()) => {
            if let Some(cfg) = stack.config_v4() {
                log_dhcp_up(cfg);
            } else {
                log_line(b"dhcp wake no-v4");
            }
        }
        Either::Second(()) => log_dhcp_timeout(&stack),
    }

    loop {
        embassy_time::Timer::after(Duration::from_secs(60)).await;
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    esp8266_hal::logger::write_byte(b'?');
    loop {}
}
