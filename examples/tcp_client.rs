//! ESP8266 TCP client using the Wi-Fi controller and embassy-net.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_futures::select::{select, Either};
use embassy_net::tcp::TcpSocket;
use embassy_time::Duration;
use embedded_io_async::Write as IoWrite;

use esp8266_hal::wifi::embassy_net::{self as wifi_net, NetDriverStateWithMtu};
mod support;
use esp8266_hal::timer::TimerExt;
use support::ip::StaticIpv4Config;
use support::log_line;

use esp8266_hal::wifi::{
    Channel, ClientConfig, Configuration, Controller, InitConfig, Interface as WifiInterface,
    Ipv4Addr as CoreIpv4Addr, Passphrase, Ssid, StationCredential, Wifi,
};
use static_cell::StaticCell;
use xtensa_lx_rt::exception::{Context, ExceptionCause};

const WIFI_SSID: &str = match option_env!("ESP8266_HAL_WIFI_SSID") {
    Some(ssid) => ssid,
    None => "esp8266-hal",
};
const WIFI_PASS: &str = match option_env!("ESP8266_HAL_WIFI_PASS") {
    Some(pass) => pass,
    None => "esp8266-pass",
};
const DEFAULT_SERVER_HOST: &str = "93.184.216.34";
const DEFAULT_SERVER_PORT: &str = "80";
const SERVER_HOST: &str = match option_env!("ESP8266_HAL_TCP_CLIENT_HOST") {
    Some(host) => host,
    None => DEFAULT_SERVER_HOST,
};
const SERVER_PORT: &str = match option_env!("ESP8266_HAL_TCP_CLIENT_PORT") {
    Some(port) => port,
    None => DEFAULT_SERVER_PORT,
};
const STATIC_IPV4: Option<&str> = option_env!("ESP8266_HAL_STATIC_IPV4");
const STATIC_PREFIX: Option<&str> = option_env!("ESP8266_HAL_STATIC_PREFIX");
const STATIC_GATEWAY: Option<&str> = option_env!("ESP8266_HAL_STATIC_GATEWAY");
const WIFI_CHANNEL: Option<&str> = option_env!("ESP8266_HAL_WIFI_CHANNEL");
const NET_RX_PACKETS: usize = 1;
const NET_TX_PACKETS: usize = 1;
// Keep the bridge MTU modest for ESP8266 DRAM. The TCP stack advertises an
// MSS derived from this MTU, so remote peers can still use the example safely.
const NET_MTU: usize = 672;
const TCP_BUF_BYTES: usize = 64;

type TcpClientNetState = NetDriverStateWithMtu<NET_MTU, NET_RX_PACKETS, NET_TX_PACKETS>;

static NET_STATE: StaticCell<TcpClientNetState> = StaticCell::new();
static WIFI: StaticCell<Wifi> = StaticCell::new();
static STACK_RESOURCES: StaticCell<embassy_net::StackResources<2>> = StaticCell::new();
static WIFI_RUNNER: StaticCell<WifiNetRunner> = StaticCell::new();
static NET_RUNNER: StaticCell<EmbassyNetRunner> = StaticCell::new();
static TCP_RX_BUF: StaticCell<[u8; TCP_BUF_BYTES]> = StaticCell::new();
static TCP_TX_BUF: StaticCell<[u8; TCP_BUF_BYTES]> = StaticCell::new();

type WifiNetRunner = wifi_net::RunnerWithMtu<'static, 'static, NET_MTU>;
type EmbassyNetRunner = embassy_net::Runner<'static, wifi_net::NetDeviceWithMtu<'static, NET_MTU>>;

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
fn log_static_up(config: StaticIpv4Config) {
    esp8266_hal::logger::write_bytes(b"static ip up Cidr { address: ");
    esp8266_hal::logger::write_ipv4_octets(config.address().octets());
    esp8266_hal::logger::write_bytes(b", prefix_len: ");
    esp8266_hal::logger::write_dec_u32(config.prefix_len() as u32);
    esp8266_hal::logger::write_bytes(b" }");
    esp8266_hal::logger::write_newline();
}

#[inline(never)]
fn log_target(host: [u8; 4], port: u16) {
    esp8266_hal::logger::write_bytes(b"tcp client target ");
    esp8266_hal::logger::write_ipv4_octets(host);
    esp8266_hal::logger::write_byte(b':');
    esp8266_hal::logger::write_dec_u32(port as u32);
    esp8266_hal::logger::write_newline();
}

#[inline(never)]
fn log_recv_count(count: usize) {
    esp8266_hal::logger::write_bytes(b"recv:");
    esp8266_hal::logger::write_dec_u32(count as u32);
    esp8266_hal::logger::write_newline();
}

fn init_wifi(
    runtime: &'static esp8266_hal::wifi::Runtime,
) -> (Controller<'static>, esp8266_hal::wifi::Interfaces<'static>) {
    let mut init = InitConfig::new(14, 4, 4).expect("init config");
    init.queues.station_rx = 4;
    let wifi = WIFI.init(Wifi::new(runtime, init).expect("wifi init"));
    let (mut driver, interfaces) = wifi.split();
    log_line(b"wifi init ok");

    let credential = if WIFI_PASS.trim().is_empty() {
        StationCredential::open()
    } else {
        StationCredential::wpa2(Passphrase::wpa2(WIFI_PASS.trim()).expect("pass"))
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
    (driver, interfaces)
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
    static_config: Option<StaticIpv4Config>,
) -> embassy_net::Stack<'static> {
    let link = interface;
    let net_state = NET_STATE.init(TcpClientNetState::new());
    let (driver_runner, net_device) = wifi_net::new_with_mtu(net_state, link).expect("net driver");

    let config = match static_config {
        Some(config) => embassy_net::Config::ipv4_static(to_embassy_static_config(config)),
        None => embassy_net::Config::dhcpv4(Default::default()),
    };
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
    log_line(b"tcp_client start");

    let static_config = match parse_static_config() {
        Ok(config) => config,
        Err(()) => {
            log_line(b"tcp client static config err");
            loop {
                embassy_time::Timer::after(Duration::from_secs(60)).await;
            }
        }
    };

    let (mut driver, interfaces) = init_wifi(runtime);
    log_line(b"wifi started");
    if let Err(error) = support::connect(&mut driver) {
        esp8266_hal::logger::write_bytes(b"tcp client wifi detail ");
        support::log_debug(&error);
        esp8266_hal::logger::write_bytes(b" reason=");
        esp8266_hal::logger::write_dec_u32(driver.last_disconnect_reason().unwrap_or(0) as u32);
        esp8266_hal::logger::write_newline();
        log_line(b"tcp client wifi err");
        loop {
            embassy_time::Timer::after(Duration::from_secs(60)).await;
        }
    }

    let stack = start_network(spawner, interfaces.station, static_config);
    if let Some(config) = static_config {
        wait_for_static_config(&stack).await;
        log_static_up(config);
    } else {
        wait_for_dhcp(&stack).await;
    }

    embassy_time::Timer::after(Duration::from_millis(1_000)).await;
    run_tcp_client(stack).await;
    loop {
        embassy_time::Timer::after(Duration::from_secs(60)).await;
    }
}

async fn wait_for_static_config(stack: &embassy_net::Stack<'static>) {
    match select(
        stack.wait_config_up(),
        embassy_time::Timer::after(Duration::from_secs(5)),
    )
    .await
    {
        Either::First(()) => {}
        Either::Second(()) => {
            log_line(b"tcp client static config-up timeout");
            loop {
                embassy_time::Timer::after(Duration::from_secs(60)).await;
            }
        }
    }
}

async fn wait_for_dhcp(stack: &embassy_net::Stack<'static>) {
    log_line(b"waiting dhcp");

    match select(
        stack.wait_config_up(),
        embassy_time::Timer::after(Duration::from_secs(30)),
    )
    .await
    {
        Either::First(()) => {
            if let Some(config) = stack.config_v4() {
                log_dhcp_up(config);
            } else {
                log_line(b"tcp client dhcp err");
                loop {
                    embassy_time::Timer::after(Duration::from_secs(60)).await;
                }
            }
        }
        Either::Second(()) => {
            log_line(b"tcp client dhcp timeout");
            loop {
                embassy_time::Timer::after(Duration::from_secs(60)).await;
            }
        }
    }
}

async fn run_tcp_client(stack: embassy_net::Stack<'static>) {
    let host = SERVER_HOST
        .parse::<CoreIpv4Addr>()
        .expect("server host")
        .octets();
    let port = SERVER_PORT.parse::<u16>().expect("server port");
    log_target(host, port);

    let rx_buf = TCP_RX_BUF.init([0; TCP_BUF_BYTES]);
    let tx_buf = TCP_TX_BUF.init([0; TCP_BUF_BYTES]);
    let mut socket = TcpSocket::new(stack, rx_buf, tx_buf);
    socket.set_timeout(Some(Duration::from_secs(10)));

    let remote = (
        embassy_net::Ipv4Address::new(host[0], host[1], host[2], host[3]),
        port,
    );
    match select(
        socket.connect(remote),
        embassy_time::Timer::after(Duration::from_secs(12)),
    )
    .await
    {
        Either::First(Ok(())) => {}
        Either::First(Err(_error)) => {
            log_line(b"tcp client connect err");
            return;
        }
        Either::Second(()) => {
            socket.abort();
            log_line(b"tcp client connect timeout");
            return;
        }
    }
    log_line(b"tcp client connected");

    let request = b"GET / HTTP/1.0\r\nHost: esp8266-hal-host\r\n\r\n";
    if socket.write_all(request).await.is_err() {
        log_line(b"tcp client write err");
        return;
    }
    if socket.flush().await.is_err() {
        log_line(b"tcp client flush err");
        return;
    }

    let mut buf = [0u8; TCP_BUF_BYTES];
    match socket.read(&mut buf).await {
        Ok(0) => log_line(b"tcp client closed"),
        Ok(count) => {
            log_recv_count(count);
            log_line(b"done");
        }
        Err(_e) => log_line(b"tcp client read err"),
    }
}

fn parse_static_config() -> Result<Option<StaticIpv4Config>, ()> {
    match (STATIC_IPV4, STATIC_PREFIX, STATIC_GATEWAY) {
        (None, None, None) => Ok(None),
        (Some(address), Some(prefix), Some(gateway)) => {
            let address = address.parse::<CoreIpv4Addr>().map_err(|_| ())?;
            let gateway = gateway.parse::<CoreIpv4Addr>().map_err(|_| ())?;
            let prefix = prefix.parse::<u8>().map_err(|_| ())?;
            StaticIpv4Config::new(address, prefix, Some(gateway))
                .map(Some)
                .map_err(|_| ())
        }
        _ => Err(()),
    }
}

fn to_embassy_static_config(config: StaticIpv4Config) -> embassy_net::StaticConfigV4 {
    let address = config.address().octets();
    let gateway = config.gateway().map(|gateway| {
        let octets = gateway.octets();
        embassy_net::Ipv4Address::new(octets[0], octets[1], octets[2], octets[3])
    });

    embassy_net::StaticConfigV4 {
        address: embassy_net::Ipv4Cidr::new(
            embassy_net::Ipv4Address::new(address[0], address[1], address[2], address[3]),
            config.prefix_len(),
        ),
        gateway,
        dns_servers: heapless::Vec::new(),
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
