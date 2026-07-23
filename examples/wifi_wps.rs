//! WPS provisioning example using the typestate WiFi core.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};

use esp8266_hal::wifi::{
    DriverError, InitConfig, Ssid, WifiDriver, WifiResources, WpsConfig, WpsEvent, WpsPin,
};
use static_cell::StaticCell;
use xtensa_lx_rt::exception::{Context, ExceptionCause};

type WpsResources = WifiResources<8, 4, 0, 0, 0>;
const DEFAULT_MAX_WPS_ATTEMPTS: u8 = 8;

static WIFI_RESOURCES: StaticCell<WpsResources> = StaticCell::new();

#[inline(always)]
fn log_line(message: &[u8]) {
    esp8266_hal::logger::write_bytes(message);
    esp8266_hal::logger::write_newline();
}

fn log_disconnect_reason(resources: &WpsResources) {
    esp8266_hal::logger::write_bytes(b" disconnect=");
    esp8266_hal::logger::write_dec_u32(resources.last_disconnect_reason().unwrap_or(0) as u32);
}

const fn should_retry_wps_disconnect(reason: Option<u8>) -> bool {
    matches!(reason, Some(2 | 4 | 8 | 23 | 201) | None)
}

fn selected_max_wps_attempts() -> u8 {
    match option_env!("ESP8266_HAL_WPS_MAX_ATTEMPTS") {
        Some("1") => 1,
        Some("2") => 2,
        Some("3") => 3,
        Some("4") => 4,
        Some("5") => 5,
        Some("6") => 6,
        Some("7") => 7,
        Some("8") => 8,
        _ => DEFAULT_MAX_WPS_ATTEMPTS,
    }
}

#[esp8266_hal::main]
async fn main(_spawner: Spawner) {
    log_line(b"wifi_wps start");

    let resources = WIFI_RESOURCES.init_with(WifiResources::new);
    let init_config = InitConfig::new(14, 4, 4).expect("init config");
    let driver = WifiDriver::new(resources)
        .initialize(init_config)
        .expect("wifi init");
    log_line(b"wifi init ok");

    let mut driver = match driver.enable_sta().and_then(|driver| driver.start()) {
        Ok(driver) => driver,
        Err(_error) => {
            log_line(b"wps sta start err");
            loop {
                Timer::after(Duration::from_secs(60)).await;
            }
        }
    };
    log_line(b"wps sta started");

    let mut session = match driver.wps() {
        Ok(session) => session,
        Err(_error) => {
            log_line(b"wps session err");
            loop {
                Timer::after(Duration::from_secs(60)).await;
            }
        }
    };

    let target_ssid = Ssid::try_from("esp8266-hal-wps").expect("wps target ssid");
    let config = match option_env!("ESP8266_HAL_WPS_PIN") {
        Some(pin) => WpsConfig::fixed_pin(WpsPin::try_from(pin).expect("wps pin"))
            .with_target_ssid(&target_ssid),
        None => WpsConfig::pbc().with_target_ssid(&target_ssid),
    };
    let mut wps_attempt = 1_u8;
    let max_wps_attempts = selected_max_wps_attempts();
    esp8266_hal::logger::write_bytes(b"wps max-attempts=");
    esp8266_hal::logger::write_dec_u32(max_wps_attempts as u32);
    esp8266_hal::logger::write_newline();
    if let Err(error) = session.start(&config) {
        esp8266_hal::logger::write_bytes(b"wps start detail");
        match error {
            DriverError::BackendError(code) => {
                esp8266_hal::logger::write_bytes(b" code=");
                if code < 0 {
                    esp8266_hal::logger::write_byte(b'-');
                    esp8266_hal::logger::write_dec_u32(code.unsigned_abs());
                } else {
                    esp8266_hal::logger::write_dec_u32(code as u32);
                }
            }
            DriverError::BackendRejected => esp8266_hal::logger::write_bytes(b" rejected"),
            DriverError::Timeout => esp8266_hal::logger::write_bytes(b" timeout"),
            DriverError::StaStartTimeout => esp8266_hal::logger::write_bytes(b" sta-start-timeout"),
            DriverError::Disconnected => esp8266_hal::logger::write_bytes(b" disconnected"),
            DriverError::EventOverflow => esp8266_hal::logger::write_bytes(b" event-overflow"),
            DriverError::PacketOverflow => esp8266_hal::logger::write_bytes(b" packet-overflow"),
            DriverError::PeerOverflow => esp8266_hal::logger::write_bytes(b" peer-overflow"),
            DriverError::PayloadTooLong => esp8266_hal::logger::write_bytes(b" payload-too-long"),
            DriverError::InvalidFrameSize => {
                esp8266_hal::logger::write_bytes(b" invalid-frame-size")
            }
            DriverError::OutOfMemory => esp8266_hal::logger::write_bytes(b" out-of-memory"),
            DriverError::NotStarted => esp8266_hal::logger::write_bytes(b" not-started"),
            DriverError::WouldBlock => esp8266_hal::logger::write_bytes(b" would-block"),
        }
        esp8266_hal::logger::write_newline();
        log_line(b"wps start err");
        loop {
            Timer::after(Duration::from_secs(60)).await;
        }
    }
    log_line(b"wps ready");

    loop {
        match session.next_event() {
            Some(WpsEvent::Succeeded) => {
                let _ = session.stop();
                esp8266_hal::logger::write_bytes(b"wps-ok");
                esp8266_hal::logger::write_newline();
                loop {
                    Timer::after(Duration::from_secs(60)).await;
                }
            }
            Some(WpsEvent::Failed { reason }) => {
                if wps_attempt < max_wps_attempts {
                    let _ = session.stop();
                    Timer::after(Duration::from_millis(1000)).await;
                    wps_attempt += 1;
                    esp8266_hal::logger::write_bytes(b"wps-fail-retry attempt=");
                    esp8266_hal::logger::write_dec_u32(wps_attempt as u32);
                    esp8266_hal::logger::write_newline();
                    if session.start(&config).is_ok() {
                        continue;
                    }
                }
                esp8266_hal::logger::write_bytes(b"wps-fail-detail");
                if let Some(reason) = reason {
                    esp8266_hal::logger::write_bytes(b" reason=");
                    esp8266_hal::logger::write_dec_u32(reason);
                }
                log_disconnect_reason(resources);
                esp8266_hal::logger::write_bytes(b" wps-failed");
                esp8266_hal::logger::write_newline();
            }
            Some(WpsEvent::TimedOut) => {
                if wps_attempt < max_wps_attempts {
                    let _ = session.stop();
                    Timer::after(Duration::from_millis(750)).await;
                    wps_attempt += 1;
                    esp8266_hal::logger::write_bytes(b"wps-retry attempt=");
                    esp8266_hal::logger::write_dec_u32(wps_attempt as u32);
                    esp8266_hal::logger::write_newline();
                    if session.start(&config).is_ok() {
                        continue;
                    }
                }
                esp8266_hal::logger::write_bytes(b"wps-timeout-detail");
                log_disconnect_reason(resources);
                esp8266_hal::logger::write_bytes(b" wps-timeout");
                esp8266_hal::logger::write_newline();
            }
            Some(WpsEvent::Pin { pin }) => {
                esp8266_hal::logger::write_bytes(b"wps pin=");
                esp8266_hal::logger::write_bytes(pin.as_bytes());
                esp8266_hal::logger::write_newline();
            }
            Some(WpsEvent::Disconnected { reason }) => {
                esp8266_hal::logger::write_bytes(b"wps-sta-disconnect");
                if let Some(reason) = reason {
                    esp8266_hal::logger::write_bytes(b" reason=");
                    esp8266_hal::logger::write_dec_u32(reason as u32);
                }
                esp8266_hal::logger::write_newline();
                if should_retry_wps_disconnect(reason) && wps_attempt < max_wps_attempts {
                    let _ = session.stop();
                    Timer::after(Duration::from_millis(750)).await;
                    wps_attempt += 1;
                    esp8266_hal::logger::write_bytes(b"wps-retry attempt=");
                    esp8266_hal::logger::write_dec_u32(wps_attempt as u32);
                    esp8266_hal::logger::write_newline();
                    if session.start(&config).is_err() {
                        esp8266_hal::logger::write_bytes(b"wps start err");
                        esp8266_hal::logger::write_newline();
                    }
                } else if !should_retry_wps_disconnect(reason) {
                    esp8266_hal::logger::write_bytes(b"wps-fail-detail");
                    log_disconnect_reason(resources);
                    esp8266_hal::logger::write_bytes(b" wps-failed");
                    esp8266_hal::logger::write_newline();
                }
            }
            _ => {}
        }
        Timer::after(Duration::from_millis(50)).await;
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    esp8266_hal::logger::write_bytes(b"P! ");
    if let Some(location) = info.location() {
        esp8266_hal::logger::write_bytes(location.file().as_bytes());
        esp8266_hal::logger::write_byte(b':');
        esp8266_hal::logger::write_dec_u32(location.line());
    } else {
        esp8266_hal::logger::write_bytes(b"no-location");
    }
    esp8266_hal::logger::write_newline();
    esp8266_hal::logger::write_bytes(b"panic");
    esp8266_hal::logger::write_newline();
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
