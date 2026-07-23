//! ESP8266 TCP client using the typestate WiFi core and embassy-net.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_futures::select::{select, Either};
use embassy_net::tcp::TcpSocket;
use embassy_time::Duration;
use embedded_io_async::Write as IoWrite;

use esp8266_hal::wifi::embassy_net::{self as wifi_net, NetDriverStateWithMtu};
use esp8266_hal::wifi::{
    Channel, ClientConfig, Esp8266TargetBackend, InitConfig, Ipv4Addr as CoreIpv4Addr, Passphrase,
    Ssid, StaMode, Started, StaticIpv4Config, StationCredential, WifiDriver, WifiResources,
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
const NET_MTU: usize = 640;
const TCP_BUF_BYTES: usize = 64;

type TcpClientNetState = NetDriverStateWithMtu<NET_MTU, NET_RX_PACKETS, NET_TX_PACKETS>;
type WifiDriverResources = WifiResources<8, 4>;

static NET_STATE: StaticCell<TcpClientNetState> = StaticCell::new();
static WIFI_RESOURCES: StaticCell<WifiDriverResources> = StaticCell::new();
static STACK_RESOURCES: StaticCell<embassy_net::StackResources<2>> = StaticCell::new();
static WIFI_RUNNER: StaticCell<WifiNetRunner> = StaticCell::new();
static NET_RUNNER: StaticCell<EmbassyNetRunner> = StaticCell::new();
static TCP_RX_BUF: StaticCell<[u8; TCP_BUF_BYTES]> = StaticCell::new();
static TCP_TX_BUF: StaticCell<[u8; TCP_BUF_BYTES]> = StaticCell::new();

type WifiStaDriver =
    WifiDriver<'static, Started<StaMode>, Esp8266TargetBackend<'static, 8, 4>, 8, 4>;
type WifiNetRunner = wifi_net::RunnerWithMtu<
    'static,
    'static,
    StaMode,
    Esp8266TargetBackend<'static, 8, 4>,
    8,
    4,
    0,
    0,
    0,
    0,
    0,
    NET_MTU,
>;
type EmbassyNetRunner = embassy_net::Runner<'static, wifi_net::NetDeviceWithMtu<'static, NET_MTU>>;

#[inline(always)]
fn log_line(message: &[u8]) {
    esp8266_hal::logger::write_bytes(message);
    esp8266_hal::logger::write_newline();
}

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

fn init_wifi() -> WifiStaDriver {
    let resources = WIFI_RESOURCES.init_with(WifiResources::new);
    let driver = WifiDriver::new(resources)
        .initialize(InitConfig::new(14, 4, 4).expect("init config"))
        .expect("wifi init");
    log_line(b"wifi init ok");

    let credential = if WIFI_PASS.trim().is_empty() {
        StationCredential::open()
    } else {
        StationCredential::wpa2(Passphrase::wpa2(WIFI_PASS.trim()).expect("pass"))
    };

    let mut config = ClientConfig::new(Ssid::try_from(WIFI_SSID.trim()).expect("ssid"), credential);
    if let Some(channel) = WIFI_CHANNEL
        .and_then(parse_u16_literal)
        .and_then(|channel| u8::try_from(channel).ok())
        .and_then(|channel| Channel::try_from(channel).ok())
    {
        config = config.with_channel(channel);
    }

    driver
        .configure_sta(config)
        .expect("configure sta")
        .start()
        .expect("wifi start")
}

fn connect_wifi(driver: &mut WifiStaDriver) -> Result<(), esp8266_hal::wifi::DriverError> {
    let mut sta = driver.sta();
    sta.connect_blocking(60_000)?;
    log_line(b"wifi connected");
    Ok(())
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
    driver: WifiStaDriver,
    static_config: Option<StaticIpv4Config>,
) -> embassy_net::Stack<'static> {
    let link = driver.sta_link().expect("sta link");
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
async fn main(spawner: Spawner) {
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

    let mut driver = init_wifi();
    log_line(b"wifi started");
    if let Err(error) = connect_wifi(&mut driver) {
        esp8266_hal::logger::write_bytes(b"tcp client wifi detail ");
        esp8266_hal::logger::write_bytes(driver_error_name(error).as_bytes());
        esp8266_hal::logger::write_bytes(b" reason=");
        esp8266_hal::logger::write_dec_u32(
            driver.resources().last_disconnect_reason().unwrap_or(0) as u32,
        );
        esp8266_hal::logger::write_newline();
        log_line(b"tcp client wifi err");
        loop {
            embassy_time::Timer::after(Duration::from_secs(60)).await;
        }
    }

    let stack = start_network(spawner, driver, static_config);
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
    let host = parse_ipv4_literal(SERVER_HOST).expect("server host");
    let port = parse_u16_literal(SERVER_PORT).expect("server port");
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
            let address = parse_ipv4_literal(address).ok_or(())?;
            let gateway = parse_ipv4_literal(gateway).ok_or(())?;
            let prefix = parse_u8_literal(prefix).ok_or(())?;
            StaticIpv4Config::new(
                CoreIpv4Addr::new(address[0], address[1], address[2], address[3]),
                prefix,
                Some(CoreIpv4Addr::new(
                    gateway[0], gateway[1], gateway[2], gateway[3],
                )),
            )
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

fn parse_ipv4_literal(value: &str) -> Option<[u8; 4]> {
    let mut octets = [0_u8; 4];
    let mut index = 0_usize;
    let mut value_acc = 0_u16;
    let mut has_digit = false;

    for byte in value.as_bytes() {
        match *byte {
            b'0'..=b'9' => {
                value_acc = value_acc
                    .saturating_mul(10)
                    .saturating_add(u16::from(*byte - b'0'));
                if value_acc > 255 {
                    return None;
                }
                has_digit = true;
            }
            b'.' => {
                if !has_digit || index >= 3 {
                    return None;
                }
                octets[index] = value_acc as u8;
                index += 1;
                value_acc = 0;
                has_digit = false;
            }
            _ => return None,
        }
    }

    if !has_digit || index != 3 {
        return None;
    }
    octets[3] = value_acc as u8;
    Some(octets)
}

fn parse_u8_literal(value: &str) -> Option<u8> {
    let parsed = parse_u16_literal(value)?;
    u8::try_from(parsed).ok()
}

fn parse_u16_literal(value: &str) -> Option<u16> {
    let mut acc = 0_u32;
    let mut has_digit = false;
    for byte in value.as_bytes() {
        match *byte {
            b'0'..=b'9' => {
                acc = acc
                    .saturating_mul(10)
                    .saturating_add(u32::from(*byte - b'0'));
                if acc > u32::from(u16::MAX) {
                    return None;
                }
                has_digit = true;
            }
            _ => return None,
        }
    }
    has_digit.then_some(acc as u16)
}

fn driver_error_name(error: esp8266_hal::wifi::DriverError) -> &'static str {
    match error {
        esp8266_hal::wifi::DriverError::Timeout => "timeout",
        esp8266_hal::wifi::DriverError::StaStartTimeout => "sta-start-timeout",
        esp8266_hal::wifi::DriverError::Disconnected => "disconnected",
        esp8266_hal::wifi::DriverError::EventOverflow => "event-overflow",
        esp8266_hal::wifi::DriverError::PacketOverflow => "packet-overflow",
        esp8266_hal::wifi::DriverError::PeerOverflow => "peer-overflow",
        esp8266_hal::wifi::DriverError::PayloadTooLong => "payload-too-long",
        esp8266_hal::wifi::DriverError::InvalidFrameSize => "invalid-frame-size",
        esp8266_hal::wifi::DriverError::OutOfMemory => "out-of-memory",
        esp8266_hal::wifi::DriverError::NotStarted => "not-started",
        esp8266_hal::wifi::DriverError::WouldBlock => "would-block",
        esp8266_hal::wifi::DriverError::BackendRejected => "backend-rejected",
        esp8266_hal::wifi::DriverError::BackendError(_) => "backend-error",
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
