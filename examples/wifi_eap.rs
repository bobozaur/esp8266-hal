#![no_std]
#![no_main]

use panic_halt as _;

mod support;
use esp8266_hal::timer::TimerExt;
use support::log_line;

use esp8266_hal::wifi::{
    Channel, ClientConfig, Configuration, Controller, EnterpriseIdentity, EnterprisePassword,
    EnterpriseUsername, InitConfig, Ssid, StationCredential, Wifi, Wpa2EnterpriseClientCertificate,
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
const EAP_DISABLE_TIME_CHECK: Option<&str> = option_env!("ESP8266_HAL_EAP_DISABLE_TIME_CHECK");

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

static WIFI: StaticCell<Wifi> = StaticCell::new();

#[esp8266_hal::main]
fn main() -> ! {
    let peripherals = esp8266_hal::target::Peripherals::take().expect("peripherals");
    let (_, timer2) = peripherals.TIMER.timers();
    let runtime = support::init(timer2);
    log_line(b"wifi_eap start");

    let mut init = InitConfig::new(14, 4, 4).expect("init config");
    init.queues.station_rx = 0;
    let wifi = WIFI.init(Wifi::new(runtime, init).expect("wifi init"));
    let (mut driver, interfaces) = wifi.split();
    esp8266_hal::logger::write_bytes(b"wifi eap station mac=");
    for (index, byte) in interfaces.station.mac_address().into_iter().enumerate() {
        if index != 0 {
            esp8266_hal::logger::write_byte(b':');
        }
        esp8266_hal::logger::write_hex_u8(byte);
    }
    esp8266_hal::logger::write_newline();
    log_line(b"wifi eap initialized");

    let mut credentials = Wpa2EnterpriseCredentials::new(
        EnterpriseIdentity::new(EAP_IDENTITY).expect("eap identity"),
        EnterpriseUsername::new(EAP_USERNAME).expect("eap username"),
        EnterprisePassword::new(EAP_PASSWORD).expect("eap password"),
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
        Ssid::try_from(EAP_SSID).expect("eap ssid"),
        StationCredential::enterprise(&credentials),
    );
    if let Some(channel) =
        parse_u8_literal(EAP_CHANNEL).and_then(|channel| Channel::try_from(channel).ok())
    {
        config = config.with_channel(channel);
    }

    if let Err(error) = driver.set_configuration(&Configuration::Station(config)) {
        log_driver_error(b"wifi eap configure err", error, &driver);
        loop_delay();
    }
    log_line(b"wifi eap configured");
    if let Err(error) = driver.start_blocking(1_000) {
        log_driver_error(b"wifi eap start err", error, &driver);
        loop_delay();
    }
    log_line(b"wifi eap started");
    log_line(b"wifi eap connecting");
    if let Err(error) = driver.connect_blocking(60_000) {
        // The SDK reports disconnection, not whether an inner EAP password caused it.
        let marker: &[u8] = if EAP_EXPECT_FAILURE
            && matches!(&error, esp8266_hal::wifi::DriverError::Disconnected { .. })
        {
            b"eap-negative-disconnected"
        } else {
            b"wifi eap connect err"
        };
        log_driver_error(marker, error, &driver);
        loop_delay();
    }

    if EAP_EXPECT_FAILURE {
        log_line(b"eap-negative-unexpected-success");
        loop_delay();
    }

    log_line(b"wifi eap associated");
    let link = interfaces.station;
    let _state = link.link_state();
    log_line(b"eap-ok");

    loop_delay();
}

fn log_driver_error(
    message: &[u8],
    error: esp8266_hal::wifi::DriverError,
    driver: &Controller<'_>,
) {
    esp8266_hal::logger::write_bytes(b"wifi eap error detail error=");
    support::log_debug(&error);
    esp8266_hal::logger::write_bytes(b" disconnect=");
    esp8266_hal::logger::write_dec_u32(driver.last_disconnect_reason().unwrap_or(0) as u32);
    esp8266_hal::logger::write_newline();
    log_line(message);
}

fn parse_u8_literal(value: &str) -> Option<u8> {
    let mut result = 0u8;
    for byte in value.trim_ascii().bytes() {
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
    match value.map(str::trim_ascii) {
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
