//! ESP8266 AP+STA coexistence validation using the typestate WiFi core.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_futures::select::{select, Either};
use embassy_time::{Duration, Instant, Timer};

use esp8266_hal::wifi::embassy_net::{self as wifi_net, NetDriverState};
use esp8266_hal::wifi::{
    AccessPointConfig, ApClientLimit, ApWifiDevice, BeaconInterval, Channel, ClientConfig,
    Credential, DhcpMessageType, DhcpServer, DhcpServerConfig, DriverError, Esp8266TargetBackend,
    InitConfig, Ipv4Addr, MixedConfig, MixedMode, Passphrase, Ssid, Started, WifiDriver,
    WifiResources,
};
use smoltcp::phy::ChecksumCapabilities;
use smoltcp::wire::{
    EthernetAddress, EthernetFrame, EthernetProtocol, IpAddress, IpProtocol,
    Ipv4Address as SmoltcpIpv4Address, Ipv4Packet, Ipv4Repr, UdpPacket, UdpRepr,
    ETHERNET_HEADER_LEN, IPV4_HEADER_LEN, UDP_HEADER_LEN,
};
use static_cell::StaticCell;
use xtensa_lx_rt::exception::{Context, ExceptionCause};

const WIFI_SSID: &str = match option_env!("ESP8266_HAL_WIFI_SSID") {
    Some(ssid) => ssid,
    None => include_str!("ssid"),
};
const WIFI_PASS: &str = match option_env!("ESP8266_HAL_WIFI_PASS") {
    Some(pass) => pass,
    None => include_str!("pass"),
};
const AP_SSID: &str = "esp8266-hal-apsta";
const WIFI_CHANNEL: u8 = 6;
const AP_IP: Ipv4Addr = Ipv4Addr::new(192, 168, 4, 1);
const AP_LEASE_START: Ipv4Addr = Ipv4Addr::new(192, 168, 4, 2);
const AP_NETMASK: Ipv4Addr = Ipv4Addr::new(255, 255, 255, 0);
const AP_RX_FRAME_BYTES: usize = 640;
const DHCP_PACKET_BYTES: usize = 576;
const DHCP_REPLY_FRAME_BYTES: usize =
    ETHERNET_HEADER_LEN + IPV4_HEADER_LEN + UDP_HEADER_LEN + DHCP_PACKET_BYTES;
const DHCP_REPLY_HEADERS: usize = ETHERNET_HEADER_LEN + IPV4_HEADER_LEN + UDP_HEADER_LEN;
const NET_RX_PACKETS: usize = 1;
const NET_TX_PACKETS: usize = 1;

type MixedNetState = NetDriverState<NET_RX_PACKETS, NET_TX_PACKETS>;
type MixedResources = WifiResources<8, 4, 2, 0, 0, 0, 2, AP_RX_FRAME_BYTES>;
type MixedBackend = Esp8266TargetBackend<'static, 8, 4, 2, 0, 0, 0, 2, AP_RX_FRAME_BYTES>;
type MixedDriver =
    WifiDriver<'static, Started<MixedMode>, MixedBackend, 8, 4, 2, 0, 0, 0, 2, AP_RX_FRAME_BYTES>;
type WifiNetRunner = wifi_net::Runner<
    'static,
    'static,
    MixedMode,
    MixedBackend,
    8,
    4,
    2,
    0,
    0,
    0,
    2,
    AP_RX_FRAME_BYTES,
>;
type EmbassyNetRunner = embassy_net::Runner<'static, wifi_net::NetDevice<'static>>;
type ApNetDevice =
    ApWifiDevice<'static, MixedMode, MixedBackend, 8, 4, 2, 0, 0, 0, 2, AP_RX_FRAME_BYTES>;

static NET_STATE: StaticCell<MixedNetState> = StaticCell::new();
static WIFI_RESOURCES: StaticCell<MixedResources> = StaticCell::new();
static STACK_RESOURCES: StaticCell<embassy_net::StackResources<2>> = StaticCell::new();
static WIFI_RUNNER: StaticCell<WifiNetRunner> = StaticCell::new();
static NET_RUNNER: StaticCell<EmbassyNetRunner> = StaticCell::new();
static AP_RX_FRAME: StaticCell<[u8; AP_RX_FRAME_BYTES]> = StaticCell::new();
static DHCP_REPLY_FRAME: StaticCell<[u8; DHCP_REPLY_FRAME_BYTES]> = StaticCell::new();

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
fn log_station_count(count: usize) {
    esp8266_hal::logger::write_bytes(b"ap stations:");
    esp8266_hal::logger::write_dec_u32(count as u32);
    esp8266_hal::logger::write_newline();
}

fn wait_for_downstream_client_enabled() -> bool {
    matches!(option_env!("ESP8266_HAL_AP_STA_WAIT_CLIENT"), Some("1"))
}

fn init_wifi() -> MixedDriver {
    let resources = WIFI_RESOURCES.init(WifiResources::new());
    let backend = Esp8266TargetBackend::new(resources);
    let driver = WifiDriver::with_backend(resources, backend)
        .initialize(InitConfig::new(14, 4, 4).expect("init config"))
        .expect("wifi init");
    log_line(b"wifi init ok");

    let sta_credential = if WIFI_PASS.trim().is_empty() {
        Credential::Open
    } else {
        Credential::Wpa2(Passphrase::wpa2(WIFI_PASS.trim()).expect("pass"))
    };
    let wifi_channel = Channel::try_from(WIFI_CHANNEL).expect("wifi channel");
    let sta_config = ClientConfig::new(
        Ssid::try_from(WIFI_SSID.trim()).expect("sta ssid"),
        sta_credential,
    )
    .with_channel(wifi_channel);
    let ap_config =
        AccessPointConfig::builder(Ssid::try_from(AP_SSID).expect("ap ssid"), wifi_channel)
            .credential(Credential::Open)
            .max_clients(ApClientLimit::try_from(4).expect("ap clients"))
            .beacon_interval(BeaconInterval::try_from(100).expect("beacon"))
            .build();

    driver
        .configure_mixed(MixedConfig::new(sta_config, ap_config))
        .expect("configure mixed")
        .start()
        .expect("wifi start")
}

fn connect_wifi(driver: &mut MixedDriver) -> Result<(), esp8266_hal::wifi::DriverError> {
    let mut sta = driver.sta().expect("sta");
    let _session = sta.connect_blocking(60_000)?;
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

fn start_network(spawner: Spawner, driver: MixedDriver) -> embassy_net::Stack<'static> {
    let link = driver.sta_link().expect("sta link");
    let net_state = NET_STATE.init(MixedNetState::new());
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

async fn wait_for_ipv4_config(
    stack: &embassy_net::Stack<'static>,
    timeout: Duration,
) -> Option<embassy_net::StaticConfigV4> {
    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        if let Some(config) = stack.config_v4() {
            return Some(config);
        }

        match select(
            stack.wait_config_up(),
            Timer::after(Duration::from_millis(250)),
        )
        .await
        {
            Either::First(()) => {
                if let Some(config) = stack.config_v4() {
                    return Some(config);
                }
                Timer::after(Duration::from_millis(100)).await;
            }
            Either::Second(()) => {}
        }
    }

    stack.config_v4()
}

async fn serve_downstream_dhcp(driver: MixedDriver, timeout: Duration) -> (MixedDriver, bool) {
    let link = match driver.ap_link() {
        Ok(link) => link,
        Err(_error) => {
            log_line(b"ap_sta err ap-link");
            loop {
                Timer::after(Duration::from_secs(60)).await;
            }
        }
    };
    let mut device = ApNetDevice::new(link);
    let rx_frame = AP_RX_FRAME.init([0; AP_RX_FRAME_BYTES]);
    let reply_frame = DHCP_REPLY_FRAME.init([0; DHCP_REPLY_FRAME_BYTES]);
    let ap_mac = match device.mac_address() {
        Ok(mac) => EthernetAddress(mac),
        Err(_error) => {
            log_line(b"ap_sta err ap-mac");
            loop {
                Timer::after(Duration::from_secs(60)).await;
            }
        }
    };
    let config = DhcpServerConfig::new(AP_IP, AP_NETMASK, AP_LEASE_START, 4)
        .expect("dhcp config")
        .router(AP_IP)
        .dns_server(AP_IP)
        .lease_seconds(3600);
    let mut server = DhcpServer::<4>::new(config);
    let deadline = Instant::now() + timeout;
    let mut last_wait_log = Instant::now();
    let mut got_ack = false;

    log_line(b"ap-sta-dhcp-ready");
    while Instant::now() < deadline && !got_ack {
        let mut reply = None;
        while let Some(frame_len) = device.receive_frame(rx_frame) {
            if let Some(packet) = dhcp_payload(&rx_frame[..frame_len]) {
                match server.handle(packet, &mut reply_frame[DHCP_REPLY_HEADERS..]) {
                    Ok(result) => reply = result,
                    Err(_error) => log_line(b"ap_sta err dhcp-parse"),
                }
            }
            if reply.is_some() {
                break;
            }
        }

        if let Some(reply) = reply {
            if transmit_dhcp_reply(&mut device, reply_frame, ap_mac, reply.len()).is_err() {
                log_line(b"ap_sta err dhcp-send");
            }
            match reply.message_type() {
                DhcpMessageType::Offer => log_line(b"ap-sta-dhcp-offer"),
                DhcpMessageType::Ack => {
                    log_line(b"ap-sta-dhcp-ack");
                    got_ack = true;
                }
                DhcpMessageType::Nak => log_line(b"ap-sta-dhcp-nak"),
                _ => {}
            }
        } else {
            if last_wait_log.elapsed() >= Duration::from_secs(2) {
                log_line(b"ap-sta-dhcp-wait");
                last_wait_log = Instant::now();
            }
            Timer::after(Duration::from_millis(10)).await;
        }
    }

    let driver = match device.into_link().into_driver() {
        Ok(driver) => driver,
        Err(_error) => {
            log_line(b"ap_sta err ap-driver");
            loop {
                Timer::after(Duration::from_secs(60)).await;
            }
        }
    };
    (driver, got_ack)
}

fn dhcp_payload(frame: &[u8]) -> Option<&[u8]> {
    let ethernet = EthernetFrame::new_checked(frame).ok()?;
    if ethernet.ethertype() != EthernetProtocol::Ipv4 {
        return None;
    }

    let ipv4 = Ipv4Packet::new_checked(ethernet.payload()).ok()?;
    if ipv4.next_header() != IpProtocol::Udp {
        return None;
    }

    let udp = UdpPacket::new_checked(ipv4.payload()).ok()?;
    if udp.src_port() == 68 && udp.dst_port() == 67 {
        Some(udp.payload())
    } else {
        None
    }
}

fn transmit_dhcp_reply(
    device: &mut ApNetDevice,
    frame: &mut [u8],
    ap_mac: EthernetAddress,
    payload_len: usize,
) -> Result<(), DriverError> {
    let src_ip = SmoltcpIpv4Address::from(AP_IP.octets());
    let dst_ip = SmoltcpIpv4Address::BROADCAST;
    let src_ip_addr = IpAddress::Ipv4(src_ip);
    let dst_ip_addr = IpAddress::Ipv4(dst_ip);
    let checksum_caps = ChecksumCapabilities::default();
    let frame_len = DHCP_REPLY_HEADERS + payload_len;

    let mut ethernet = EthernetFrame::new_unchecked(&mut frame[..frame_len]);
    ethernet.set_dst_addr(EthernetAddress::BROADCAST);
    ethernet.set_src_addr(ap_mac);
    ethernet.set_ethertype(EthernetProtocol::Ipv4);

    let mut ipv4 = Ipv4Packet::new_unchecked(ethernet.payload_mut());
    Ipv4Repr {
        src_addr: src_ip,
        dst_addr: dst_ip,
        next_header: IpProtocol::Udp,
        payload_len: UDP_HEADER_LEN + payload_len,
        hop_limit: 64,
    }
    .emit(&mut ipv4, &checksum_caps);

    UdpRepr {
        src_port: 67,
        dst_port: 68,
    }
    .emit(
        &mut UdpPacket::new_unchecked(ipv4.payload_mut()),
        &src_ip_addr,
        &dst_ip_addr,
        payload_len,
        |_| {},
        &checksum_caps,
    );

    device.transmit_frame(&frame[..frame_len])
}

#[esp8266_hal::main]
async fn main(spawner: Spawner) {
    log_line(b"wifi_ap_sta start");

    let mut driver = init_wifi();
    log_line(b"ap started");

    if let Err(error) = connect_wifi(&mut driver) {
        match error {
            esp8266_hal::wifi::DriverError::Timeout => log_line(b"ap_sta err connect-timeout"),
            esp8266_hal::wifi::DriverError::StaStartTimeout => {
                log_line(b"ap_sta err sta-start-timeout")
            }
            esp8266_hal::wifi::DriverError::Disconnected => {
                log_line(b"ap_sta err connect-disconnected")
            }
            esp8266_hal::wifi::DriverError::BackendRejected => {
                log_line(b"ap_sta err connect-backend")
            }
            _ => log_line(b"ap_sta err connect-other"),
        }
        loop {
            embassy_time::Timer::after(Duration::from_secs(60)).await;
        }
    }

    if wait_for_downstream_client_enabled() {
        let (_driver, downstream_ready) =
            serve_downstream_dhcp(driver, Duration::from_secs(30)).await;
        if downstream_ready {
            log_line(b"ap-sta-downstream-ok");
        } else {
            log_line(b"ap_sta err downstream-timeout");
        }
        loop {
            Timer::after(Duration::from_secs(60)).await;
        }
    }

    match driver.ap().and_then(|mut ap| ap.connected_stations::<10>()) {
        Ok(stations) => log_station_count(stations.len()),
        Err(_error) => log_line(b"ap_sta err stations"),
    }

    log_line(b"network starting");
    let stack = start_network(spawner, driver);
    log_line(b"waiting dhcp");
    match wait_for_ipv4_config(&stack, Duration::from_secs(15)).await {
        Some(config) => {
            log_dhcp_up(config);
            log_line(b"ap-sta-ok");
        }
        None => log_line(b"ap_sta err dhcp-timeout"),
    }

    loop {
        Timer::after(Duration::from_secs(60)).await;
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
