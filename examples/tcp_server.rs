//! ESP8266 TCP echo server using embassy-net.
//!
//! This is the Rust/embassy equivalent of the ESP8266 RTOS SDK's
//! `examples/protocols/sockets/tcp_server` flow:
//!
//! 1. bring up WiFi station mode
//! 2. wait until the station is connected
//! 3. start the network stack and wait for DHCP
//! 4. enter a TCP accept loop

#![no_std]
#![no_main]

use core::future::{pending, poll_fn, Future};
use core::task::Poll;

use embassy_executor::Spawner;
use embassy_futures::select::{select, Either};
use embassy_futures::yield_now;
use embassy_net::tcp::TcpSocket;
use embassy_time::Duration;
use embedded_io_async::Write as IoWrite;

use esp8266_hal::wifi::embassy_net::{self as wifi_net, NetDriverStateWithMtu};
mod support;
use esp8266_hal::timer::TimerExt;
use support::log_line;

use esp8266_hal::wifi::{
    Channel, ClientConfig, Configuration, Controller, InitConfig, Interface as WifiInterface,
    Passphrase, Ssid, StationCredential, Wifi,
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
const WIFI_CHANNEL: Option<&str> = option_env!("ESP8266_HAL_WIFI_CHANNEL");
const LISTEN_PORT: u16 = 1234;
const NET_RX_PACKETS: usize = 1;
const NET_TX_PACKETS: usize = 1;
const NET_MTU: usize = 672;
// One KiB total keeps a useful receive window without consuming the radio heap.
const TCP_BUF_BYTES: usize = 512;
type TcpServerNetState = NetDriverStateWithMtu<NET_MTU, NET_RX_PACKETS, NET_TX_PACKETS>;

static NET_STATE: StaticCell<TcpServerNetState> = StaticCell::new();
static WIFI: StaticCell<Wifi> = StaticCell::new();
static STACK_RESOURCES: StaticCell<embassy_net::StackResources<2>> = StaticCell::new();
static WIFI_RUNNER: StaticCell<WifiNetRunner> = StaticCell::new();
static NET_RUNNER: StaticCell<EmbassyNetRunner> = StaticCell::new();
static TCP_RX_BUF: StaticCell<[u8; TCP_BUF_BYTES]> = StaticCell::new();
static TCP_TX_BUF: StaticCell<[u8; TCP_BUF_BYTES]> = StaticCell::new();

type WifiNetRunner = wifi_net::RunnerWithMtu<'static, 'static, NET_MTU>;
type EmbassyNetRunner = embassy_net::Runner<'static, wifi_net::NetDeviceWithMtu<'static, NET_MTU>>;

#[inline(never)]
fn log_listen_port() {
    esp8266_hal::logger::write_bytes(b"listen ");
    esp8266_hal::logger::write_dec_u32(LISTEN_PORT as u32);
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
fn log_dhcp_timeout(stack: &embassy_net::Stack<'static>) {
    esp8266_hal::logger::write_bytes(b"tcp server dhcp detail link=");
    esp8266_hal::logger::write_dec_u32(stack.is_link_up() as u32);
    esp8266_hal::logger::write_bytes(b" cfg=");
    esp8266_hal::logger::write_dec_u32(stack.is_config_up() as u32);
    esp8266_hal::logger::write_newline();
    log_line(b"tcp server dhcp timeout");
}

#[inline(never)]
fn log_connect_error(error: esp8266_hal::wifi::DriverError, driver: &Controller<'_>) {
    esp8266_hal::logger::write_bytes(b"tcp server connect detail error=");
    support::log_debug(&error);
    esp8266_hal::logger::write_bytes(b" reason=");
    esp8266_hal::logger::write_dec_u32(driver.last_disconnect_reason().unwrap_or(0) as u32);
    esp8266_hal::logger::write_newline();
}

#[inline(never)]
fn init_wifi(
    runtime: &'static esp8266_hal::wifi::Runtime,
) -> (Controller<'static>, esp8266_hal::wifi::Interfaces<'static>) {
    let mut init = InitConfig::new(14, 4, 4).expect("init config");
    init.queues.station_rx = 4;
    let wifi = WIFI.init(Wifi::new(runtime, init).unwrap_or_else(|error| {
        esp8266_hal::logger::write_bytes(b"wifi init error=");
        support::log_debug(&error);
        esp8266_hal::logger::write_newline();
        loop {
            esp8266_hal::wifi::delay_ticks(100);
        }
    }));
    let (mut driver, interfaces) = wifi.split();
    log_line(b"wifi init ok");

    let credential = if WIFI_PASS.trim().is_empty() {
        StationCredential::open()
    } else {
        StationCredential::wpa2(Passphrase::wpa2(WIFI_PASS.trim()).expect("pass"))
    };

    let mut config = ClientConfig::new(Ssid::try_from(WIFI_SSID.trim()).expect("ssid"), credential);
    if let Some(channel) = WIFI_CHANNEL
        .and_then(parse_u8_literal)
        .and_then(|channel| Channel::try_from(channel).ok())
    {
        config = config.with_channel(channel);
    }
    driver
        .set_configuration(&Configuration::Station(config))
        .expect("configure sta");
    match driver.start_blocking(1_000) {
        Ok(()) => {}
        Err(error) => {
            esp8266_hal::logger::write_bytes(b"wifi start err ");
            support::log_debug(&error);
            esp8266_hal::logger::write_newline();
            loop {}
        }
    };

    (driver, interfaces)
}

fn parse_u8_literal(value: &str) -> Option<u8> {
    let mut result = 0u8;
    for byte in value.trim().bytes() {
        if !byte.is_ascii_digit() {
            return None;
        }
        result = result.checked_mul(10)?.checked_add(byte - b'0')?;
    }
    Some(result)
}

#[embassy_executor::task]
async fn wifi_driver_task(runner: &'static mut WifiNetRunner) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn net_runner_task(runner: &'static mut EmbassyNetRunner) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn tcp_server_task(stack: embassy_net::Stack<'static>) -> ! {
    tcp_server_loop(stack).await
}

#[inline(never)]
fn start_network(
    spawner: Spawner,
    interface: WifiInterface<'static>,
) -> embassy_net::Stack<'static> {
    let link = interface;
    let net_state = NET_STATE.init(TcpServerNetState::new());
    let (driver_runner, net_device) = wifi_net::new_with_mtu(net_state, link).expect("net driver");

    let config = embassy_net::Config::dhcpv4(Default::default());
    let stack_resources = STACK_RESOURCES.init(embassy_net::StackResources::<2>::new());
    let seed = 0x1234_5678_9abc_def0u64;

    let (stack, net_runner) = embassy_net::new(net_device, config, stack_resources, seed);
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
    log_line(b"tcp_server start");
    let (mut driver, interfaces) = init_wifi(runtime);

    log_line(b"wifi started");
    if let Err(error) = support::connect(&mut driver) {
        log_connect_error(error, &driver);
        log_line(b"tcp server wifi err");
        loop {
            embassy_time::Timer::after(Duration::from_secs(60)).await;
        }
    }
    let stack = start_network(spawner, interfaces.station);
    if !wait_for_dhcp(&stack).await {
        loop {
            embassy_time::Timer::after(Duration::from_secs(60)).await;
        }
    }
    spawner.must_spawn(tcp_server_task(stack));
    pending::<()>().await;
}

async fn wait_for_dhcp(stack: &embassy_net::Stack<'static>) -> bool {
    log_line(b"waiting dhcp");

    match select(
        stack.wait_config_up(),
        embassy_time::Timer::after(Duration::from_secs(30)),
    )
    .await
    {
        Either::First(()) => {
            if let Some(cfg) = stack.config_v4() {
                log_dhcp_up(cfg);
                true
            } else {
                log_line(b"tcp server dhcp no-v4");
                false
            }
        }
        Either::Second(()) => {
            log_dhcp_timeout(stack);
            false
        }
    }
}

async fn tcp_server_loop(stack: embassy_net::Stack<'static>) -> ! {
    let rx_buf = TCP_RX_BUF.init([0; TCP_BUF_BYTES]);
    let tx_buf = TCP_TX_BUF.init([0; TCP_BUF_BYTES]);
    loop {
        let mut socket = TcpSocket::new(stack, rx_buf, tx_buf);
        socket.set_timeout(Some(Duration::from_secs(10)));

        let accept_result = {
            let mut accept = core::pin::pin!(socket.accept(LISTEN_PORT));
            let mut accepted = None;
            poll_fn(|cx| {
                if let Poll::Ready(result) = accept.as_mut().poll(cx) {
                    accepted = Some(result);
                }
                Poll::Ready(())
            })
            .await;

            yield_now().await;
            log_listen_port();
            match accepted {
                Some(result) => result,
                None => accept.await,
            }
        };
        match accept_result {
            Ok(()) => log_line(b"client connected"),
            Err(e) => {
                let _ = e;
                log_line(b"accept err");
                continue;
            }
        }

        let mut buf = [0u8; TCP_BUF_BYTES];
        loop {
            match socket.read(&mut buf).await {
                Ok(0) => {
                    log_line(b"client closed");
                    break;
                }
                Ok(n) => {
                    if let Err(e) = socket.write_all(&buf[..n]).await {
                        let _ = e;
                        log_line(b"write err");
                        break;
                    }
                    if let Err(e) = socket.flush().await {
                        let _ = e;
                        log_line(b"flush err");
                        break;
                    }
                }
                Err(e) => {
                    let _ = e;
                    log_line(b"read err");
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
    esp8266_hal::logger::write_bytes(b"exception ");
    support::log_debug(&cause);
    esp8266_hal::logger::write_bytes(b" ");
    support::log_debug(&frame);
    esp8266_hal::logger::write_newline();
    loop {}
}
