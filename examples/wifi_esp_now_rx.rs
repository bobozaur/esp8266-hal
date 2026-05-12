//! ESP-NOW RX validation example using the typestate WiFi core.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};

use esp8266_hal::wifi::{
    Channel, Esp8266TargetBackend, EspNowPeerConfig, InitConfig, WifiDriver, WifiResources,
};
use static_cell::StaticCell;
use xtensa_lx_rt::exception::{Context, ExceptionCause};

type EspNowResources = WifiResources<8, 4, 2, 2, 2, 0>;

static WIFI_RESOURCES: StaticCell<EspNowResources> = StaticCell::new();

#[inline(always)]
fn log_line(message: &[u8]) {
    esp8266_hal::logger::write_bytes(message);
    esp8266_hal::logger::write_newline();
}

fn log_peer(peer_addr: &[u8; 6]) {
    esp8266_hal::logger::write_bytes(b"esp-now rx peer=");
    for (index, byte) in peer_addr.iter().copied().enumerate() {
        if index != 0 {
            esp8266_hal::logger::write_byte(b':');
        }
        esp8266_hal::logger::write_hex_u8(byte);
    }
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

#[esp8266_hal::main]
async fn main(_spawner: Spawner) {
    log_line(b"wifi_esp_now_rx start");

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

    if session
        .set_channel(Channel::try_from(6).expect("esp-now channel"))
        .is_err()
    {
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

    if let Some(peer) = option_env!("ESP8266_HAL_ESPNOW_PEER").and_then(parse_peer_addr) {
        let peer_config = if encrypted_peer_enabled() {
            log_line(b"esp-now rx peer encrypted");
            EspNowPeerConfig::encrypted(peer, [0x42; 16])
                .channel(Channel::try_from(6).expect("esp-now channel"))
        } else {
            log_line(b"esp-now rx peer unicast");
            EspNowPeerConfig::unencrypted(peer)
                .channel(Channel::try_from(6).expect("esp-now channel"))
        };
        if session.add_station_peer(peer_config.for_station()).is_err() {
            log_line(b"esp-now peer err");
            loop {
                Timer::after(Duration::from_secs(60)).await;
            }
        }
    }
    log_line(b"esp-now rx ready");

    loop {
        if let Some(frame) = session.receive() {
            log_peer(frame.peer_addr());
            esp8266_hal::logger::write_bytes(b"esp-now rx len=");
            esp8266_hal::logger::write_dec_u32(frame.payload().len() as u32);
            esp8266_hal::logger::write_newline();
            log_line(b"esp-now-rx-ok");
        }
        Timer::after(Duration::from_millis(20)).await;
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
