//! ESP8266 soft-AP start validation using the Wi-Fi controller.

#![no_std]
#![no_main]

use core::future::poll_fn;
use core::task::Poll;

use embassy_executor::Spawner;
use embassy_futures::select::select;
use embassy_time::{Duration, Instant, Timer};
use esp8266_hal::wifi::smoltcp::WifiDevice;
mod support;
use esp8266_hal::timer::TimerExt;
use support::dhcp::{DhcpMessageType, DhcpReply, DhcpServer, DhcpServerConfig};
use support::log_line;

use esp8266_hal::wifi::{
    AccessPointConfig, ApClientLimit, ApCredential, BeaconInterval, Channel, Configuration,
    DriverError, InitConfig, Interface as WifiInterface, Ipv4Addr, Ssid, Wifi,
};
use smoltcp::phy::ChecksumCapabilities;
use smoltcp::wire::{
    EthernetAddress, EthernetFrame, EthernetProtocol, IpAddress, IpProtocol,
    Ipv4Address as SmoltcpIpv4Address, Ipv4Packet, Ipv4Repr, UdpPacket, UdpRepr,
    ETHERNET_HEADER_LEN, IPV4_HEADER_LEN, UDP_HEADER_LEN,
};
use static_cell::StaticCell;
use xtensa_lx_rt::exception::{Context, ExceptionCause};

const AP_SSID: &str = "esp8266-hal-ap";
const AP_IP: Ipv4Addr = Ipv4Addr::new(192, 168, 4, 1);
const AP_LEASE_START: Ipv4Addr = Ipv4Addr::new(192, 168, 4, 2);
const AP_NETMASK: Ipv4Addr = Ipv4Addr::new(255, 255, 255, 0);
const DHCP_PACKET_BYTES: usize = 288;
const AP_RX_FRAME_BYTES: usize = 512;
const DHCP_REPLY_FRAME_BYTES: usize =
    ETHERNET_HEADER_LEN + IPV4_HEADER_LEN + UDP_HEADER_LEN + DHCP_PACKET_BYTES;
const DHCP_REPLY_HEADERS: usize = ETHERNET_HEADER_LEN + IPV4_HEADER_LEN + UDP_HEADER_LEN;

type ApNetDevice = WifiDevice<'static>;

struct ApNetRuntime {
    device: ApNetDevice,
}

static WIFI: StaticCell<Wifi> = StaticCell::new();
static AP_RUNTIME: StaticCell<ApNetRuntime> = StaticCell::new();
static AP_RX_FRAME: StaticCell<[u8; AP_RX_FRAME_BYTES]> = StaticCell::new();
static DHCP_REPLY_FRAME: StaticCell<[u8; DHCP_REPLY_FRAME_BYTES]> = StaticCell::new();

#[inline(never)]
fn log_station_count(count: usize) {
    esp8266_hal::logger::write_bytes(b"ap stations:");
    esp8266_hal::logger::write_dec_u32(count as u32);
    esp8266_hal::logger::write_newline();
}

#[inline(never)]
fn log_stage(stage: &[u8]) {
    esp8266_hal::logger::write_bytes(b"ap stage ");
    esp8266_hal::logger::write_bytes(stage);
    esp8266_hal::logger::write_newline();
}

#[inline(always)]
fn write_hex16(value: u16) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    esp8266_hal::logger::write_byte(HEX[((value >> 12) & 0x0f) as usize]);
    esp8266_hal::logger::write_byte(HEX[((value >> 8) & 0x0f) as usize]);
    esp8266_hal::logger::write_byte(HEX[((value >> 4) & 0x0f) as usize]);
    esp8266_hal::logger::write_byte(HEX[(value & 0x0f) as usize]);
}

fn wait_for_client_enabled() -> bool {
    matches!(option_env!("ESP8266_HAL_AP_WAIT_CLIENT"), Some("1"))
}

fn dhcp_enabled() -> bool {
    matches!(option_env!("ESP8266_HAL_AP_DHCP"), Some("1"))
}

fn ap_attach_only_enabled() -> bool {
    matches!(option_env!("ESP8266_HAL_AP_ATTACH_ONLY"), Some("1"))
}

fn ap_network_attach_only_enabled() -> bool {
    matches!(option_env!("ESP8266_HAL_AP_NET_ATTACH_ONLY"), Some("1"))
}

fn ap_dhcp_bind_only_enabled() -> bool {
    matches!(option_env!("ESP8266_HAL_AP_DHCP_BIND_ONLY"), Some("1"))
}

fn parse_u64_ascii(text: &str) -> Option<u64> {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return None;
    }

    let mut value = 0_u64;
    for &byte in bytes {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?;
        value = value.checked_add((byte - b'0') as u64)?;
    }

    Some(value)
}

fn ap_delay_before_network_ms() -> Option<u64> {
    option_env!("ESP8266_HAL_AP_DELAY_BEFORE_NET_MS").and_then(parse_u64_ascii)
}

fn ap_dhcp_grace_ms() -> Option<u64> {
    option_env!("ESP8266_HAL_AP_DHCP_GRACE_MS").and_then(parse_u64_ascii)
}

fn ap_dhcp_poll_ms() -> u64 {
    option_env!("ESP8266_HAL_AP_DHCP_POLL_MS")
        .and_then(parse_u64_ascii)
        .unwrap_or(1000)
}

async fn run_dhcp_server(runtime: &'static mut ApNetRuntime) -> ! {
    log_stage(b"dhcp-task-enter");
    if ap_dhcp_bind_only_enabled() {
        log_line(b"ap-dhcp-bind-ready");
        loop {
            Timer::after(Duration::from_secs(1)).await;
        }
    }

    let config = DhcpServerConfig::new(AP_IP, AP_NETMASK, AP_LEASE_START, 4)
        .expect("dhcp config")
        .router(AP_IP)
        .dns_server(AP_IP)
        .lease_seconds(3600);
    let mut server = DhcpServer::<4>::new(config);
    let reply_frame = DHCP_REPLY_FRAME.init([0; DHCP_REPLY_FRAME_BYTES]);
    let rx_frame = AP_RX_FRAME.init([0; AP_RX_FRAME_BYTES]);
    let ap_mac = EthernetAddress(runtime.device.mac_address());
    let mut last_wait_log = Instant::now();

    log_line(b"ap-dhcp-ready");
    if let Some(delay_ms) = ap_dhcp_grace_ms() {
        if delay_ms > 0 {
            log_line(b"ap-dhcp-grace");
            Timer::after(Duration::from_millis(delay_ms)).await;
            log_line(b"ap-dhcp-grace-done");
        }
    }
    loop {
        let mut reply = None;

        loop {
            let frame_len = match runtime.device.receive_frame(rx_frame) {
                Ok(Some(frame_len)) => frame_len,
                Ok(None) => break,
                Err(_) => {
                    log_line(b"ap-rx-drop");
                    continue;
                }
            };
            match dhcp_payload(&rx_frame[..frame_len]) {
                Some(packet) => match server.handle(
                    Instant::now().as_secs(),
                    packet,
                    &mut reply_frame[DHCP_REPLY_HEADERS..],
                ) {
                    Ok(result) => {
                        reply = result;
                        last_wait_log = Instant::now();
                    }
                    Err(_error) => log_line(b"ap-dhcp parse err"),
                },
                None => {
                    if last_wait_log.elapsed() >= Duration::from_secs(2) {
                        log_line(b"ap-dhcp non-dhcp");
                    }
                }
            }
            if reply.is_some() {
                break;
            }
        }

        let Some(reply) = reply else {
            if last_wait_log.elapsed() >= Duration::from_secs(2) {
                log_line(b"ap-dhcp-wait");
                last_wait_log = Instant::now();
            }
            wait_for_ap_rx(&runtime.device, Duration::from_millis(ap_dhcp_poll_ms())).await;
            continue;
        };

        if transmit_dhcp_reply(&mut runtime.device, reply_frame, ap_mac, reply).is_err() {
            log_line(b"ap-dhcp send err");
            continue;
        }

        match reply.message_type() {
            DhcpMessageType::Offer => log_line(b"ap-dhcp-offer"),
            DhcpMessageType::Ack => {
                log_line(b"ap-dhcp-ack");
                log_line(b"ap-dhcp-ok");
            }
            DhcpMessageType::Nak => log_line(b"ap-dhcp-nak"),
            _ => {}
        }
    }
}

async fn wait_for_ap_rx(device: &ApNetDevice, fallback: Duration) {
    if device.has_rx_data() {
        return;
    }

    let rx_ready = poll_fn(|cx| {
        device.register_waker(cx.waker());
        if device.has_rx_data() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    });
    let _ = select(rx_ready, Timer::after(fallback)).await;
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

    device.transmit_frame(&frame[..frame_len])
}

fn start_ap_network(
    link: WifiInterface<'static>,
) -> Result<&'static mut ApNetRuntime, &'static [u8]> {
    log_stage(b"net-link");
    let device = WifiDevice::new(link).map_err(|_| b"ap-net frame size err" as &'static [u8])?;

    log_stage(b"net-ready");
    Ok(AP_RUNTIME.init(ApNetRuntime { device }))
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
async fn app_main(_spawner: Spawner, runtime: &'static esp8266_hal::wifi::Runtime) {
    log_line(b"wifi_ap start");

    let mut init = InitConfig::default();
    init.queues.station_rx = 0;
    init.queues.access_point_rx = 2;
    init.queues.frame_bytes = AP_RX_FRAME_BYTES;
    let wifi = WIFI.init(Wifi::new(runtime, init).expect("wifi init"));
    let (mut driver, interfaces) = wifi.split();
    log_line(b"wifi init ok");

    let config = AccessPointConfig::builder(
        Ssid::try_from(AP_SSID).expect("ap ssid"),
        Channel::try_from(6).expect("ap channel"),
    )
    .credential(ApCredential::Open)
    .max_clients(ApClientLimit::try_from(4).expect("ap clients"))
    .beacon_interval(BeaconInterval::try_from(100).expect("beacon"))
    .build();

    log_stage(b"ap-configure");
    match driver.set_configuration(&Configuration::AccessPoint(config)) {
        Ok(()) => {}
        Err(_error) => {
            log_line(b"ap cfg err");
            loop {
                embassy_time::Timer::after(Duration::from_secs(60)).await;
            }
        }
    };
    log_stage(b"ap-start");
    match driver.start_blocking(1_000) {
        Ok(()) => {}
        Err(_error) => {
            log_line(b"ap start err");
            loop {
                embassy_time::Timer::after(Duration::from_secs(60)).await;
            }
        }
    };
    log_line(b"ap started");

    let mut stations = [esp8266_hal::wifi::ApStationInfo::default(); 10];
    let station_count = match driver.connected_stations(&mut stations) {
        Ok(count) => count,
        Err(_error) => {
            log_line(b"ap start err");
            loop {
                Timer::after(Duration::from_secs(60)).await;
            }
        }
    };
    log_station_count(station_count);

    if ap_attach_only_enabled() {
        let _link = interfaces.access_point;
        log_line(b"ap-link-ready");
        loop {
            Timer::after(Duration::from_secs(60)).await;
        }
    }

    if dhcp_enabled() {
        if let Some(delay_ms) = ap_delay_before_network_ms() {
            log_line(b"ap-delay-before-net");
            Timer::after(Duration::from_millis(delay_ms)).await;
            log_line(b"ap-delay-before-net-done");
        }
        log_line(b"ap network starting");
        let runtime = match start_ap_network(interfaces.access_point) {
            Ok(runtime) => runtime,
            Err(marker) => {
                log_line(marker);
                loop {
                    Timer::after(Duration::from_secs(60)).await;
                }
            }
        };
        log_line(b"ap-net-up");
        log_line(b"ap-start-ok");
        if ap_network_attach_only_enabled() {
            let _runtime = runtime;
            log_line(b"ap-net-attach-ready");
            loop {
                Timer::after(Duration::from_secs(1)).await;
            }
        }
        run_dhcp_server(runtime).await;
    }

    if wait_for_client_enabled() {
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut saw_client = false;
        while Instant::now() < deadline {
            match driver.connected_stations(&mut stations) {
                Ok(count) if count != 0 => {
                    log_station_count(count);
                    saw_client = true;
                    break;
                }
                Ok(_) => {}
                Err(_error) => {
                    log_line(b"ap start err");
                    loop {
                        Timer::after(Duration::from_secs(60)).await;
                    }
                }
            }
            Timer::after(Duration::from_millis(250)).await;
        }

        if !saw_client {
            log_line(b"ap wait client timeout");
            loop {
                Timer::after(Duration::from_secs(60)).await;
            }
        }

        if driver.deauth_all().is_err() {
            log_line(b"ap deauth err");
            loop {
                Timer::after(Duration::from_secs(60)).await;
            }
        }
        log_line(b"ap-deauth-ok");
        log_line(b"ap-station-list-ok");
    }

    log_line(b"ap-start-ok");

    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    esp8266_hal::logger::write_byte(b'P');
    if let Some(location) = info.location() {
        let path = location.file().as_bytes();
        let mut start = 0usize;
        let mut end = path.len();
        for (idx, &byte) in path.iter().enumerate() {
            if byte == b'/' {
                start = idx + 1;
                end = path.len();
            } else if byte == b'.' {
                end = idx;
            }
        }
        let stop = core::cmp::min(start + 6, end);
        for &byte in &path[start..stop] {
            esp8266_hal::logger::write_byte(byte);
        }
        esp8266_hal::logger::write_byte(b':');
        write_hex16(location.line() as u16);
    } else {
        esp8266_hal::logger::write_bytes(b"noloc");
    }
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
