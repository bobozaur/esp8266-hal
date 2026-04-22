#![no_std]
#![no_main]

use panic_halt as _;

use esp8266_hal::wifi::{
    AccessPointRecord, DriverError, Esp8266TargetBackend, InitConfig, InitError, ScanOptions,
    ScanTimings, WifiDriver, WifiResources,
};
use static_cell::StaticCell;

static RESOURCES: StaticCell<WifiResources<8, 4, 2>> = StaticCell::new();
static SCAN_RECORDS: StaticCell<heapless::Vec<AccessPointRecord, 16>> = StaticCell::new();

#[inline(always)]
fn log_line(message: &[u8]) {
    esp8266_hal::logger::write_bytes(message);
    esp8266_hal::logger::write_newline();
}

#[esp8266_hal::main]
fn main() -> ! {
    log_line(b"wifi_core_scan start");

    let resources = RESOURCES.init(WifiResources::new());
    let backend = Esp8266TargetBackend::new(resources);
    let driver =
        match WifiDriver::with_backend(resources, backend).initialize(InitConfig::default()) {
            Ok(driver) => driver,
            Err(error) => {
                esp8266_hal::logger::write_bytes(b"wifi core scan init error ");
                esp8266_hal::logger::write_bytes(init_error_name(error).as_bytes());
                esp8266_hal::logger::write_newline();
                loop {
                    esp8266_hal::wifi::delay_ticks(100);
                }
            }
        };
    log_line(b"wifi core scan initialized");

    let mut driver = driver
        .enable_sta()
        .expect("configure")
        .start()
        .expect("start");
    log_line(b"wifi core scan started");

    let options = ScanOptions::all()
        .show_hidden(true)
        .timings(ScanTimings::active(200, 500));

    let records = SCAN_RECORDS.init(heapless::Vec::new());
    match driver.scan_blocking_into(&options, records) {
        Ok(()) => {
            esp8266_hal::logger::write_bytes(b"wifi core scan done count=");
            esp8266_hal::logger::write_dec_u32(records.len() as u32);
            esp8266_hal::logger::write_newline();
            for record in records.iter() {
                esp8266_hal::logger::write_bytes(b"ap ssid=");
                esp8266_hal::logger::write_bytes(record.ssid());
                esp8266_hal::logger::write_bytes(b" bssid=");
                for (index, byte) in record.bssid().iter().enumerate() {
                    if index != 0 {
                        esp8266_hal::logger::write_byte(b':');
                    }
                    esp8266_hal::logger::write_hex_u8(*byte);
                }
                esp8266_hal::logger::write_bytes(b" ch=");
                esp8266_hal::logger::write_dec_u32(record.channel().get() as u32);
                esp8266_hal::logger::write_bytes(b" rssi=");
                if record.rssi() < 0 {
                    esp8266_hal::logger::write_byte(b'-');
                    esp8266_hal::logger::write_dec_u32(record.rssi().unsigned_abs() as u32);
                } else {
                    esp8266_hal::logger::write_dec_u32(record.rssi() as u32);
                }
                esp8266_hal::logger::write_bytes(b" auth=");
                esp8266_hal::logger::write_dec_u32(record.auth_mode() as u32);
                esp8266_hal::logger::write_bytes(b" wps=");
                esp8266_hal::logger::write_dec_u32(record.wps() as u32);
                esp8266_hal::logger::write_newline();
            }
            log_line(b"scan-ok");
        }
        Err(error) => {
            esp8266_hal::logger::write_bytes(b"wifi core scan error ");
            esp8266_hal::logger::write_bytes(driver_error_name(error).as_bytes());
            esp8266_hal::logger::write_newline();
        }
    }

    loop {
        esp8266_hal::wifi::delay_ticks(100);
    }
}

fn init_error_name(error: InitError) -> &'static str {
    match error {
        InitError::AlreadyInitialized => "already-initialized",
        InitError::BackendUnavailable => "backend-unavailable",
    }
}

fn driver_error_name(error: DriverError) -> &'static str {
    match error {
        DriverError::Timeout => "timeout",
        DriverError::StaStartTimeout => "sta-start-timeout",
        DriverError::Disconnected => "disconnected",
        DriverError::EventOverflow => "event-overflow",
        DriverError::PacketOverflow => "packet-overflow",
        DriverError::PeerOverflow => "peer-overflow",
        DriverError::PayloadTooLong => "payload-too-long",
        DriverError::OutOfMemory => "out-of-memory",
        DriverError::NotStarted => "not-started",
        DriverError::WouldBlock => "would-block",
        DriverError::BackendRejected => "backend-rejected",
        DriverError::BackendError(_) => "backend-error",
    }
}
