//! Associate using WPA2 Enterprise PEAP or EAP-TLS.
//! Set the identity and PEAP username/password below. For EAP-TLS, set
//! CLIENT_CERTIFICATE to Some(Wpa2EnterpriseClientCertificate::new_der(
//! include_bytes!("client.der"), include_bytes!("key.der"))). The client key
//! may be RSA or P256. Set CA_CERTIFICATE_DER to Some(include_bytes!("ca.der"))
//! to validate the server's certificate chain. Paths are relative to this file.
//!
//! The PEM constructor is also available: Wpa2EnterpriseClientCertificate::new
//! takes PEM certificate/key buffers with a trailing NUL, and
//! with_private_key_password accepts a NUL-terminated password for encrypted keys.

#![no_std]
#![no_main]
#![feature(asm_experimental_arch)]

// Leave DRAM for certificate chains and the separate EAP worker stack.
core::arch::global_asm!(".global STACK_SIZE", ".set STACK_SIZE, 4096");

mod support;

use core::fmt::Write;
use esp8266_hal::timer::TimerExt;
use esp8266_hal::wifi::{
    ClientConfig, Configuration, EnterpriseIdentity, EnterprisePassword, EnterpriseUsername,
    InitConfig, Ssid, StationCredential, Wifi, Wpa2EnterpriseClientCertificate,
    Wpa2EnterpriseCredentials,
};
use panic_halt as _;

const SSID: &str = "your-enterprise-ssid";
const IDENTITY: &str = "your-identity";
const USERNAME: &str = "your-username";
const PASSWORD: &str = "your-password";
const CA_CERTIFICATE_DER: Option<&[u8]> = None;
const CLIENT_CERTIFICATE: Option<Wpa2EnterpriseClientCertificate> = None;

#[esp8266_hal::entry]
fn main() -> ! {
    let peripherals = esp8266_hal::target::Peripherals::take().unwrap();
    let mut serial = support::serial(peripherals.UART0, peripherals.GPIO);
    let (_, timer2) = peripherals.TIMER.timers();
    let runtime = support::init(timer2);
    let mut init = InitConfig::new(14, 4, 4).unwrap();
    init.queues.station_rx = 0;
    let mut wifi = Wifi::new(runtime, init).expect("Wi-Fi initialization");
    let (mut controller, _) = wifi.split();

    let mut credentials = Wpa2EnterpriseCredentials::new(
        EnterpriseIdentity::new(IDENTITY).expect("identity"),
        EnterpriseUsername::new(USERNAME).expect("username"),
        EnterprisePassword::new(PASSWORD).expect("password"),
    )
    // This example has no wall clock; certificate-chain validation is separate.
    .with_disable_time_check(true);
    if let Some(ca) = CA_CERTIFICATE_DER {
        credentials = credentials.with_ca_cert_der(ca);
    }
    if let Some(certificate) = CLIENT_CERTIFICATE {
        credentials = credentials.with_client_certificate(certificate);
    }
    let config = ClientConfig::new(
        Ssid::try_from(SSID).expect("SSID"),
        StationCredential::enterprise(&credentials),
    );
    controller
        .set_configuration(&Configuration::Station(config))
        .expect("enterprise configuration");
    controller.start_blocking(1_000).expect("start Wi-Fi");
    writeln!(serial, "Connecting to {SSID}\r").unwrap();
    match controller.connect_blocking(60_000) {
        Ok(()) => writeln!(serial, "Enterprise station associated\r").unwrap(),
        Err(error) => writeln!(serial, "Enterprise connection failed: {error:?}\r").unwrap(),
    }
    loop {
        esp8266_hal::wifi::delay_ticks(100);
    }
}
