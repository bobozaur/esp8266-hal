//! SmartConfig provisioning example using the Wi-Fi controller.

#![no_std]
#![no_main]

mod support;
use esp8266_hal::timer::TimerExt;
use support::log_line;

use esp8266_hal::wifi::{
    Controller, InitConfig, Interface as WifiInterface, Ipv4Addr as CoreIpv4Addr,
    SmartConfigCredentials, SmartConfigEvent, SmartConfigStartConfig, SmartConfigTimeout,
    SmartConfigType, Wifi,
};
use smoltcp::iface::{Config as SmolConfig, Interface, SocketHandle, SocketSet, SocketStorage};
use smoltcp::socket::{dhcpv4, udp};
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpAddress, IpCidr, IpEndpoint, Ipv4Address};
use static_cell::StaticCell;
use xtensa_lx_rt::exception::{Context, ExceptionCause};

const UDP_BUF_BYTES: usize = 32;
const ESPTOUCH_ACK_PORT: u16 = 18_266;
const DHCP_TIMEOUT_MS: u64 = 30_000;
const ACK_TIMEOUT_MS: u64 = 5_000;
const STATIC_ACK_IP: CoreIpv4Addr = CoreIpv4Addr::new(192, 168, 4, 2);
const STATIC_ACK_ENABLED: bool = option_env!("ESP8266_HAL_SMARTCONFIG_STATIC_ACK").is_some();

type SmartConfigDevice = esp8266_hal::wifi::smoltcp::WifiDevice<'static>;

static WIFI: StaticCell<Wifi> = StaticCell::new();
static SOCKET_STORAGE: StaticCell<[SocketStorage<'static>; 3]> = StaticCell::new();
static UDP_RX_METADATA: StaticCell<[udp::PacketMetadata; 1]> = StaticCell::new();
static UDP_TX_METADATA: StaticCell<[udp::PacketMetadata; 1]> = StaticCell::new();
static UDP_RX_BUFFER: StaticCell<[u8; 1]> = StaticCell::new();
static UDP_TX_BUFFER: StaticCell<[u8; UDP_BUF_BYTES]> = StaticCell::new();

fn idle_forever() -> ! {
    loop {
        esp8266_hal::wifi::delay_ticks(100);
    }
}

fn provision_with_smartconfig(
    driver: &mut Controller<'static>,
    link: WifiInterface<'static>,
) -> Result<(), ()> {
    let mut session = match driver.smart_config() {
        Ok(session) => session,
        Err(_error) => {
            log_line(b"smartconfig session err");
            return Err(());
        }
    };

    let _ = session.set_type(SmartConfigType::EspTouch);
    let _ = session.set_timeout(SmartConfigTimeout::try_from(15).expect("smartconfig timeout"));

    if session.start(&SmartConfigStartConfig::default()).is_err() {
        log_line(b"smartconfig start err");
        return Err(());
    }
    log_line(b"smartconfig ready");

    loop {
        match session.next_event() {
            Ok(Some(SmartConfigEvent::ScanDone)) => {
                log_line(b"smartconfig scan done");
            }
            Ok(Some(SmartConfigEvent::FoundChannel)) => {
                log_line(b"smartconfig found channel");
            }
            Ok(Some(SmartConfigEvent::GotCredentials(credentials))) => {
                esp8266_hal::logger::write_bytes(b"smartconfig credentials ssid-len=");
                esp8266_hal::logger::write_dec_u32(credentials.ssid().len() as u32);
                esp8266_hal::logger::write_newline();
                let config = match credentials.client_config() {
                    Ok(config) => config,
                    Err(_error) => {
                        log_line(b"smartconfig credentials err");
                        continue;
                    }
                };
                if session.configure_station(&config).is_err() {
                    log_line(b"smartconfig configure err");
                    continue;
                }
                if session.connect_blocking(30_000).is_err() {
                    log_line(b"smartconfig connect err");
                    continue;
                }
                log_line(b"smartconfig connected");
                log_line(b"smartconfig-credentials-ok");
                // Keep provisioning active through DHCP and the ACK, as the SDK does.
                let result = run_network_ack(link, &credentials);
                if result.is_err() {
                    log_line(b"smartconfig ack err");
                }
                if session.stop().is_err() {
                    log_line(b"smartconfig stop err");
                    return Err(());
                }
                return result;
            }
            Ok(Some(SmartConfigEvent::SendAckDone)) => {
                log_line(b"smartconfig sdk ack done");
            }
            Err(error) => {
                support::log_debug(&error);
                log_line(b"provisioning event err");
            }
            _ => {}
        }
        esp8266_hal::wifi::delay_ticks(5);
    }
}

fn now() -> Instant {
    let us = unsafe { esp8266_wifi_sys::esp_timer_get_time() };
    Instant::from_millis(us / 1000)
}

fn elapsed_ms_since(start: Instant) -> u64 {
    (now() - start).total_millis()
}

fn log_dhcp_up(address: [u8; 4], prefix_len: u8) {
    esp8266_hal::logger::write_bytes(b"smartconfig dhcp up ");
    esp8266_hal::logger::write_ipv4_octets(address);
    esp8266_hal::logger::write_bytes(b"/");
    esp8266_hal::logger::write_dec_u32(prefix_len as u32);
    esp8266_hal::logger::write_newline();
}

fn run_network_ack(
    link: WifiInterface<'static>,
    credentials: &SmartConfigCredentials,
) -> Result<(), ()> {
    log_line(b"smartconfig net start");
    let sta_mac = link.mac_address();

    let mut device = SmartConfigDevice::new(link).expect("valid station frame capacity");
    let config = SmolConfig::new(HardwareAddress::Ethernet(EthernetAddress(sta_mac)));
    let mut iface = Interface::new(config, &mut device, now());
    log_line(b"smartconfig net iface");
    iface.update_ip_addrs(|addrs| {
        let _ = addrs.push(IpCidr::new(Ipv4Address::UNSPECIFIED.into(), 0));
    });

    let storage = SOCKET_STORAGE.init([SocketStorage::EMPTY; 3]);
    let mut sockets = SocketSet::new(&mut storage[..]);
    let dhcp_handle = sockets.add(dhcpv4::Socket::new());
    log_line(b"smartconfig dhcp socket");

    let local_ip = if STATIC_ACK_ENABLED {
        let address = STATIC_ACK_IP.octets();
        iface.update_ip_addrs(|addresses| {
            addresses.clear();
            let _ = addresses.push(IpCidr::new(Ipv4Address::from(address).into(), 24));
        });
        address
    } else {
        match wait_for_dhcp(&mut iface, &mut device, &mut sockets, dhcp_handle) {
            Some(address) => address,
            None => {
                log_line(b"smartconfig dhcp err");
                return Err(());
            }
        }
    };

    send_esptouch_ack(
        &mut iface,
        &mut device,
        &mut sockets,
        credentials,
        sta_mac,
        local_ip,
    )
}

fn wait_for_dhcp(
    iface: &mut Interface,
    device: &mut SmartConfigDevice,
    sockets: &mut SocketSet<'static>,
    dhcp_handle: SocketHandle,
) -> Option<[u8; 4]> {
    log_line(b"smartconfig waiting dhcp");
    let start = now();

    loop {
        let _ = iface.poll(now(), device, sockets);
        {
            let dhcp = sockets.get_mut::<dhcpv4::Socket>(dhcp_handle);
            if let Some(dhcpv4::Event::Configured(config)) = dhcp.poll() {
                iface.update_ip_addrs(|addrs| {
                    if let Some(addr) = addrs.iter_mut().next() {
                        *addr = IpCidr::Ipv4(config.address);
                    }
                });
                if let Some(router) = config.router {
                    iface.routes_mut().add_default_ipv4_route(router).ok();
                }
            }
        }

        if let Some(ip) = iface.ipv4_addr() {
            if !ip.is_unspecified() {
                let prefix_len = iface
                    .ip_addrs()
                    .iter()
                    .find_map(|cidr| match cidr {
                        IpCidr::Ipv4(v4) => Some(v4.prefix_len()),
                        #[allow(unreachable_patterns)]
                        _ => None,
                    })
                    .unwrap_or(24);
                let octets = ip.octets();
                log_dhcp_up(octets, prefix_len);
                return Some(octets);
            }
        }

        if elapsed_ms_since(start) >= DHCP_TIMEOUT_MS {
            log_line(b"smartconfig dhcp timeout");
            return None;
        }

        esp8266_hal::wifi::delay_ticks(1);
    }
}

fn send_esptouch_ack(
    iface: &mut Interface,
    device: &mut SmartConfigDevice,
    sockets: &mut SocketSet<'static>,
    credentials: &SmartConfigCredentials,
    sta_mac: [u8; 6],
    local_ip: [u8; 4],
) -> Result<(), ()> {
    let rx_meta = UDP_RX_METADATA.init([udp::PacketMetadata::EMPTY; 1]);
    let tx_meta = UDP_TX_METADATA.init([udp::PacketMetadata::EMPTY; 1]);
    let rx_buf = UDP_RX_BUFFER.init([0; 1]);
    let tx_buf = UDP_TX_BUFFER.init([0; UDP_BUF_BYTES]);
    let rx_buffer = udp::PacketBuffer::new(&mut rx_meta[..], &mut rx_buf[..]);
    let tx_buffer = udp::PacketBuffer::new(&mut tx_meta[..], &mut tx_buf[..]);
    let udp_handle = sockets.add(udp::Socket::new(rx_buffer, tx_buffer));

    let mut payload = [0u8; 11];
    payload[0] = credentials.token();
    payload[1..7].copy_from_slice(&sta_mac);
    payload[7..11].copy_from_slice(&local_ip);

    let phone = credentials.cellphone_ip().octets();
    esp8266_hal::logger::write_bytes(b"smartconfig ack target ");
    esp8266_hal::logger::write_ipv4_octets(phone);
    esp8266_hal::logger::write_byte(b':');
    esp8266_hal::logger::write_dec_u32(ESPTOUCH_ACK_PORT as u32);
    esp8266_hal::logger::write_newline();

    let remote = IpEndpoint::new(
        IpAddress::Ipv4(Ipv4Address::new(phone[0], phone[1], phone[2], phone[3])),
        ESPTOUCH_ACK_PORT,
    );

    {
        let socket = sockets.get_mut::<udp::Socket>(udp_handle);
        if socket.bind(ESPTOUCH_ACK_PORT).is_err() {
            log_line(b"smartconfig ack bind err");
            return Err(());
        }
    }

    for _ in 0..5 {
        let send_start = now();
        loop {
            let send_result = {
                let socket = sockets.get_mut::<udp::Socket>(udp_handle);
                socket.send_slice(&payload, remote)
            };
            match send_result {
                Ok(()) => break,
                Err(udp::SendError::BufferFull) => {}
                Err(udp::SendError::Unaddressable) => {
                    log_line(b"smartconfig ack send err");
                    return Err(());
                }
            }

            let _ = iface.poll(now(), device, sockets);
            if elapsed_ms_since(send_start) >= ACK_TIMEOUT_MS {
                log_line(b"smartconfig ack send err");
                return Err(());
            }
            esp8266_hal::wifi::delay_ticks(1);
        }

        let flush_start = now();
        loop {
            let _ = iface.poll(now(), device, sockets);
            let queued = sockets.get::<udp::Socket>(udp_handle).send_queue();
            if queued == 0 {
                break;
            }
            if elapsed_ms_since(flush_start) >= ACK_TIMEOUT_MS {
                log_line(b"smartconfig ack send err");
                return Err(());
            }
            esp8266_hal::wifi::delay_ticks(1);
        }

        esp8266_hal::wifi::delay_ticks(10);
    }

    log_line(b"smartconfig ack sent");
    Ok(())
}

#[esp8266_hal::main]
fn main() -> ! {
    let peripherals = esp8266_hal::target::Peripherals::take().expect("peripherals");
    let (_, timer2) = peripherals.TIMER.timers();
    let runtime = support::init(timer2);
    log_line(b"wifi_smartconfig start");

    let init = InitConfig::new(14, 4, 4).expect("init config");

    let wifi = WIFI.init(Wifi::new(runtime, init).expect("wifi init"));
    let (mut driver, interfaces) = wifi.split();
    esp8266_hal::logger::write_bytes(b"smartconfig station mac=");
    for (index, byte) in interfaces.station.mac_address().into_iter().enumerate() {
        if index != 0 {
            esp8266_hal::logger::write_byte(b':');
        }
        esp8266_hal::logger::write_hex_u8(byte);
    }
    esp8266_hal::logger::write_newline();
    log_line(b"wifi init ok");

    match driver
        .enable_station()
        .and_then(|()| driver.start_blocking(1_000))
    {
        Ok(()) => {}
        Err(_error) => {
            log_line(b"smartconfig sta start err");
            idle_forever();
        }
    };
    log_line(b"smartconfig sta started");

    if provision_with_smartconfig(&mut driver, interfaces.station).is_err() {
        idle_forever();
    }
    log_line(b"smartconfig-ok");
    idle_forever();
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    esp8266_hal::println!("panic {}", info);
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
