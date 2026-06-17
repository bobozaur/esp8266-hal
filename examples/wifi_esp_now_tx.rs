//! ESP-NOW broadcast TX smoke validation using the typestate WiFi core.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};

use esp8266_hal::wifi::{
    Channel, Esp8266TargetBackend, EspNowPeerConfig, EspNowSendStatus, EspNowSession, InitConfig,
    StaMode, WifiDriver, WifiResources,
};
use static_cell::StaticCell;
use xtensa_lx_rt::exception::{Context, ExceptionCause};

type EspNowResources = WifiResources<8, 4, 2, 2, 2, 0>;
type EspNowBackend = Esp8266TargetBackend<'static, 8, 4, 2, 2, 2, 0>;
type EspNowTxSession<'a> = EspNowSession<'a, StaMode, EspNowBackend, 8, 4, 2, 2, 2, 0>;

static WIFI_RESOURCES: StaticCell<EspNowResources> = StaticCell::new();

#[inline(always)]
fn log_line(message: &[u8]) {
    esp8266_hal::logger::write_bytes(message);
    esp8266_hal::logger::write_newline();
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn parse_peer_addr(value: &str) -> Option<[u8; 6]> {
    let bytes = value.as_bytes();
    let mut out = [0u8; 6];
    let mut byte_index = 0;
    let mut offset = 0;

    while byte_index < out.len() {
        if offset + 2 > bytes.len() {
            return None;
        }
        let high = hex_value(bytes[offset])?;
        let low = hex_value(bytes[offset + 1])?;
        out[byte_index] = (high << 4) | low;
        offset += 2;
        byte_index += 1;

        if byte_index < out.len() {
            if bytes.get(offset).copied() != Some(b':') {
                return None;
            }
            offset += 1;
        }
    }

    if offset == bytes.len() {
        Some(out)
    } else {
        None
    }
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
            .add_station_peer(peer_config.for_station())
            .map_err(|_| ())?;
        return Ok(peer);
    }

    let broadcast = EspNowPeerConfig::broadcast().channel(channel);
    session
        .add_station_peer(broadcast.for_station())
        .map_err(|_| ())?;
    Ok(*broadcast.peer_addr())
}

#[esp8266_hal::main]
async fn main(_spawner: Spawner) {
    log_line(b"wifi_esp_now_tx start");

    let resources = WIFI_RESOURCES.init_with(WifiResources::new);
    let backend = Esp8266TargetBackend::new(resources);
    let driver = WifiDriver::with_backend(resources, backend)
        .initialize(InitConfig::new(14, 4, 4).expect("init config"))
        .expect("wifi init");
    log_line(b"wifi init ok");

    let mut driver = match driver.enable_sta().and_then(|driver| driver.start()) {
        Ok(driver) => driver,
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

    for _ in 0..8 {
        if session.send(&peer_addr, b"esp8266-hal-esp-now").is_err() {
            log_line(b"esp-now send err");
            loop {
                Timer::after(Duration::from_secs(60)).await;
            }
        }
        Timer::after(Duration::from_millis(100)).await;
    }

    let mut saw_send_callback = false;
    let mut saw_send_success = false;
    for _ in 0..100 {
        while let Some(status) = session.send_status() {
            esp8266_hal::logger::write_bytes(b"esp-now send status=");
            let status_text: &[u8] = match status.status() {
                EspNowSendStatus::Success => {
                    saw_send_success = true;
                    b"success"
                }
                EspNowSendStatus::Failed => b"failed",
            };
            esp8266_hal::logger::write_bytes(status_text);
            esp8266_hal::logger::write_newline();
            saw_send_callback = true;
        }
        if saw_send_success {
            break;
        }
        Timer::after(Duration::from_millis(20)).await;
    }

    if !saw_send_callback {
        log_line(b"esp-now send status timeout");
        loop {
            Timer::after(Duration::from_secs(60)).await;
        }
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
