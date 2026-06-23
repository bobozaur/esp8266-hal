#![no_std]
#![no_main]

use panic_halt as _;

use esp8266_hal::wifi::{
    Channel, ClientConfig, Credential, EnterpriseIdentity, EnterprisePassword, EnterpriseUsername,
    Esp8266TargetBackend, InitConfig, LinkState, Ssid, WifiDriver, WifiResources,
    Wpa2EnterpriseClientCertificate, Wpa2EnterpriseCredentials,
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
const EAP_DISABLE_TIME_CHECK: Option<&str> = option_env!("ESP8266_HAL_EAP_DISABLE_TIME_CHECK");

type EapResources = WifiResources<0, 0, 1>;

#[cfg(esp8266_hal_eap_ca_cert_pem_nul_file)]
const EAP_CA_CERT_PEM_NUL: Option<&[u8]> =
    Some(include_bytes!(env!("ESP8266_HAL_EAP_CA_CERT_PEM_NUL_FILE")));
#[cfg(not(esp8266_hal_eap_ca_cert_pem_nul_file))]
const EAP_CA_CERT_PEM_NUL: Option<&[u8]> = None;

#[cfg(esp8266_hal_eap_client_cert_pem_nul_file)]
const EAP_CLIENT_CERT_PEM_NUL: Option<&[u8]> = Some(include_bytes!(env!(
    "ESP8266_HAL_EAP_CLIENT_CERT_PEM_NUL_FILE"
)));
#[cfg(not(esp8266_hal_eap_client_cert_pem_nul_file))]
const EAP_CLIENT_CERT_PEM_NUL: Option<&[u8]> = None;

#[cfg(esp8266_hal_eap_private_key_pem_nul_file)]
const EAP_PRIVATE_KEY_PEM_NUL: Option<&[u8]> = Some(include_bytes!(env!(
    "ESP8266_HAL_EAP_PRIVATE_KEY_PEM_NUL_FILE"
)));
#[cfg(not(esp8266_hal_eap_private_key_pem_nul_file))]
const EAP_PRIVATE_KEY_PEM_NUL: Option<&[u8]> = None;

#[cfg(esp8266_hal_eap_private_key_password_nul_file)]
const EAP_PRIVATE_KEY_PASSWORD_NUL: Option<&[u8]> = Some(include_bytes!(env!(
    "ESP8266_HAL_EAP_PRIVATE_KEY_PASSWORD_NUL_FILE"
)));
#[cfg(not(esp8266_hal_eap_private_key_password_nul_file))]
const EAP_PRIVATE_KEY_PASSWORD_NUL: Option<&[u8]> = None;

#[cfg(esp8266_hal_eap_ca_cert_der_file)]
const EAP_CA_CERT_DER: Option<&[u8]> =
    Some(include_bytes!(env!("ESP8266_HAL_EAP_CA_CERT_DER_FILE")));
#[cfg(not(esp8266_hal_eap_ca_cert_der_file))]
const EAP_CA_CERT_DER: Option<&[u8]> = None;

#[cfg(esp8266_hal_eap_client_cert_der_file)]
const EAP_CLIENT_CERT_DER: Option<&[u8]> =
    Some(include_bytes!(env!("ESP8266_HAL_EAP_CLIENT_CERT_DER_FILE")));
#[cfg(not(esp8266_hal_eap_client_cert_der_file))]
const EAP_CLIENT_CERT_DER: Option<&[u8]> = None;

#[cfg(esp8266_hal_eap_private_key_der_file)]
const EAP_PRIVATE_KEY_DER: Option<&[u8]> =
    Some(include_bytes!(env!("ESP8266_HAL_EAP_PRIVATE_KEY_DER_FILE")));
#[cfg(not(esp8266_hal_eap_private_key_der_file))]
const EAP_PRIVATE_KEY_DER: Option<&[u8]> = None;

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

    let mut credentials = Wpa2EnterpriseCredentials::new(
        EnterpriseIdentity::new(EAP_IDENTITY.trim()).expect("eap identity"),
        EnterpriseUsername::new(EAP_USERNAME.trim()).expect("eap username"),
        EnterprisePassword::new(EAP_PASSWORD.trim()).expect("eap password"),
    )
    .with_disable_time_check(env_bool(EAP_DISABLE_TIME_CHECK, true));
    if let Some(ca_cert) = optional_der_bytes(EAP_CA_CERT_DER, b"wifi eap ca cert der invalid") {
        credentials = credentials.with_ca_cert_der(ca_cert);
        log_line(b"wifi eap ca cert der configured");
    } else if let Some(ca_cert) =
        optional_nul_terminated_bytes(EAP_CA_CERT_PEM_NUL, b"wifi eap ca cert missing nul")
    {
        credentials = credentials.with_ca_cert(ca_cert);
        log_line(b"wifi eap ca cert pem configured");
    }
    match client_certificate_from_embedded_material() {
        Some(certificate) => {
            credentials = credentials.with_client_certificate(certificate);
            log_line(b"wifi eap client cert configured");
        }
        None if no_client_certificate_material() => {}
        None => {
            log_line(b"wifi eap client cert incomplete");
            loop_delay();
        }
    }
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
    let first_failure = esp8266_hal::wifi::first_alloc_failure();
    let last_failure = esp8266_hal::wifi::last_alloc_failure();
    if let Some(failure) = first_failure {
        log_alloc_failure(b" alloc_first", failure);
    }
    if let Some(failure) = last_failure {
        if first_failure != Some(failure) {
            log_alloc_failure(b" alloc_last", failure);
        }
    }
    esp8266_hal::logger::write_newline();
    esp8266_hal::logger::write_bytes(message);
    esp8266_hal::logger::write_newline();
}

fn log_alloc_failure(prefix: &[u8], failure: esp8266_hal::wifi::AllocFailureDebug) {
    esp8266_hal::logger::write_bytes(prefix);
    esp8266_hal::logger::write_bytes(b"_kind=");
    esp8266_hal::logger::write_byte(failure.kind);
    esp8266_hal::logger::write_bytes(prefix);
    esp8266_hal::logger::write_bytes(b"_size=0x");
    esp8266_hal::logger::write_hex_u32(failure.requested_bytes as u32);
    esp8266_hal::logger::write_bytes(prefix);
    esp8266_hal::logger::write_bytes(b"_free=0x");
    esp8266_hal::logger::write_hex_u32(failure.free_bytes as u32);
    esp8266_hal::logger::write_bytes(prefix);
    esp8266_hal::logger::write_bytes(b"_task=");
    esp8266_hal::logger::write_byte(failure.task_marker);
    esp8266_hal::logger::write_bytes(prefix);
    esp8266_hal::logger::write_bytes(b"_count=");
    esp8266_hal::logger::write_dec_u32(failure.count);
    esp8266_hal::logger::write_bytes(prefix);
    esp8266_hal::logger::write_bytes(b"_eap_active=");
    esp8266_hal::logger::write_dec_u32(failure.eap_active_count);
    esp8266_hal::logger::write_bytes(prefix);
    esp8266_hal::logger::write_bytes(b"_eap_reserved=");
    esp8266_hal::logger::write_dec_u32(failure.eap_reserved as u32);
    esp8266_hal::logger::write_bytes(prefix);
    esp8266_hal::logger::write_bytes(b"_eap_usable=");
    esp8266_hal::logger::write_dec_u32(failure.eap_usable_bytes as u32);
    esp8266_hal::logger::write_bytes(prefix);
    esp8266_hal::logger::write_bytes(b"_eap_in_use=");
    esp8266_hal::logger::write_dec_u32(failure.eap_in_use as u32);
    esp8266_hal::logger::write_bytes(prefix);
    esp8266_hal::logger::write_bytes(b"_eap_avail=");
    esp8266_hal::logger::write_dec_u32(failure.eap_reserve_available as u32);
    esp8266_hal::logger::write_bytes(prefix);
    esp8266_hal::logger::write_bytes(b"_eap_count=");
    esp8266_hal::logger::write_dec_u32(failure.eap_reserved_count as u32);
    esp8266_hal::logger::write_bytes(prefix);
    esp8266_hal::logger::write_bytes(b"_eap_usage=0x");
    esp8266_hal::logger::write_hex_u32(failure.eap_usage_bits);
    esp8266_hal::logger::write_bytes(prefix);
    esp8266_hal::logger::write_bytes(b"_realloc_old=0x");
    esp8266_hal::logger::write_hex_u32(failure.realloc_old_usable as u32);
    esp8266_hal::logger::write_bytes(prefix);
    esp8266_hal::logger::write_bytes(b"_realloc_workspace=0x");
    esp8266_hal::logger::write_hex_u32(failure.realloc_workspace_usable as u32);
    esp8266_hal::logger::write_bytes(prefix);
    esp8266_hal::logger::write_bytes(b"_ctx=0x");
    esp8266_hal::logger::write_hex_u32(failure.context);
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
    esp8266_hal::logger::write_bytes(b" active=");
    esp8266_hal::logger::write_dec_u32(debug.active_count);
    esp8266_hal::logger::write_bytes(b" attempts=");
    esp8266_hal::logger::write_dec_u32(debug.reserve_attempts);
    esp8266_hal::logger::write_bytes(b" made=");
    esp8266_hal::logger::write_dec_u32(debug.reserved);
    esp8266_hal::logger::write_bytes(b" used=");
    esp8266_hal::logger::write_dec_u32(debug.used);
    esp8266_hal::logger::write_bytes(b" released=");
    esp8266_hal::logger::write_dec_u32(debug.released);
    esp8266_hal::logger::write_bytes(b" usable=");
    esp8266_hal::logger::write_dec_u32(debug.usable_bytes as u32);
    esp8266_hal::logger::write_bytes(b" in_use=");
    esp8266_hal::logger::write_dec_u32(debug.in_use as u32);
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

fn optional_nul_terminated_bytes(
    value: Option<&'static [u8]>,
    missing_nul_message: &[u8],
) -> Option<&'static [u8]> {
    let bytes = value?;
    if bytes.is_empty() {
        return None;
    }
    if bytes.last() != Some(&0) {
        log_line(missing_nul_message);
        return None;
    }
    Some(bytes)
}

fn optional_der_bytes(
    value: Option<&'static [u8]>,
    invalid_message: &[u8],
) -> Option<&'static [u8]> {
    let bytes = value?;
    if bytes.first() != Some(&0x30) {
        log_line(invalid_message);
        return None;
    }
    Some(bytes)
}

fn client_certificate_from_embedded_material() -> Option<Wpa2EnterpriseClientCertificate> {
    match (
        optional_der_bytes(EAP_CLIENT_CERT_DER, b"wifi eap client cert der invalid"),
        optional_der_bytes(EAP_PRIVATE_KEY_DER, b"wifi eap private key der invalid"),
    ) {
        (Some(client_cert), Some(private_key)) => {
            return Some(Wpa2EnterpriseClientCertificate::new_der(
                client_cert,
                private_key,
            ));
        }
        (None, None) => {}
        _ => return None,
    }

    match (
        optional_nul_terminated_bytes(EAP_CLIENT_CERT_PEM_NUL, b"wifi eap client cert missing nul"),
        optional_nul_terminated_bytes(EAP_PRIVATE_KEY_PEM_NUL, b"wifi eap private key missing nul"),
    ) {
        (Some(client_cert), Some(private_key)) => {
            let mut certificate = Wpa2EnterpriseClientCertificate::new(client_cert, private_key);
            if let Some(password) = optional_nul_terminated_bytes(
                EAP_PRIVATE_KEY_PASSWORD_NUL,
                b"wifi eap private key password missing nul",
            ) {
                certificate = certificate.with_private_key_password(password);
            }
            Some(certificate)
        }
        (None, None) => None,
        _ => None,
    }
}

fn no_client_certificate_material() -> bool {
    EAP_CLIENT_CERT_DER.is_none()
        && EAP_PRIVATE_KEY_DER.is_none()
        && EAP_CLIENT_CERT_PEM_NUL.is_none()
        && EAP_PRIVATE_KEY_PEM_NUL.is_none()
}

fn env_bool(value: Option<&str>, default: bool) -> bool {
    match value.map(str::trim) {
        Some("0") | Some("false") | Some("no") | Some("off") => false,
        Some("1") | Some("true") | Some("yes") | Some("on") => true,
        _ => default,
    }
}

fn loop_delay() -> ! {
    loop {
        esp8266_hal::wifi::delay_ticks(100);
    }
}
