use std::cell::RefCell;
use std::rc::Rc;

use esp8266_hal::wifi_core::{
    AccessPointRecord, AuthenticationMethod, Channel, DriverError, InitConfig, RawMode,
    ScanOptions, ScanTimings, WifiApBackend, WifiDriver, WifiFeatureBackend, WifiLifecycleBackend,
    WifiPacketBackend, WifiResources, WifiRuntimeBackend, WifiScanBackend, WifiStaBackend,
};

#[derive(Clone, Default)]
struct ScanBackend {
    requested: Rc<RefCell<Option<ScanOptions>>>,
}

#[test]
fn scan_options_preserve_active_timing_bounds() {
    let options = ScanOptions::all().timings(ScanTimings::active(200, 500));

    assert_eq!(options.scan_timings().active_min_ms(), 200);
    assert_eq!(options.scan_timings().active_max_ms(), 500);
}

impl WifiRuntimeBackend for ScanBackend {
    fn now_ms(&self) -> u64 {
        0
    }

    fn spin_once(&mut self) {}
}

impl WifiLifecycleBackend for ScanBackend {
    fn initialize(
        &mut self,
        _config: &InitConfig,
    ) -> Result<(), esp8266_hal::wifi_core::InitError> {
        Ok(())
    }

    fn set_mode(&mut self, _mode: RawMode) -> Result<(), DriverError> {
        Ok(())
    }

    fn start(&mut self) -> Result<(), DriverError> {
        Ok(())
    }

    fn stop(&mut self) -> Result<(), DriverError> {
        Ok(())
    }
}

impl WifiScanBackend for ScanBackend {
    fn scan_into<const N: usize>(
        &mut self,
        options: &ScanOptions,
        records: &mut heapless::Vec<AccessPointRecord, N>,
    ) -> Result<(), DriverError> {
        *self.requested.borrow_mut() = Some(options.clone());
        records.clear();
        records
            .push(
                AccessPointRecord::new(
                    b"core-net",
                    [1, 2, 3, 4, 5, 6],
                    Channel::try_from(6u8).unwrap(),
                    -42,
                    AuthenticationMethod::Wpa2Personal,
                    true,
                )
                .unwrap(),
            )
            .map_err(|_| DriverError::PacketOverflow)?;
        Ok(())
    }
}

impl WifiStaBackend for ScanBackend {}
impl WifiApBackend for ScanBackend {}
impl WifiPacketBackend for ScanBackend {}
impl WifiFeatureBackend for ScanBackend {}

#[test]
fn sta_started_driver_scans_through_typed_backend() {
    let resources = WifiResources::<4, 1>::new();
    let requested = Rc::new(RefCell::new(None));
    let backend = ScanBackend {
        requested: requested.clone(),
    };
    let mut driver = WifiDriver::with_backend(&resources, backend)
        .initialize(InitConfig::default())
        .unwrap()
        .enable_sta()
        .unwrap()
        .start()
        .unwrap();

    let options = ScanOptions::all()
        .channel(Channel::try_from(6u8).unwrap())
        .show_hidden(true);
    let records = driver.scan_blocking::<4>(&options).unwrap();

    assert_eq!(records[0].ssid(), b"core-net");
    assert_eq!(records[0].channel().get(), 6);
    assert_eq!(
        records[0].authentication(),
        AuthenticationMethod::Wpa2Personal
    );
    assert_eq!(requested.borrow().as_ref(), Some(&options));
}

#[test]
fn sta_started_driver_scans_into_caller_storage() {
    let resources = WifiResources::<4, 1>::new();
    let requested = Rc::new(RefCell::new(None));
    let backend = ScanBackend {
        requested: requested.clone(),
    };
    let mut driver = WifiDriver::with_backend(&resources, backend)
        .initialize(InitConfig::default())
        .unwrap()
        .enable_sta()
        .unwrap()
        .start()
        .unwrap();

    let options = ScanOptions::all();
    let mut records = heapless::Vec::<AccessPointRecord, 4>::new();
    driver.scan_blocking_into(&options, &mut records).unwrap();

    assert_eq!(records[0].ssid(), b"core-net");
    assert_eq!(requested.borrow().as_ref(), Some(&options));
}
