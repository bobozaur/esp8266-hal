//! SmartConfig provisioning example using the typestate WiFi core.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};

use esp8266_hal::wifi::{
    Esp8266TargetBackend, InitConfig, SmartConfigStartConfig, SmartConfigTimeout, SmartConfigType,
    WifiDriver, WifiEvent, WifiResources,
};
use static_cell::StaticCell;
use xtensa_lx_rt::exception::{Context, ExceptionCause};

type SmartConfigResources = WifiResources<8, 4, 2, 0, 0, 0>;

static WIFI_RESOURCES: StaticCell<SmartConfigResources> = StaticCell::new();

#[inline(always)]
fn log_line(message: &[u8]) {
    esp8266_hal::logger::write_bytes(message);
    esp8266_hal::logger::write_newline();
}

#[esp8266_hal::main]
async fn main(_spawner: Spawner) {
    log_line(b"wifi_smartconfig start");

    let resources = WIFI_RESOURCES.init(WifiResources::new());
    let backend = Esp8266TargetBackend::new(resources);
    let driver = WifiDriver::with_backend(resources, backend)
        .initialize(InitConfig::default())
        .expect("wifi init");
    log_line(b"wifi init ok");

    let mut driver = match driver.enable_sta().and_then(|driver| driver.start()) {
        Ok(driver) => driver,
        Err(_error) => {
            log_line(b"smartconfig sta start err");
            loop {
                Timer::after(Duration::from_secs(60)).await;
            }
        }
    };
    log_line(b"smartconfig sta started");

    let mut session = match driver.smart_config() {
        Ok(session) => session,
        Err(_error) => {
            log_line(b"smartconfig session err");
            loop {
                Timer::after(Duration::from_secs(60)).await;
            }
        }
    };

    let _ = session.set_type(SmartConfigType::EspTouch);
    let _ = session.set_timeout(SmartConfigTimeout::try_from(15).expect("smartconfig timeout"));

    if session.start(&SmartConfigStartConfig::default()).is_err() {
        log_line(b"smartconfig start err");
        loop {
            Timer::after(Duration::from_secs(60)).await;
        }
    }
    log_line(b"smartconfig ready");

    loop {
        match session.next_event() {
            Some(WifiEvent::SmartConfigScanDone) => log_line(b"smartconfig scan done"),
            Some(WifiEvent::SmartConfigFoundChannel) => log_line(b"smartconfig found channel"),
            Some(WifiEvent::SmartConfigGotCredentials { credentials }) => {
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
                let _ = session.stop();
                log_line(b"smartconfig-ok");
            }
            Some(WifiEvent::SmartConfigSendAckDone) => {
                let _ = session.stop();
                log_line(b"smartconfig-ok");
            }
            _ => {}
        }
        Timer::after(Duration::from_millis(50)).await;
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
