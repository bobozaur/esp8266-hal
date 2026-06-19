#![no_std]
#![no_main]

use panic_halt as _;

use esp8266_hal::wifi::{
    Channel, ClientConfig, Credential, EnterpriseIdentity, EnterprisePassword, EnterpriseUsername,
    Esp8266TargetBackend, InitConfig, LinkState, Ssid, WifiDriver, WifiResources,
    Wpa2EnterpriseCredentials,
};
use static_cell::StaticCell;

const EAP_SSID: &str = match option_env!("ESP8266_HAL_EAP_SSID") {
    Some(ssid) => ssid,
    None => "esp8266-router-eap",
};
const EAP_IDENTITY: &str = match option_env!("ESP8266_HAL_EAP_IDENTITY") {
    Some(identity) => identity,
    None => "esp8266-user",
};
const EAP_USERNAME: &str = match option_env!("ESP8266_HAL_EAP_USERNAME") {
    Some(username) => username,
    None => "esp8266-user",
};
const EAP_PASSWORD: &str = match option_env!("ESP8266_HAL_EAP_PASSWORD") {
    Some(password) => password,
    None => "esp8266-password",
};
const EAP_CHANNEL: &str = match option_env!("ESP8266_HAL_EAP_CHANNEL") {
    Some(channel) => channel,
    None => "11",
};
const EAP_EXPECT_FAILURE: bool = match option_env!("ESP8266_HAL_EAP_EXPECT_FAILURE") {
    Some(_) => true,
    None => false,
};

type EapResources = WifiResources<0, 0, 1>;

static RESOURCES: StaticCell<EapResources> = StaticCell::new();

#[esp8266_hal::main]
fn main() -> ! {
    log_line(b"wifi_eap start");

    let resources = RESOURCES.init_with(WifiResources::new);
    log_line(b"wifi eap resources");
    log_heap(b"wifi eap heap before init");

    let backend = Esp8266TargetBackend::new(resources);
    let driver = WifiDriver::with_backend(resources, backend);
    log_line(b"wifi eap before init");
    let init_config = InitConfig::new(14, 4, 4).expect("init config");
    let driver = match driver.initialize(init_config) {
        Ok(driver) => {
            log_line(b"wifi eap initialized");
            log_heap(b"wifi eap heap after init");
            driver
        }
        Err(error) => {
            log_init_error(b"wifi eap init err", error);
            loop_delay();
        }
    };

    let credentials = Wpa2EnterpriseCredentials::new(
        EnterpriseIdentity::new(EAP_IDENTITY.trim()).expect("eap identity"),
        EnterpriseUsername::new(EAP_USERNAME.trim()).expect("eap username"),
        EnterprisePassword::new(EAP_PASSWORD.trim()).expect("eap password"),
    );
    let mut config = ClientConfig::new(
        Ssid::try_from(EAP_SSID.trim()).expect("eap ssid"),
        Credential::Wpa2Enterprise(credentials),
    );
    if let Some(channel) =
        parse_u8_literal(EAP_CHANNEL).and_then(|channel| Channel::try_from(channel).ok())
    {
        config = config.with_channel(channel);
    }

    let driver = match driver.configure_sta(config) {
        Ok(driver) => {
            log_line(b"wifi eap configured");
            driver
        }
        Err(error) => {
            log_driver_error(b"wifi eap configure err", error, resources);
            loop_delay();
        }
    };

    let mut driver = match driver.start() {
        Ok(driver) => {
            log_line(b"wifi eap started");
            driver
        }
        Err(error) => {
            log_driver_error(b"wifi eap start err", error, resources);
            loop_delay();
        }
    };

    {
        log_line(b"wifi eap connecting");
        let mut sta = driver.sta().expect("sta");
        log_eap_workspace(b"wifi eap workspace before connect");
        if let Err(error) = sta.connect() {
            log_driver_error(
                b"wifi eap connect err",
                driver_error_from_connect(error),
                resources,
            );
            if EAP_EXPECT_FAILURE {
                log_line(b"eap-negative-ok");
            }
            loop_delay();
        }

        let mut associated = false;
        for poll in 0..120 {
            if resources.link_state() == LinkState::Up {
                associated = true;
                break;
            }

            if poll % 10 == 0 {
                log_eap_workspace(b"wifi eap workspace progress");
                log_heap(b"wifi eap heap progress");
            }

            esp8266_hal::wifi::delay_ticks(100);
        }

        if !associated {
            let _ = sta.disconnect();
            log_eap_workspace(b"wifi eap workspace timeout");
            log_driver_error(
                b"wifi eap connect err",
                esp8266_hal::wifi::DriverError::Timeout,
                resources,
            );
            if EAP_EXPECT_FAILURE {
                log_line(b"eap-negative-ok");
            }
            loop_delay();
        }
    }

    if EAP_EXPECT_FAILURE {
        log_line(b"eap-negative-unexpected-success");
        loop_delay();
    }

    log_line(b"wifi eap associated");
    let link = driver.sta_link().expect("sta link");
    let _mac = link.mac_address().expect("mac");
    let _state = link.link_state();
    log_line(b"eap-ok");

    loop_delay();
}

fn log_line(message: &[u8]) {
    esp8266_hal::logger::write_bytes(message);
    esp8266_hal::logger::write_newline();
}

fn log_driver_error(
    message: &[u8],
    error: esp8266_hal::wifi::DriverError,
    resources: &EapResources,
) {
    esp8266_hal::logger::write_bytes(b"wifi eap error detail error=");
    esp8266_hal::logger::write_bytes(driver_error_name(&error));
    if let esp8266_hal::wifi::DriverError::BackendError(code) = error {
        esp8266_hal::logger::write_bytes(b" code=");
        write_i32(code);
    }
    esp8266_hal::logger::write_bytes(b" disconnect=");
    esp8266_hal::logger::write_dec_u32(resources.last_disconnect_reason().unwrap_or(0) as u32);
    if let Some(failure) = esp8266_hal::wifi::last_alloc_failure() {
        esp8266_hal::logger::write_bytes(b" alloc_kind=");
        esp8266_hal::logger::write_byte(failure.kind);
        esp8266_hal::logger::write_bytes(b" alloc_size=0x");
        esp8266_hal::logger::write_hex_u32(failure.requested_bytes as u32);
        esp8266_hal::logger::write_bytes(b" alloc_free=0x");
        esp8266_hal::logger::write_hex_u32(failure.free_bytes as u32);
        esp8266_hal::logger::write_bytes(b" alloc_task=");
        esp8266_hal::logger::write_byte(failure.task_marker);
        esp8266_hal::logger::write_bytes(b" alloc_count=");
        esp8266_hal::logger::write_dec_u32(failure.count);
    }
    esp8266_hal::logger::write_newline();
    esp8266_hal::logger::write_bytes(message);
    esp8266_hal::logger::write_newline();
}

fn log_init_error(message: &[u8], error: esp8266_hal::wifi::InitError) {
    esp8266_hal::logger::write_bytes(message);
    esp8266_hal::logger::write_bytes(b" error=");
    esp8266_hal::logger::write_bytes(match error {
        esp8266_hal::wifi::InitError::AlreadyInitialized => b"already-initialized",
        esp8266_hal::wifi::InitError::BackendUnavailable => b"backend-unavailable",
    });
    esp8266_hal::logger::write_newline();
}

fn log_heap(message: &[u8]) {
    esp8266_hal::logger::write_bytes(message);
    esp8266_hal::logger::write_bytes(b" heap=");
    esp8266_hal::logger::write_dec_u32(esp8266_hal::wifi::free_heap_bytes());
    esp8266_hal::logger::write_newline();
}

fn log_eap_workspace(message: &[u8]) {
    let debug = esp8266_hal::wifi::eap_rsa_workspace_debug();
    esp8266_hal::logger::write_bytes(message);
    esp8266_hal::logger::write_bytes(b" reserved=");
    esp8266_hal::logger::write_dec_u32(debug.reserved_now as u32);
    esp8266_hal::logger::write_bytes(b" attempts=");
    esp8266_hal::logger::write_dec_u32(debug.reserve_attempts);
    esp8266_hal::logger::write_bytes(b" made=");
    esp8266_hal::logger::write_dec_u32(debug.reserved);
    esp8266_hal::logger::write_bytes(b" used=");
    esp8266_hal::logger::write_dec_u32(debug.used);
    esp8266_hal::logger::write_bytes(b" released=");
    esp8266_hal::logger::write_dec_u32(debug.released);
    esp8266_hal::logger::write_newline();
}

fn driver_error_name(error: &esp8266_hal::wifi::DriverError) -> &'static [u8] {
    match error {
        &esp8266_hal::wifi::DriverError::Timeout => b"timeout",
        &esp8266_hal::wifi::DriverError::StaStartTimeout => b"sta-start-timeout",
        &esp8266_hal::wifi::DriverError::Disconnected => b"disconnected",
        &esp8266_hal::wifi::DriverError::EventOverflow => b"event-overflow",
        &esp8266_hal::wifi::DriverError::PacketOverflow => b"packet-overflow",
        &esp8266_hal::wifi::DriverError::PeerOverflow => b"peer-overflow",
        &esp8266_hal::wifi::DriverError::PayloadTooLong => b"payload-too-long",
        &esp8266_hal::wifi::DriverError::OutOfMemory => b"out-of-memory",
        &esp8266_hal::wifi::DriverError::NotStarted => b"not-started",
        &esp8266_hal::wifi::DriverError::WouldBlock => b"would-block",
        &esp8266_hal::wifi::DriverError::BackendRejected => b"backend-rejected",
        &esp8266_hal::wifi::DriverError::BackendError(_) => b"backend-error",
    }
}

fn driver_error_from_connect(
    error: esp8266_hal::wifi::ConnectError,
) -> esp8266_hal::wifi::DriverError {
    match error {
        esp8266_hal::wifi::ConnectError::Timeout => esp8266_hal::wifi::DriverError::Timeout,
        esp8266_hal::wifi::ConnectError::Disconnected => {
            esp8266_hal::wifi::DriverError::Disconnected
        }
        esp8266_hal::wifi::ConnectError::BackendRejected => {
            esp8266_hal::wifi::DriverError::BackendRejected
        }
    }
}

fn write_i32(value: i32) {
    if value < 0 {
        esp8266_hal::logger::write_byte(b'-');
        esp8266_hal::logger::write_dec_u32(value.unsigned_abs());
    } else {
        esp8266_hal::logger::write_dec_u32(value as u32);
    }
}

fn parse_u8_literal(value: &str) -> Option<u8> {
    let mut result = 0u8;
    for byte in value.trim().bytes() {
        if !byte.is_ascii_digit() {
            return None;
        }
        result = result.checked_mul(10)?.checked_add(byte - b'0')?;
    }
    Some(result)
}

fn loop_delay() -> ! {
    loop {
        esp8266_hal::wifi::delay_ticks(100);
    }
}
