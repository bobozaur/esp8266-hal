//! ESP8266 AP+STA coexistence validation using the Wi-Fi controller.
//!
//! ESP8266 shares one radio channel. ESP8266_HAL_WIFI_CHANNEL optionally hints
//! the upstream channel; without it the station scans and the AP starts on 6.
//! After association the AP follows the actual upstream channel, so downstream
//! clients should scan rather than assume the initial AP channel remains 6.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};

use esp8266_hal::wifi::embassy_net::{self as wifi_net, NetDriverStateWithMtu};
mod support;
use esp8266_hal::timer::TimerExt;
use support::dhcp::{DhcpMessageType, DhcpReply, DhcpServer, DhcpServerConfig};
use support::log_line;

use esp8266_hal::wifi::{
    AccessPointConfig, ApClientLimit, ApCredential, BeaconInterval, Channel, ClientConfig,
    Configuration, Controller, DriverError, InitConfig, Ipv4Addr, MixedConfig, Passphrase, Ssid,
    StationCredential, Wifi,
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
    None => "esp8266-hal",
};
const WIFI_PASS: &str = match option_env!("ESP8266_HAL_WIFI_PASS") {
    Some(pass) => pass,
    None => "esp8266-pass",
};
const AP_SSID: &str = "esp8266-hal-apsta";
const WIFI_CHANNEL: Option<&str> = option_env!("ESP8266_HAL_WIFI_CHANNEL");
const AP_IP: Ipv4Addr = Ipv4Addr::new(192, 168, 4, 1);
const AP_LEASE_START: Ipv4Addr = Ipv4Addr::new(192, 168, 4, 2);
const AP_NETMASK: Ipv4Addr = Ipv4Addr::new(255, 255, 255, 0);
// smoltcp DHCP needs at least 658 Ethernet bytes to advertise option 57 >= 576.
// Keep this DHCP-only demo at 672 to bound channel and application frame storage.
const NET_MTU: usize = 672;
const DHCP_PACKET_BYTES: usize = 576;
const DHCP_REPLY_FRAME_BYTES: usize =
    ETHERNET_HEADER_LEN + IPV4_HEADER_LEN + UDP_HEADER_LEN + DHCP_PACKET_BYTES;
const DHCP_REPLY_HEADERS: usize = ETHERNET_HEADER_LEN + IPV4_HEADER_LEN + UDP_HEADER_LEN;
const NET_RX_PACKETS: usize = 1;
const NET_TX_PACKETS: usize = 1;

type MixedNetState = NetDriverStateWithMtu<NET_MTU, NET_RX_PACKETS, NET_TX_PACKETS>;
type WifiNetRunner = wifi_net::RunnerWithMtu<'static, 'static, NET_MTU>;
type EmbassyNetRunner = embassy_net::Runner<'static, wifi_net::NetDeviceWithMtu<'static, NET_MTU>>;
type ApNetDevice = esp8266_hal::wifi::Interface<'static>;

static NET_STATE: StaticCell<MixedNetState> = StaticCell::new();
static WIFI: StaticCell<Wifi> = StaticCell::new();
static STACK_RESOURCES: StaticCell<embassy_net::StackResources<2>> = StaticCell::new();
static WIFI_RUNNER: StaticCell<WifiNetRunner> = StaticCell::new();
static NET_RUNNER: StaticCell<EmbassyNetRunner> = StaticCell::new();
static AP_DEVICE: StaticCell<ApNetDevice> = StaticCell::new();
static AP_RX_FRAME: StaticCell<[u8; NET_MTU]> = StaticCell::new();
static DHCP_REPLY_FRAME: StaticCell<[u8; DHCP_REPLY_FRAME_BYTES]> = StaticCell::new();

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

fn init_wifi(
    runtime: &'static esp8266_hal::wifi::Runtime,
) -> (Controller<'static>, esp8266_hal::wifi::Interfaces<'static>) {
    let mut init = InitConfig::new(14, 4, 4).expect("init config");
    init.queues.station_rx = 4;
    init.queues.access_point_rx = 2;
    init.queues.frame_bytes = NET_MTU;
    let wifi = WIFI.init(Wifi::new(runtime, init).expect("wifi init"));
    let (mut driver, interfaces) = wifi.split();
    log_line(b"wifi init ok");

    let sta_credential = if WIFI_PASS.trim().is_empty() {
        StationCredential::open()
    } else {
        StationCredential::wpa2(Passphrase::wpa2(WIFI_PASS.trim()).expect("pass"))
    };
    let wifi_channel = WIFI_CHANNEL.map(|value| {
        Channel::try_from(value.parse::<u8>().expect("wifi channel")).expect("wifi channel")
    });
    let mut sta_config = ClientConfig::new(
        Ssid::try_from(WIFI_SSID.trim()).expect("sta ssid"),
        sta_credential,
    );
    if let Some(channel) = wifi_channel {
        sta_config = sta_config.with_channel(channel);
    }
    let ap_channel = wifi_channel.unwrap_or(Channel::try_from(6).expect("initial AP channel"));
    let ap_config =
        AccessPointConfig::builder(Ssid::try_from(AP_SSID).expect("ap ssid"), ap_channel)
            .credential(ApCredential::Open)
            .max_clients(ApClientLimit::try_from(4).expect("ap clients"))
            .beacon_interval(BeaconInterval::try_from(100).expect("beacon"))
            .build();

    driver
        .set_configuration(&Configuration::Mixed(MixedConfig::new(
            sta_config, ap_config,
        )))
        .expect("configure mixed");
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
    interfaces: esp8266_hal::wifi::Interfaces<'static>,
) -> (embassy_net::Stack<'static>, &'static mut ApNetDevice) {
    let sta_link = interfaces.station;
    let ap_link = interfaces.access_point;
    let net_state = NET_STATE.init(MixedNetState::new());
    let (driver_runner, net_device) =
        wifi_net::new_with_mtu(net_state, sta_link).expect("STA net driver");
    let ap_device = AP_DEVICE.init(ap_link);

    let config = embassy_net::Config::dhcpv4(Default::default());
    let resources = STACK_RESOURCES.init(embassy_net::StackResources::<2>::new());
    let seed = 0x1234_5678_9abc_def0u64;

    let (stack, net_runner) = embassy_net::new(net_device, config, resources, seed);
    let driver_runner = WIFI_RUNNER.init(driver_runner);
    let net_runner = NET_RUNNER.init(net_runner);
    spawner.must_spawn(wifi_driver_task(driver_runner));
    spawner.must_spawn(net_runner_task(net_runner));
    (stack, ap_device)
}

async fn run_networks(stack: embassy_net::Stack<'static>, device: &'static mut ApNetDevice) -> ! {
    let rx_frame = AP_RX_FRAME.init([0; NET_MTU]);
    let reply_frame = DHCP_REPLY_FRAME.init([0; DHCP_REPLY_FRAME_BYTES]);
    let ap_mac = EthernetAddress(device.mac_address());
    let config = DhcpServerConfig::new(AP_IP, AP_NETMASK, AP_LEASE_START, 4)
        .expect("dhcp config")
        .router(AP_IP)
        .dns_server(AP_IP)
        .lease_seconds(3600);
    let mut server = DhcpServer::<4>::new(config);
    let sta_deadline = Instant::now() + Duration::from_secs(15);
    let mut sta_ready = false;
    let mut sta_timeout_reported = false;
    let mut last_wait_log = Instant::now();
    let mut downstream_ready = false;

    log_line(b"ap-sta-dhcp-ready");
    loop {
        let mut reply = None;
        let mut made_progress = false;
        loop {
            let frame_len = match device.receive(rx_frame) {
                Ok(Some(frame_len)) => {
                    made_progress = true;
                    frame_len
                }
                Ok(None) => break,
                Err(_) => {
                    log_line(b"ap_sta rx drop");
                    continue;
                }
            };
            if let Some(packet) = dhcp_payload(&rx_frame[..frame_len]) {
                match server.handle(
                    Instant::now().as_secs(),
                    packet,
                    &mut reply_frame[DHCP_REPLY_HEADERS..],
                ) {
                    Ok(result) => reply = result,
                    Err(_error) => log_line(b"ap_sta err dhcp-parse"),
                }
            }
            if reply.is_some() {
                break;
            }
        }

        if let Some(reply) = reply {
            if let Err(error) = transmit_dhcp_reply(device, reply_frame, ap_mac, reply).await {
                esp8266_hal::logger::write_bytes(b"ap_sta err dhcp-send ");
                support::log_debug(&error);
                esp8266_hal::logger::write_newline();
            } else {
                match reply.message_type() {
                    DhcpMessageType::Offer => log_line(b"ap-sta-dhcp-offer"),
                    DhcpMessageType::Ack => {
                        log_line(b"ap-sta-dhcp-ack");
                        if !downstream_ready {
                            log_line(b"ap-sta-downstream-ok");
                            downstream_ready = true;
                        }
                    }
                    DhcpMessageType::Nak => log_line(b"ap-sta-dhcp-nak"),
                    _ => {}
                }
            }
        } else if !downstream_ready {
            if last_wait_log.elapsed() >= Duration::from_secs(2) {
                log_line(b"ap-sta-dhcp-wait");
                last_wait_log = Instant::now();
            }
        }

        if !sta_ready {
            if let Some(config) = stack.config_v4() {
                log_dhcp_up(config);
                log_line(b"ap-sta-ok");
                sta_ready = true;
            } else if !sta_timeout_reported && Instant::now() >= sta_deadline {
                log_line(b"ap_sta err dhcp-timeout");
                sta_timeout_reported = true;
            }
        }

        Timer::after(if made_progress {
            Duration::from_millis(1)
        } else {
            Duration::from_millis(10)
        })
        .await;
    }
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

async fn transmit_dhcp_reply(
    device: &mut ApNetDevice,
    frame: &mut [u8],
    ap_mac: EthernetAddress,
    reply: DhcpReply,
) -> Result<(), DriverError> {
    let src_ip = SmoltcpIpv4Address::from(AP_IP.octets());
    let payload_len = reply.len();
    let broadcast = reply.broadcast() || reply.leased_ip() == Ipv4Addr::UNSPECIFIED;
    let dst_ip = if broadcast {
        SmoltcpIpv4Address::BROADCAST
    } else {
        SmoltcpIpv4Address::from(reply.leased_ip().octets())
    };
    let src_ip_addr = IpAddress::Ipv4(src_ip);
    let dst_ip_addr = IpAddress::Ipv4(dst_ip);
    let checksum_caps = ChecksumCapabilities::default();
    let frame_len = DHCP_REPLY_HEADERS + payload_len;

    let mut ethernet = EthernetFrame::new_unchecked(&mut frame[..frame_len]);
    ethernet.set_dst_addr(if broadcast {
        EthernetAddress::BROADCAST
    } else {
        EthernetAddress(reply.client_mac())
    });
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

    // Both interfaces share SDK packet storage. Keep this reply while yielding
    // to the station stack, but bound how long a busy radio can delay the AP.
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        match device.transmit(&frame[..frame_len]) {
            Err(DriverError::WouldBlock | DriverError::OutOfMemory)
                if Instant::now() < deadline =>
            {
                Timer::after_millis(1).await;
            }
            result => return result,
        }
    }
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
    log_line(b"wifi_ap_sta start");

    let (mut driver, interfaces) = init_wifi(runtime);
    log_line(b"ap started");

    if let Err(error) = support::connect(&mut driver) {
        support::log_debug(&error);
        log_line(b"ap_sta err connect");
        loop {
            embassy_time::Timer::after(Duration::from_secs(60)).await;
        }
    }

    let mut stations = [esp8266_hal::wifi::ApStationInfo::default(); 10];
    match driver.connected_stations(&mut stations) {
        Ok(count) => log_station_count(count),
        Err(_error) => log_line(b"ap_sta err stations"),
    }

    log_line(b"network starting");
    let (stack, ap_device) = start_network(spawner, interfaces);
    run_networks(stack, ap_device).await
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    esp8266_hal::println!("panic {info}");
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
