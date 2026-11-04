//! ESP8266 soft-AP start validation using the Wi-Fi controller.

#![no_std]
#![no_main]

use core::future::poll_fn;
use core::task::Poll;

use embassy_executor::Spawner;
use embassy_futures::select::select;
use embassy_time::{Duration, Instant, Timer};
mod support;
use esp8266_hal::timer::TimerExt;
use support::dhcp::{DhcpMessageType, DhcpReply, DhcpServer, DhcpServerConfig};
use support::dhcp_wire::{self, REPLY_HEADERS};
use support::log_line;

use esp8266_hal::wifi::{
    AccessPointConfig, ApClientLimit, BeaconInterval, Channel, Configuration, DriverError,
    InitConfig, Interface as WifiInterface, Ipv4Addr, Ssid, Wifi,
};
use static_cell::StaticCell;
use xtensa_lx_rt::exception::{Context, ExceptionCause};

const AP_SSID: &str = "esp8266-hal-ap";
const AP_IP: Ipv4Addr = Ipv4Addr::new(192, 168, 4, 1);
const AP_LEASE_START: Ipv4Addr = Ipv4Addr::new(192, 168, 4, 2);
const AP_NETMASK: Ipv4Addr = Ipv4Addr::new(255, 255, 255, 0);
const DHCP_PACKET_BYTES: usize = 288;
const AP_RX_FRAME_BYTES: usize = 512;
const DHCP_REPLY_FRAME_BYTES: usize = REPLY_HEADERS + DHCP_PACKET_BYTES;

type ApNetDevice = WifiInterface<'static>;

static WIFI: StaticCell<Wifi> = StaticCell::new();
static AP_DEVICE: StaticCell<ApNetDevice> = StaticCell::new();
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

fn ap_dhcp_grace_ms() -> Option<u64> {
    option_env!("ESP8266_HAL_AP_DHCP_GRACE_MS").and_then(|value| value.parse::<u64>().ok())
}

fn ap_dhcp_poll_ms() -> u64 {
    option_env!("ESP8266_HAL_AP_DHCP_POLL_MS")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(1000)
}

fn wait_before_dhcp_enabled() -> bool {
    matches!(option_env!("ESP8266_HAL_AP_WAIT_BEFORE_DHCP"), Some("1"))
}

async fn run_dhcp_server(device: &'static mut ApNetDevice) -> ! {
    log_stage(b"dhcp-task-enter");
    let config = DhcpServerConfig::new(AP_IP, AP_NETMASK, AP_LEASE_START, 4)
        .expect("dhcp config")
        .router(AP_IP)
        .dns_server(AP_IP)
        .lease_seconds(3600);
    let mut server = DhcpServer::<4>::new(config);
    let reply_frame = DHCP_REPLY_FRAME.init([0; DHCP_REPLY_FRAME_BYTES]);
    let rx_frame = AP_RX_FRAME.init([0; AP_RX_FRAME_BYTES]);
    let ap_mac = device.mac_address();
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
            let frame_len = match device.receive(rx_frame) {
                Ok(Some(frame_len)) => frame_len,
                Ok(None) => break,
                Err(_) => {
                    log_line(b"ap-rx-drop");
                    continue;
                }
            };
            match dhcp_wire::payload(&rx_frame[..frame_len]) {
                Some(packet) => match server.handle(
                    Instant::now().as_secs(),
                    packet,
                    &mut reply_frame[REPLY_HEADERS..],
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
            wait_for_ap_rx(device, Duration::from_millis(ap_dhcp_poll_ms())).await;
            continue;
        };

        if transmit_dhcp_reply(device, reply_frame, ap_mac, reply).is_err() {
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
        device.register_rx_waker(cx.waker());
        device.register_link_waker(cx.waker());
        if device.has_rx_data() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    });
    let _ = select(rx_ready, Timer::after(fallback)).await;
}

fn transmit_dhcp_reply(
    device: &mut ApNetDevice,
    frame: &mut [u8],
    ap_mac: [u8; 6],
    reply: DhcpReply,
) -> Result<(), DriverError> {
    let frame_len = dhcp_wire::emit_reply(frame, AP_IP, ap_mac, reply);

    device.transmit(&frame[..frame_len])
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

    let config = AccessPointConfig::open(
        Ssid::try_from(AP_SSID).expect("ap ssid"),
        Channel::try_from(6).expect("ap channel"),
    )
    .with_max_clients(ApClientLimit::try_from(4).expect("ap clients"))
    .with_beacon_interval(BeaconInterval::try_from(100).expect("beacon"));

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

    if wait_for_client_enabled() || wait_before_dhcp_enabled() {
        if wait_before_dhcp_enabled() {
            log_line(b"ap-association-ready");
        }
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

        if wait_before_dhcp_enabled() {
            log_line(b"ap-associated-before-dhcp");
        }
        if wait_for_client_enabled() {
            if driver.deauth_all().is_err() {
                log_line(b"ap deauth err");
                loop {
                    Timer::after(Duration::from_secs(60)).await;
                }
            }
            log_line(b"ap-deauth-ok");
            log_line(b"ap-station-list-ok");
        }
    }

    if dhcp_enabled() {
        log_line(b"ap network starting");
        let device = AP_DEVICE.init(interfaces.access_point);
        log_line(b"ap-net-up");
        log_line(b"ap-start-ok");
        run_dhcp_server(device).await;
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
