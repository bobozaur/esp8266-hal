//! Provision with ESP-Touch, obtain an IPv4 address with smoltcp DHCP, then
//! acknowledge the sender over UDP. Provisioning stays active through the ACK.

#![no_std]
#![no_main]

mod support;

use core::fmt::Write;
use esp8266_hal::timer::TimerExt;
use esp8266_hal::uart::UART0Serial;
use esp8266_hal::wifi::{
    InitConfig, Interface as WifiInterface, SmartConfigCredentials, SmartConfigEvent,
    SmartConfigStartConfig, SmartConfigTimeout, SmartConfigType, Wifi,
};
use panic_halt as _;
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet, SocketStorage};
use smoltcp::socket::{dhcpv4, udp};
use smoltcp::time::{Duration, Instant};
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpCidr, IpEndpoint};
use static_cell::StaticCell;

type WifiDevice<'tx> = esp8266_hal::wifi::smoltcp::WifiDevice<'static, 'tx>;

const ACK_PORT: u16 = 18_266;
static WIFI: StaticCell<Wifi> = StaticCell::new();
static SOCKET_STORAGE: StaticCell<[SocketStorage<'static>; 2]> = StaticCell::new();
static UDP_RX_META: StaticCell<[udp::PacketMetadata; 1]> = StaticCell::new();
static UDP_TX_META: StaticCell<[udp::PacketMetadata; 1]> = StaticCell::new();
static UDP_RX: StaticCell<[u8; 1]> = StaticCell::new();
static UDP_TX: StaticCell<[u8; 11]> = StaticCell::new();

#[esp8266_hal::entry]
fn main() -> ! {
    let peripherals = esp8266_hal::target::Peripherals::take().unwrap();
    let mut serial = support::serial(peripherals.UART0, peripherals.GPIO);
    let (_, timer2) = peripherals.TIMER.timers();
    let runtime = support::init(timer2);
    let wifi = WIFI.init(
        Wifi::new(runtime, InitConfig::new(14, 4, 4).unwrap()).expect("Wi-Fi initialization"),
    );
    let (mut controller, interfaces) = wifi.split();
    controller.enable_station().expect("station mode");
    controller.start_blocking(1_000).expect("start Wi-Fi");

    let mut session = controller.smart_config().expect("SmartConfig session");
    session
        .set_type(SmartConfigType::EspTouch)
        .expect("ESP-Touch mode");
    session
        .set_timeout(SmartConfigTimeout::try_from(60).unwrap())
        .expect("provisioning timeout");
    session
        .start(&SmartConfigStartConfig::default())
        .expect("start SmartConfig");
    writeln!(serial, "Waiting for ESP-Touch credentials\r").unwrap();

    let credentials = loop {
        match session.next_event() {
            Ok(Some(SmartConfigEvent::GotCredentials(credentials))) => break credentials,
            Ok(Some(SmartConfigEvent::FoundChannel)) => {
                writeln!(serial, "Found sender channel\r").unwrap()
            }
            Ok(_) => {}
            Err(error) => {
                writeln!(serial, "Provisioning failed: {error:?}\r").unwrap();
                session.stop().expect("stop SmartConfig");
                idle();
            }
        }
        esp8266_hal::wifi::delay_ticks(5);
    };
    let config = credentials
        .client_config()
        .expect("received station credentials");
    session
        .configure_station(&config)
        .expect("station configuration");
    if let Err(error) = session.connect_blocking(30_000) {
        writeln!(serial, "Connection failed: {error:?}\r").unwrap();
        session.stop().expect("stop SmartConfig");
        idle();
    }
    match acknowledge(interfaces.station, &credentials, &mut serial) {
        Ok(()) => writeln!(serial, "ESP-Touch acknowledgement sent\r").unwrap(),
        Err(error) => writeln!(serial, "Acknowledgement failed: {error}\r").unwrap(),
    }
    session.stop().expect("stop SmartConfig");
    idle();
}

fn idle() -> ! {
    loop {
        esp8266_hal::wifi::delay_ticks(100);
    }
}

fn now() -> Instant {
    // SAFETY: callers run after the application's Wi-Fi runtime is initialized.
    Instant::from_millis(unsafe { esp8266_wifi_sys::esp_timer_get_time() } / 1_000)
}

fn acknowledge(
    link: WifiInterface<'static>,
    credentials: &SmartConfigCredentials,
    serial: &mut UART0Serial,
) -> Result<(), &'static str> {
    let mac = link.mac_address();
    let mut tx_frame = [0; esp8266_hal::wifi::MAX_ETHERNET_FRAME_SIZE];
    let mut device = WifiDevice::new(link, &mut tx_frame).expect("Ethernet frame storage");
    let mut iface = Interface::new(
        Config::new(HardwareAddress::Ethernet(EthernetAddress(mac))),
        &mut device,
        now(),
    );
    let mut sockets = SocketSet::new(&mut SOCKET_STORAGE.init([SocketStorage::EMPTY; 2])[..]);
    let dhcp = sockets.add(dhcpv4::Socket::new());
    writeln!(serial, "Waiting for DHCP\r").unwrap();
    let local_ip =
        wait_for_dhcp(&mut iface, &mut device, &mut sockets, dhcp).ok_or("DHCP timed out")?;
    writeln!(serial, "IPv4 address: {local_ip}\r").unwrap();

    let mut payload = [0; 11];
    payload[0] = credentials.token();
    payload[1..7].copy_from_slice(&mac);
    payload[7..11].copy_from_slice(&local_ip.octets());
    let remote = IpEndpoint::new(credentials.cellphone_ip().into(), ACK_PORT);
    let udp = udp::Socket::new(
        udp::PacketBuffer::new(
            &mut UDP_RX_META.init([udp::PacketMetadata::EMPTY; 1])[..],
            &mut UDP_RX.init([0; 1])[..],
        ),
        udp::PacketBuffer::new(
            &mut UDP_TX_META.init([udp::PacketMetadata::EMPTY; 1])[..],
            &mut UDP_TX.init([0; 11])[..],
        ),
    );
    let udp = sockets.add(udp);
    sockets
        .get_mut::<udp::Socket>(udp)
        .bind(ACK_PORT)
        .map_err(|_| "UDP bind failed")?;

    // Repeat the UDP acknowledgement because its transport has no delivery ACK.
    for _ in 0..5 {
        let deadline = now() + Duration::from_secs(5);
        loop {
            match sockets
                .get_mut::<udp::Socket>(udp)
                .send_slice(&payload, remote)
            {
                Ok(()) => break,
                Err(udp::SendError::Unaddressable) => return Err("sender is unreachable"),
                Err(udp::SendError::BufferFull) => {}
            }
            iface.poll(now(), &mut device, &mut sockets);
            if now() >= deadline {
                return Err("UDP send timed out");
            }
            esp8266_hal::wifi::delay_ticks(1);
        }
        while sockets.get::<udp::Socket>(udp).send_queue() != 0 {
            iface.poll(now(), &mut device, &mut sockets);
            if now() >= deadline {
                return Err("UDP flush timed out");
            }
            esp8266_hal::wifi::delay_ticks(1);
        }
        esp8266_hal::wifi::delay_ticks(10);
    }
    Ok(())
}

fn wait_for_dhcp(
    iface: &mut Interface,
    device: &mut WifiDevice<'_>,
    sockets: &mut SocketSet<'static>,
    dhcp: SocketHandle,
) -> Option<smoltcp::wire::Ipv4Address> {
    let deadline = now() + Duration::from_secs(30);
    while now() < deadline {
        iface.poll(now(), device, sockets);
        if let Some(dhcpv4::Event::Configured(config)) =
            sockets.get_mut::<dhcpv4::Socket>(dhcp).poll()
        {
            iface.update_ip_addrs(|addresses| {
                addresses.clear();
                addresses.push(IpCidr::Ipv4(config.address)).unwrap();
            });
            if let Some(router) = config.router {
                iface.routes_mut().add_default_ipv4_route(router).unwrap();
            }
            return Some(config.address.address());
        }
        esp8266_hal::wifi::delay_ticks(1);
    }
    None
}
