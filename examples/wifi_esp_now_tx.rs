//! ESP-NOW broadcast TX smoke validation using the Wi-Fi controller.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};

mod support;
use esp8266_hal::timer::TimerExt;
use support::log_line;

use esp8266_hal::wifi::{
    Channel, EspNowPeerConfig, EspNowSendStatus, EspNowSession, InitConfig, Wifi,
};
use static_cell::StaticCell;
use xtensa_lx_rt::exception::{Context, ExceptionCause};

type EspNowTxSession<'a> = EspNowSession<'a>;

static WIFI: StaticCell<Wifi> = StaticCell::new();
const ESP_NOW_PMK: [u8; 16] = [0x24; 16];

fn parse_peer_addr(value: &str) -> Option<[u8; 6]> {
    let mut parts = value.split(':');
    let mut address = [0; 6];
    for byte in &mut address {
        let part = parts.next()?;
        if part.len() != 2 || !part.bytes().all(|digit| digit.is_ascii_hexdigit()) {
            return None;
        }
        *byte = u8::from_str_radix(part, 16).ok()?;
    }
    parts.next().is_none().then_some(address)
}

fn encrypted_peer_enabled() -> bool {
    matches!(option_env!("ESP8266_HAL_ESPNOW_ENCRYPTED"), Some("1"))
}

#[inline(never)]
fn add_configured_peer(session: &mut EspNowTxSession<'_>, channel: Channel) -> Result<[u8; 6], ()> {
    if let Some(peer) = option_env!("ESP8266_HAL_ESPNOW_PEER").and_then(parse_peer_addr) {
        let peer_config = if encrypted_peer_enabled() {
            log_line(b"esp-now peer encrypted");
            EspNowPeerConfig::encrypted(peer, [0x42; 16]).channel(channel)
        } else {
            log_line(b"esp-now peer unicast");
            EspNowPeerConfig::unencrypted(peer).channel(channel)
        };
        session
            .add_peer(peer_config.for_station())
            .map_err(|_| ())?;
        return Ok(peer);
    }

    let broadcast = EspNowPeerConfig::broadcast().channel(channel);
    session.add_peer(broadcast.for_station()).map_err(|_| ())?;
    Ok(*broadcast.peer_addr())
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
    log_line(b"wifi_esp_now_tx start");

    let mut init = InitConfig::new(14, 4, 4).expect("init config");
    init.queues.esp_now_peers = 1;
    let wifi = WIFI.init(Wifi::new(runtime, init).expect("wifi init"));
    let (mut driver, _) = wifi.split();
    log_line(b"wifi init ok");

    match driver
        .enable_station()
        .and_then(|()| driver.start_blocking(1_000))
    {
        Ok(()) => {}
        Err(_error) => {
            log_line(b"esp-now start err");
            loop {
                Timer::after(Duration::from_secs(60)).await;
            }
        }
    };
    log_line(b"esp-now sta started");

    let mut session = match driver.esp_now() {
        Ok(session) => session,
        Err(_error) => {
            log_line(b"esp-now session err");
            loop {
                Timer::after(Duration::from_secs(60)).await;
            }
        }
    };

    let channel = Channel::try_from(6).expect("esp-now channel");
    if session.set_channel(channel).is_err() {
        log_line(b"esp-now channel err");
        loop {
            Timer::after(Duration::from_secs(60)).await;
        }
    }
    log_line(b"esp-now channel ok");

    if session.start().is_err() {
        log_line(b"esp-now start err");
        loop {
            Timer::after(Duration::from_secs(60)).await;
        }
    }
    if encrypted_peer_enabled() && session.set_pmk(&ESP_NOW_PMK).is_err() {
        log_line(b"esp-now pmk err");
        loop {
            Timer::after(Duration::from_secs(60)).await;
        }
    }

    let configured_peer = option_env!("ESP8266_HAL_ESPNOW_PEER").is_some();
    let peer_addr = match add_configured_peer(&mut session, channel) {
        Ok(peer_addr) => peer_addr,
        Err(()) => {
            log_line(b"esp-now peer err");
            loop {
                Timer::after(Duration::from_secs(60)).await;
            }
        }
    };
    log_line(b"esp-now peer ok");

    let mut saw_send_success = false;
    for _ in 0..8 {
        if session.send(&peer_addr, b"esp8266-hal-esp-now").is_err() {
            log_line(b"esp-now send err");
            loop {
                Timer::after(Duration::from_secs(60)).await;
            }
        }
        let mut completed = false;
        for _ in 0..100 {
            if let Some(status) = session.send_status() {
                esp8266_hal::logger::write_bytes(b"esp-now send status=");
                let status_text: &[u8] = match status {
                    EspNowSendStatus::Success => {
                        saw_send_success = true;
                        b"success"
                    }
                    EspNowSendStatus::Failed => b"failed",
                };
                esp8266_hal::logger::write_bytes(status_text);
                esp8266_hal::logger::write_newline();
                completed = true;
                break;
            }
            Timer::after(Duration::from_millis(20)).await;
        }
        if !completed {
            log_line(b"esp-now send status timeout");
            loop {
                Timer::after(Duration::from_secs(60)).await;
            }
        }
        Timer::after(Duration::from_millis(100)).await;
    }
    if configured_peer {
        if !saw_send_success {
            log_line(b"esp-now send status failed");
            loop {
                Timer::after(Duration::from_secs(60)).await;
            }
        }
        log_line(b"esp-now send-status-ok");
    } else {
        log_line(b"esp-now send-callback-ok");
    }

    let _ = session.stop();
    log_line(b"esp-now-tx-ok");

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
    esp8266_hal::logger::write_bytes(b"exception cause=");
    esp8266_hal::logger::write_dec_u32(cause as u32);
    let registers =
        unsafe { core::slice::from_raw_parts((&frame as *const Context).cast::<u32>(), 21) };
    esp8266_hal::logger::write_bytes(b" pc=0x");
    esp8266_hal::logger::write_hex_u32(registers[0]);
    esp8266_hal::logger::write_bytes(b" a0=0x");
    esp8266_hal::logger::write_hex_u32(registers[2]);
    esp8266_hal::logger::write_bytes(b" a1=0x");
    esp8266_hal::logger::write_hex_u32(registers[3]);
    esp8266_hal::logger::write_bytes(b" vaddr=0x");
    esp8266_hal::logger::write_hex_u32(registers[20]);
    esp8266_hal::logger::write_newline();
    esp8266_hal::logger::write_bytes(b"panic");
    esp8266_hal::logger::write_newline();
    loop {}
}
