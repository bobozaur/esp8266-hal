#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_futures::select::{select, Either};
use embassy_time::Duration;

use esp8266_hal::wifi::embassy_net::{self as wifi_net, NetDriverState};
use esp8266_hal::wifi::{
    Channel, ClientConfig, Credential, Esp8266TargetBackend, InitConfig, Passphrase, Ssid, StaMode,
    Started, WifiDriver, WifiResources,
};
use static_cell::StaticCell;

const WIFI_SSID: &str = match option_env!("ESP8266_HAL_WIFI_SSID") {
    Some(ssid) => ssid,
    None => include_str!("ssid"),
};
const WIFI_PASS: &str = match option_env!("ESP8266_HAL_WIFI_PASS") {
    Some(pass) => pass,
    None => include_str!("pass"),
};
const WIFI_CHANNEL: Option<&str> = option_env!("ESP8266_HAL_WIFI_CHANNEL");
const NET_RX_PACKETS: usize = 1;
const NET_TX_PACKETS: usize = 1;

type ProbeNetState = NetDriverState<NET_RX_PACKETS, NET_TX_PACKETS>;
type ProbeResources = WifiResources<8, 4, 2>;
type ProbeDriver =
    WifiDriver<'static, Started<StaMode>, Esp8266TargetBackend<'static, 8, 4, 2>, 8, 4, 2>;
type WifiNetRunner =
    wifi_net::Runner<'static, 'static, StaMode, Esp8266TargetBackend<'static, 8, 4, 2>, 8, 4, 2>;
type EmbassyNetRunner = embassy_net::Runner<'static, wifi_net::NetDevice<'static>>;

static NET_STATE: StaticCell<ProbeNetState> = StaticCell::new();
static WIFI_RESOURCES: StaticCell<ProbeResources> = StaticCell::new();
static STACK_RESOURCES: StaticCell<embassy_net::StackResources<2>> = StaticCell::new();
static WIFI_RUNNER: StaticCell<WifiNetRunner> = StaticCell::new();
static NET_RUNNER: StaticCell<EmbassyNetRunner> = StaticCell::new();

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
fn log_dhcp_timeout(stack: &embassy_net::Stack<'static>, resources: &'static ProbeResources) {
    esp8266_hal::logger::write_bytes(b"dhcp timeout link=");
    esp8266_hal::logger::write_dec_u32(stack.is_link_up() as u32);
    esp8266_hal::logger::write_bytes(b" cfg=");
    esp8266_hal::logger::write_dec_u32(stack.is_config_up() as u32);
    esp8266_hal::logger::write_bytes(b" queued=");
    esp8266_hal::logger::write_dec_u32(resources.queued_rx() as u32);
    esp8266_hal::logger::write_bytes(b" drops=");
    esp8266_hal::logger::write_dec_u32(resources.rx_drops());
    esp8266_hal::logger::write_newline();
}

fn init_wifi() -> (ProbeDriver, &'static ProbeResources) {
    let resources = WIFI_RESOURCES.init(ProbeResources::new());
    let backend = Esp8266TargetBackend::new(resources);
    let driver = WifiDriver::with_backend(resources, backend)
        .initialize(InitConfig::new(14, 4, 4).expect("init config"))
        .expect("wifi init");

    let credential = if WIFI_PASS.trim().is_empty() {
        Credential::Open
    } else {
        Credential::Wpa2(Passphrase::wpa2(WIFI_PASS.trim()).expect("passphrase"))
    };

    let mut config = ClientConfig::new(Ssid::try_from(WIFI_SSID.trim()).expect("ssid"), credential);
    if let Some(channel) = WIFI_CHANNEL
        .and_then(parse_u8_literal)
        .and_then(|channel| Channel::try_from(channel).ok())
    {
        config = config.with_channel(channel);
    }
    let driver = driver
        .configure_sta(config)
        .expect("configure sta")
        .start()
        .expect("start");

    log_line(b"wifi started");
    (driver, resources)
}

fn connect_wifi(driver: &mut ProbeDriver) -> Result<(), esp8266_hal::wifi::DriverError> {
    let mut sta = driver.sta().expect("sta");
    let _session = sta.connect_blocking(60_000)?;
    log_line(b"wifi connected");
    Ok(())
}

fn log_connect_error(error: esp8266_hal::wifi::DriverError, resources: &'static ProbeResources) {
    esp8266_hal::logger::write_bytes(b"wifi connect err kind=");
    esp8266_hal::logger::write_bytes(driver_error_name(error).as_bytes());
    esp8266_hal::logger::write_bytes(b" reason=");
    esp8266_hal::logger::write_dec_u32(resources.last_disconnect_reason().unwrap_or(0) as u32);
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

fn start_network(spawner: Spawner, driver: ProbeDriver) -> embassy_net::Stack<'static> {
    let link = driver.sta_link().expect("sta link");
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
async fn main(spawner: Spawner) {
    log_line(b"wifi_core_dhcp_probe start");

    let (mut driver, resources) = init_wifi();
    if let Err(error) = connect_wifi(&mut driver) {
        log_connect_error(error, resources);
        loop {
            embassy_time::Timer::after(Duration::from_secs(60)).await;
        }
    }

    let stack = start_network(spawner, driver);
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
        Either::Second(()) => log_dhcp_timeout(&stack, resources),
    }

    loop {
        embassy_time::Timer::after(Duration::from_secs(60)).await;
    }
}

fn parse_u8_literal(value: &str) -> Option<u8> {
    let mut acc = 0_u16;
    let mut has_digit = false;
    for byte in value.as_bytes() {
        match *byte {
            b'0'..=b'9' => {
                acc = acc
                    .saturating_mul(10)
                    .saturating_add(u16::from(*byte - b'0'));
                if acc > u16::from(u8::MAX) {
                    return None;
                }
                has_digit = true;
            }
            _ => return None,
        }
    }
    has_digit.then_some(acc as u8)
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
