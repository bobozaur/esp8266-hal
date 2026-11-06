use super::{
    allocator::RuntimeAllocator,
    interface::{Interface, Interfaces},
    resources::{InterfaceId, Resources},
    target::Sdk,
    AccessPointRecord, ApStationInfo, Configuration, Deadline, DriverError, InitConfig, InitError,
    LinkState, RawMode, ScanOptions,
};
use allocator_api2::boxed::Box;
use core::task::Poll;

/// Owns the stable callback storage and the ESP8266 radio instance.
///
/// Keep this owner alive while its controller or packet interfaces are borrowed.
/// Drop attempts native teardown; failed teardown or a forgotten receive token
/// retains the allocation and radio instance so callbacks cannot access freed
/// memory. Runtime infrastructure remains initialized after this object is dropped.
pub struct Wifi {
    resources: Option<Box<Resources, RuntimeAllocator>>,
    sdk: Sdk,
    station_mac: [u8; 6],
    ap_mac: [u8; 6],
}

impl Wifi {
    fn allocate_resources(
        config: &InitConfig,
        allocator: RuntimeAllocator,
    ) -> Result<Box<Resources, RuntimeAllocator>, InitError> {
        Box::try_new_in(Resources::try_new(&config.queues, allocator)?, allocator)
            .map_err(|_| InitError::OutOfMemory)
    }
    #[cfg(target_arch = "xtensa")]
    /// Initializes the radio using an explicitly initialized application runtime.
    ///
    /// Rust queues allocate once according to [`InitConfig::queues`]. The SDK
    /// continues using the runtime's allocator during operation. Construction
    /// does not configure, start, or connect the radio; use [`Self::split`] to
    /// obtain the controller. Native buffer counts remain fixed for the boot as
    /// described by [`InitConfig`].
    pub fn new(
        runtime: &'static crate::wifi::Runtime,
        config: InitConfig,
    ) -> Result<Self, InitError> {
        let resources = Self::allocate_resources(&config, runtime.allocator())?;
        let mut wifi = Self {
            sdk: Sdk::new(&resources),
            resources: Some(resources),
            station_mac: [0; 6],
            ap_mac: [0; 6],
        };
        wifi.sdk.initialize(&config)?;
        wifi.station_mac = wifi.sdk.sta_mac_address().map_err(|e| match e {
            DriverError::Sdk(code) => InitError::Sdk(code),
            _ => InitError::BackendUnavailable,
        })?;
        wifi.ap_mac = wifi.sdk.ap_mac_address().map_err(|e| match e {
            DriverError::Sdk(code) => InitError::Sdk(code),
            _ => InitError::BackendUnavailable,
        })?;
        Ok(wifi)
    }
    /// Borrows independent control, station-frame, and access-point-frame handles.
    /// The configured radio mode determines which packet interfaces can operate.
    pub fn split(&mut self) -> (Controller<'_>, Interfaces<'_>) {
        let resources = self.resources.as_deref().expect("live Wifi owns resources");
        (
            Controller {
                sdk: &mut self.sdk,
                resources,
            },
            Interfaces {
                station: Interface {
                    resources,
                    id: InterfaceId::Station,
                    mac: self.station_mac,
                },
                access_point: Interface {
                    resources,
                    id: InterfaceId::AccessPoint,
                    mac: self.ap_mac,
                },
            },
        )
    }
}
impl Drop for Wifi {
    fn drop(&mut self) {
        let resources = self.resources.as_deref().expect("live Wifi owns resources");
        let safe_to_shutdown = resources.close_admission().is_ok();
        if safe_to_shutdown {
            resources.drain_sta_rx();
            resources.drain_ap_rx();
        }
        if !safe_to_shutdown || !self.sdk.shutdown() {
            // A forgotten packet/session or failed native teardown must never
            // turn late callbacks into accesses to released application memory.
            core::mem::forget(self.resources.take());
        }
    }
}

/// Radio configuration and operation control, independent of packet interfaces.
///
/// Blocking timeouts are milliseconds; async waits have no internal deadline and
/// work with the application's executor. A timeout or dropped future stops
/// waiting and may leave SDK work active. Request disconnection, cancel a scan,
/// or stop the radio explicitly before starting conflicting work. Reconnection
/// policy belongs to the application.
pub struct Controller<'a> {
    pub(crate) sdk: &'a mut Sdk,
    pub(crate) resources: &'a Resources,
}
impl Controller<'_> {
    fn require_stopped(&self) -> Result<(), DriverError> {
        if self.sdk.started || self.sdk.session_claimed {
            Err(DriverError::Busy)
        } else {
            Ok(())
        }
    }
    /// Replaces the station/AP configuration while the radio is stopped and no session is held.
    pub fn set_configuration(&mut self, config: &Configuration<'_>) -> Result<(), DriverError> {
        self.require_stopped()?;
        self.resources.state.with(|s| s.station_configured = false);
        let result = match config {
            Configuration::Station(station) => self.sdk.configure_sta(station),
            Configuration::AccessPoint(ap) => self.sdk.configure_ap(ap),
            Configuration::Mixed(mixed) => self
                .sdk
                .configure_mixed(mixed.client(), mixed.access_point()),
        };
        if result.is_ok() {
            self.resources
                .state
                .with(|s| s.station_configured = !matches!(config, Configuration::AccessPoint(_)));
        } else {
            self.sdk.mode = None;
        }
        result
    }
    /// Selects credential-free station mode for scanning and provisioning.
    pub fn enable_station(&mut self) -> Result<(), DriverError> {
        self.require_stopped()?;
        self.resources.state.with(|s| s.station_configured = false);
        self.sdk.set_mode(RawMode::Sta)
    }
    fn begin_start(&mut self) -> Result<(), DriverError> {
        if self.sdk.started || self.sdk.mode.is_none() {
            return Err(DriverError::InvalidState);
        }
        self.resources.state.with(|s| {
            s.stopping = false;
            s.sta_started = false;
            s.ap_started = false;
        });
        self.sdk.start()?;
        let result = (|| {
            if self.sdk.mode.is_some_and(RawMode::has_station)
                && self.resources.station_rx_capacity > 0
            {
                self.sdk.attach_sta_rx()?;
            }
            if self.sdk.mode.is_some_and(RawMode::has_ap) && self.resources.ap_rx_capacity > 0 {
                self.sdk.attach_ap_rx()?;
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = self.sdk.stop();
        }
        result
    }
    fn started(&self) -> bool {
        self.resources.state.with(|s| match self.sdk.mode {
            Some(RawMode::Sta) => s.sta_started,
            Some(RawMode::Ap) => s.ap_started,
            Some(RawMode::ApSta) => s.sta_started && s.ap_started,
            None => false,
        })
    }
    /// Waits for SDK start events. Timeout leaves the radio operation active.
    pub fn start_blocking(&mut self, timeout_ms: u32) -> Result<(), DriverError> {
        self.begin_start()?;
        let deadline = Deadline::new(self.sdk.now_ms(), timeout_ms);
        while !self.started() {
            if deadline.is_expired(self.sdk.now_ms()) {
                return Err(DriverError::Timeout);
            }
            self.sdk.spin_once()
        }
        Ok(())
    }
    pub async fn start_async(&mut self) -> Result<(), DriverError> {
        self.begin_start()?;
        core::future::poll_fn(|cx| {
            self.resources.register_control_waker(cx.waker());
            if self.started() {
                Poll::Ready(Ok(()))
            } else {
                Poll::Pending
            }
        })
        .await
    }
    /// Stops the radio and protocol sessions, discarding queued native RX frames.
    ///
    /// A checked-out native receive token on either interface, or an active TX
    /// submission, returns [`DriverError::Busy`]. Release those before retrying.
    /// Configuration remains available for a later start. Frames already copied
    /// into a network stack and existing sockets belong to the application's lifecycle.
    pub fn stop(&mut self) -> Result<(), DriverError> {
        self.resources.close_admission()?;
        self.resources.drain_sta_rx();
        self.resources.drain_ap_rx();
        let result = self.sdk.stop();
        if result.is_ok() {
            self.sdk.session_claimed = false;
            #[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps"))]
            self.resources.release_events();
        }
        result
    }
    fn begin_connect(&mut self, provisioning: bool) -> Result<(), DriverError> {
        if self.sdk.session_claimed && !provisioning {
            return Err(DriverError::Busy);
        }
        if !self.sdk.started
            || !self.resources.state.with(|s| s.station_configured)
            || !self.sdk.mode.is_some_and(RawMode::has_station)
        {
            return Err(DriverError::InvalidState);
        }
        self.resources.state.with(|s| {
            if s.connected || s.connecting || s.disconnecting || s.scan_pending {
                return Err(DriverError::Busy);
            }
            s.connecting = true;
            s.connect_result = None;
            s.reason = None;
            Ok(())
        })?;
        if let Err(error) = self.sdk.connect() {
            self.resources.state.with(|s| {
                s.connecting = false;
                s.connect_result = Some(Err(error.clone()));
            });
            return Err(error);
        }
        Ok(())
    }
    fn connect_result(&self) -> Option<Result<(), DriverError>> {
        self.resources.state.with(|s| s.connect_result.clone())
    }
    /// Waits for association or terminal SDK failure. Timeout leaves SDK work active.
    pub fn connect_blocking(&mut self, timeout_ms: u32) -> Result<(), DriverError> {
        self.connect_blocking_impl(timeout_ms, false)
    }
    pub(super) fn connect_blocking_impl(
        &mut self,
        timeout_ms: u32,
        provisioning: bool,
    ) -> Result<(), DriverError> {
        self.begin_connect(provisioning)?;
        let deadline = Deadline::new(self.sdk.now_ms(), timeout_ms);
        loop {
            if let Some(result) = self.connect_result() {
                return result;
            }
            if deadline.is_expired(self.sdk.now_ms()) {
                return Err(DriverError::Timeout);
            }
            self.sdk.spin_once()
        }
    }
    /// Dropping this future stops waiting; explicit disconnect aborts SDK work.
    pub async fn connect_async(&mut self) -> Result<(), DriverError> {
        self.connect_async_impl(false).await
    }
    pub(super) async fn connect_async_impl(
        &mut self,
        provisioning: bool,
    ) -> Result<(), DriverError> {
        self.begin_connect(provisioning)?;
        core::future::poll_fn(|cx| {
            self.resources.register_control_waker(cx.waker());
            match self.connect_result() {
                Some(result) => Poll::Ready(result),
                None => Poll::Pending,
            }
        })
        .await
    }
    /// Requests disconnection; completion is reported by the actual SDK event.
    /// Queued station frames are discarded. A held station receive token returns
    /// [`DriverError::Busy`]; access-point receive tokens remain independent.
    pub fn disconnect(&mut self) -> Result<(), DriverError> {
        let needed = self
            .resources
            .state
            .with(|s| s.connected || s.connecting || s.disconnecting);
        if !needed {
            return Ok(());
        }
        self.resources.close_station()?;
        self.resources.drain_sta_rx();
        self.resources.state.with(|s| s.disconnecting = true);
        if let Err(error) = self.sdk.disconnect() {
            self.resources.state.with(|s| s.disconnecting = false);
            return Err(error);
        }
        Ok(())
    }
    fn disconnected(&self) -> bool {
        self.resources
            .state
            .with(|s| !s.connected && !s.connecting && !s.disconnecting)
    }
    pub fn disconnect_blocking(&mut self, timeout_ms: u32) -> Result<(), DriverError> {
        self.disconnect()?;
        let deadline = Deadline::new(self.sdk.now_ms(), timeout_ms);
        while !self.disconnected() {
            if deadline.is_expired(self.sdk.now_ms()) {
                return Err(DriverError::Timeout);
            }
            self.sdk.spin_once()
        }
        Ok(())
    }
    pub async fn disconnect_async(&mut self) -> Result<(), DriverError> {
        self.disconnect()?;
        core::future::poll_fn(|cx| {
            self.resources.register_control_waker(cx.waker());
            if self.disconnected() {
                Poll::Ready(Ok(()))
            } else {
                Poll::Pending
            }
        })
        .await
    }
    pub fn link_state(&self) -> LinkState {
        self.resources.link_state()
    }
    pub fn last_disconnect_reason(&self) -> Option<u8> {
        self.resources.last_disconnect_reason()
    }
    pub fn connected_stations(
        &mut self,
        stations: &mut [ApStationInfo],
    ) -> Result<usize, DriverError> {
        if !self.sdk.started || !self.sdk.mode.is_some_and(RawMode::has_ap) {
            return Err(DriverError::InvalidState);
        }
        self.sdk.connected_ap_stations(stations)
    }
    pub fn deauth_all(&mut self) -> Result<(), DriverError> {
        if !self.sdk.started || !self.sdk.mode.is_some_and(RawMode::has_ap) {
            return Err(DriverError::InvalidState);
        }
        self.sdk.deauth_all_ap_stations()
    }
    fn begin_scan(&mut self, options: &ScanOptions) -> Result<(), DriverError> {
        if self.sdk.session_claimed {
            return Err(DriverError::Busy);
        }
        if !self.sdk.started || !self.sdk.mode.is_some_and(RawMode::has_station) {
            return Err(DriverError::InvalidState);
        }
        let (connected, connecting, pending, done) = self.resources.state.with(|s| {
            (
                s.connected,
                s.connecting || s.disconnecting,
                s.scan_pending,
                s.scan_done.is_some(),
            )
        });
        if connected || connecting {
            return Err(DriverError::Busy);
        }
        if pending {
            if !done {
                return Err(DriverError::Busy);
            }
            self.sdk.discard_scan();
        }
        self.resources.state.with(|s| {
            s.scan_pending = true;
            s.scan_done = None;
        });
        if let Err(error) = self.sdk.begin_scan(options) {
            self.resources.state.with(|s| s.scan_pending = false);
            return Err(error);
        }
        Ok(())
    }
    fn scan_done(&self) -> Option<Result<(), DriverError>> {
        self.resources.state.with(|s| s.scan_done.clone())
    }
    fn finish_scan(&mut self, records: &mut [AccessPointRecord]) -> Result<usize, DriverError> {
        let result = self.scan_done().ok_or(DriverError::Busy)?;
        let result = match result {
            Ok(()) => self.sdk.collect_scan(records),
            Err(error) => {
                self.sdk.discard_scan();
                Err(error)
            }
        };
        self.resources.state.with(|s| {
            s.scan_pending = false;
            s.scan_done = None;
        });
        result
    }
    /// Scans in started station mode while disconnected and not connecting.
    /// The SDK owns results until this call collects them into `records[..count]`.
    /// Timeout leaves the scan tracked; use [`Self::cancel_scan`] to request cancellation.
    pub fn scan_blocking(
        &mut self,
        options: &ScanOptions,
        records: &mut [AccessPointRecord],
        timeout_ms: u32,
    ) -> Result<usize, DriverError> {
        if records.is_empty() {
            return Err(DriverError::PacketOverflow);
        }
        self.begin_scan(options)?;
        let deadline = Deadline::new(self.sdk.now_ms(), timeout_ms);
        while self.scan_done().is_none() {
            if deadline.is_expired(self.sdk.now_ms()) {
                return Err(DriverError::Timeout);
            }
            self.sdk.spin_once()
        }
        self.finish_scan(records)
    }
    /// Dropping the waiter leaves the scan tracked; cancel explicitly to abort.
    pub async fn scan_async(
        &mut self,
        options: &ScanOptions,
        records: &mut [AccessPointRecord],
    ) -> Result<usize, DriverError> {
        if records.is_empty() {
            return Err(DriverError::PacketOverflow);
        }
        self.begin_scan(options)?;
        core::future::poll_fn(|cx| {
            self.resources.register_control_waker(cx.waker());
            if self.scan_done().is_some() {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
        self.finish_scan(records)
    }
    /// Requests cancellation. Result storage is retained until the SDK's done event.
    pub fn cancel_scan(&mut self) -> Result<(), DriverError> {
        if !self.resources.state.with(|s| s.scan_pending) {
            return Ok(());
        }
        if self.scan_done().is_none() {
            self.sdk.cancel_scan()?;
        }
        Ok(())
    }
    fn finish_cancel_scan(&mut self) {
        if self.resources.state.with(|s| s.scan_pending) {
            self.sdk.discard_scan();
            self.resources.state.with(|s| {
                s.scan_pending = false;
                s.scan_done = None;
            });
        }
    }
    pub fn cancel_scan_blocking(&mut self, timeout_ms: u32) -> Result<(), DriverError> {
        self.cancel_scan()?;
        let deadline = Deadline::new(self.sdk.now_ms(), timeout_ms);
        while self.resources.state.with(|s| s.scan_pending) && self.scan_done().is_none() {
            if deadline.is_expired(self.sdk.now_ms()) {
                return Err(DriverError::Timeout);
            }
            self.sdk.spin_once()
        }
        self.finish_cancel_scan();
        Ok(())
    }
    pub async fn cancel_scan_async(&mut self) -> Result<(), DriverError> {
        self.cancel_scan()?;
        core::future::poll_fn(|cx| {
            self.resources.register_control_waker(cx.waker());
            if !self.resources.state.with(|s| s.scan_pending) || self.scan_done().is_some() {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
        self.finish_cancel_scan();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::super::{
        allocator::test_allocator, ClientConfig, QueueConfig, Ssid, StationCredential, WifiEvent,
    };
    use super::*;
    use core::{
        future::Future,
        task::{Context, Poll},
    };
    use std::{
        boxed::Box as HostBox,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        task::Wake,
    };
    struct CountWake(AtomicUsize);
    impl Wake for CountWake {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
    fn waker() -> core::task::Waker {
        Arc::new(CountWake(AtomicUsize::new(0))).into()
    }
    pub(super) fn station() -> Configuration<'static> {
        Configuration::Station(ClientConfig::new(
            Ssid::try_from("network").unwrap(),
            StationCredential::open(),
        ))
    }
    #[test]
    fn scan_mode_needs_no_ssid_and_cannot_connect() {
        let r = Resources::try_new(&QueueConfig::default(), test_allocator()).unwrap();
        let mut sdk = Sdk::new(&r);
        let mut c = Controller {
            sdk: &mut sdk,
            resources: &r,
        };
        c.enable_station().unwrap();
        c.start_blocking(1000).unwrap();
        assert_eq!(c.begin_connect(false), Err(DriverError::InvalidState));
        assert!(c.begin_scan(&ScanOptions::all()).is_ok());
    }
    #[test]
    fn changing_ssid_after_disconnect_cannot_deliver_old_queued_rx() {
        use super::super::{packet::RawRxRecord, resources::InterfaceId};
        static OLD: [u8; 64] = [0x11; 64];
        static NEW: [u8; 64] = [0x22; 64];
        let r = Resources::try_new(&QueueConfig::default(), test_allocator()).unwrap();
        let mut sdk = Sdk::new(&r);
        let mut c = Controller {
            sdk: &mut sdk,
            resources: &r,
        };
        c.set_configuration(&station()).unwrap();
        c.start_blocking(1000).unwrap();
        r.publish_event(WifiEvent::StaConnected).unwrap();
        unsafe fn release_static(_: *mut core::ffi::c_void, _: *mut core::ffi::c_void) {}
        // SAFETY: immutable static backing remains live; release has no work.
        let record = |bytes: &'static [u8; 64]| unsafe {
            RawRxRecord::from_raw(
                bytes.as_ptr().cast_mut().cast(),
                64,
                core::ptr::null_mut(),
                release_static,
            )
        };
        r.enqueue_rx(InterfaceId::Station, record(&OLD)).unwrap();
        let held = r.take_rx(InterfaceId::Station).unwrap();
        r.enqueue_rx(InterfaceId::Station, record(&OLD)).unwrap();
        r.publish_event(WifiEvent::StaDisconnected { reason: Some(8) })
            .unwrap();
        assert_eq!(held.as_slice(), OLD);
        assert_eq!(c.stop(), Err(DriverError::Busy));
        drop(held);
        c.stop().unwrap();
        let replacement = Configuration::Station(ClientConfig::new(
            Ssid::try_from("replacement-ssid").unwrap(),
            StationCredential::open(),
        ));
        c.set_configuration(&replacement).unwrap();
        c.start_blocking(1000).unwrap();
        r.publish_event(WifiEvent::StaConnected).unwrap();
        assert!(r.take_rx(InterfaceId::Station).is_none());
        r.enqueue_rx(InterfaceId::Station, record(&NEW)).unwrap();
        assert_eq!(r.take_rx(InterfaceId::Station).unwrap().as_slice(), NEW);
    }
    #[test]
    fn connect_waiter_reports_failure_and_drop_does_not_abort_attempt() {
        let r = Resources::try_new(&QueueConfig::default(), test_allocator()).unwrap();
        let mut sdk = Sdk::new(&r);
        let mut c = Controller {
            sdk: &mut sdk,
            resources: &r,
        };
        c.set_configuration(&station()).unwrap();
        c.start_blocking(1000).unwrap();
        let w = waker();
        let mut cx = Context::from_waker(&w);
        let mut future = HostBox::pin(c.connect_async());
        assert_eq!(future.as_mut().poll(&mut cx), Poll::Pending);
        drop(future);
        assert!(r.state.with(|s| s.connecting));
        assert_eq!(c.begin_connect(false), Err(DriverError::Busy));
        r.publish_event(WifiEvent::StaDisconnected { reason: Some(202) })
            .unwrap();
        assert_eq!(
            c.connect_result(),
            Some(Err(DriverError::Disconnected { reason: Some(202) }))
        );
        let mut future = HostBox::pin(c.connect_async());
        assert_eq!(future.as_mut().poll(&mut cx), Poll::Pending);
        r.publish_event(WifiEvent::StaDisconnected { reason: Some(201) })
            .unwrap();
        assert_eq!(
            future.as_mut().poll(&mut cx),
            Poll::Ready(Err(DriverError::Disconnected { reason: Some(201) }))
        );
    }
    #[test]
    fn blocking_timeout_and_disconnect_wait_preserve_actual_completion() {
        let r = Resources::try_new(&QueueConfig::default(), test_allocator()).unwrap();
        let mut sdk = Sdk::new(&r);
        let mut c = Controller {
            sdk: &mut sdk,
            resources: &r,
        };
        c.set_configuration(&station()).unwrap();
        c.start_blocking(1000).unwrap();
        assert_eq!(c.connect_blocking(0), Err(DriverError::Timeout));
        assert!(r.state.with(|s| s.connecting));
        assert_eq!(c.disconnect_blocking(0), Err(DriverError::Timeout));
        assert!(r.state.with(|s| s.disconnecting));
        r.publish_event(WifiEvent::StaDisconnected { reason: Some(8) })
            .unwrap();
        assert!(c.disconnected());
        let count = c.sdk.commands;
        r.publish_event(WifiEvent::StaDisconnected { reason: Some(201) })
            .unwrap();
        assert_eq!(c.sdk.commands, count);
    }
    #[test]
    fn native_error_and_partial_configuration_failure_keep_precise_state() {
        let r = Resources::try_new(&QueueConfig::default(), test_allocator()).unwrap();
        let mut sdk = Sdk::new(&r);
        let mut c = Controller {
            sdk: &mut sdk,
            resources: &r,
        };
        c.set_configuration(&station()).unwrap();
        c.sdk.fail_next = Some(DriverError::Sdk(123));
        assert_eq!(c.set_configuration(&station()), Err(DriverError::Sdk(123)));
        assert!(c.sdk.mode.is_none());
        assert!(!r.state.with(|s| s.station_configured));
        assert_eq!(c.start_blocking(1000), Err(DriverError::InvalidState));
    }
    #[test]
    fn blocking_scan_timeout_can_be_cancelled_without_executor() {
        let r = Resources::try_new(&QueueConfig::default(), test_allocator()).unwrap();
        let mut sdk = Sdk::new(&r);
        let mut c = Controller {
            sdk: &mut sdk,
            resources: &r,
        };
        c.set_configuration(&station()).unwrap();
        c.start_blocking(1000).unwrap();
        let mut records = [AccessPointRecord::default()];
        assert_eq!(
            c.scan_blocking(&ScanOptions::all(), &mut records, 0),
            Err(DriverError::Timeout)
        );
        c.cancel_scan().unwrap();
        r.publish_event(WifiEvent::ScanDone { status: 1 }).unwrap();
        c.cancel_scan_blocking(0).unwrap();
        assert_eq!(c.begin_connect(false), Ok(()));
    }
    #[test]
    fn dropped_scan_keeps_result_ownership_until_completion_and_cleanup() {
        let r = Resources::try_new(&QueueConfig::default(), test_allocator()).unwrap();
        let mut sdk = Sdk::new(&r);
        let mut c = Controller {
            sdk: &mut sdk,
            resources: &r,
        };
        c.enable_station().unwrap();
        c.start_blocking(1000).unwrap();
        let w = waker();
        let mut cx = Context::from_waker(&w);
        let mut records = [AccessPointRecord::default()];
        let options = ScanOptions::all();
        let mut future = HostBox::pin(c.scan_async(&options, &mut records));
        assert_eq!(future.as_mut().poll(&mut cx), Poll::Pending);
        drop(future);
        assert_eq!(c.begin_scan(&options), Err(DriverError::Busy));
        r.publish_event(WifiEvent::ScanDone { status: 0 }).unwrap();
        let before = c.sdk.commands;
        c.begin_scan(&options).unwrap();
        assert_eq!(c.sdk.commands, before + 2);
        c.cancel_scan().unwrap();
        assert!(r.state.with(|s| s.scan_pending));
        r.publish_event(WifiEvent::ScanDone { status: 1 }).unwrap();
        let mut cancel = HostBox::pin(c.cancel_scan_async());
        assert_eq!(cancel.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
        drop(cancel);
        assert!(!r.state.with(|s| s.scan_pending));
    }
}

#[cfg(test)]
mod ownership_tests {
    extern crate std;
    use super::super::allocator::TrackingAllocator;
    use super::*;
    use core::sync::atomic::Ordering;
    #[test]
    fn every_partial_allocation_failure_rolls_back_queues_and_header() {
        let mut config = InitConfig::default();
        config.queues = super::super::QueueConfig {
            access_point_rx: 2,
            esp_now_peers: 2,
            esp_now_rx: 2,
            promiscuous_rx: 2,
            ..super::super::QueueConfig::default()
        };
        let mut succeeded = false;
        for budget in 0..16 {
            let (provider, allocator) = TrackingAllocator::new(budget);
            let result = Wifi::allocate_resources(&config, allocator);
            succeeded |= result.is_ok();
            drop(result);
            assert_eq!(
                provider.live.load(Ordering::SeqCst),
                0,
                "failure budget {budget}"
            );
        }
        assert!(succeeded);
    }
    #[test]
    fn forgetting_owner_retains_callback_storage() {
        let (provider, allocator) = TrackingAllocator::new(32);
        let resources = Wifi::allocate_resources(&InitConfig::default(), allocator).unwrap();
        let ptr = &*resources as *const Resources;
        let wifi = Wifi {
            sdk: Sdk::new(&resources),
            resources: Some(resources),
            station_mac: [0; 6],
            ap_mac: [0; 6],
        };
        let before = provider.live.load(Ordering::SeqCst);
        core::mem::forget(wifi);
        assert_eq!(provider.live.load(Ordering::SeqCst), before);
        assert_eq!(unsafe { &*ptr }.frame_bytes, 1514);
    }
    #[test]
    fn forgotten_ap_rx_lease_quarantines_owner_without_native_teardown() {
        use super::super::packet::RawRxRecord;
        static BYTES: [u8; 64] = [0xa5; 64];
        let (provider, allocator) = TrackingAllocator::new(32);
        let mut config = InitConfig::default();
        config.queues.access_point_rx = 1;
        let resources = Wifi::allocate_resources(&config, allocator).unwrap();
        resources
            .publish_event(super::super::WifiEvent::ApStarted)
            .unwrap();
        unsafe fn release_static(_: *mut core::ffi::c_void, _: *mut core::ffi::c_void) {}
        // SAFETY: immutable static bytes remain valid even when the lease is
        // deliberately forgotten. The release hook has no work for static data.
        let record = unsafe {
            RawRxRecord::from_raw(
                BYTES.as_ptr().cast_mut().cast(),
                64,
                core::ptr::null_mut(),
                release_static,
            )
        };
        resources
            .enqueue_rx(super::super::resources::InterfaceId::AccessPoint, record)
            .unwrap();
        let packet = resources
            .take_rx(super::super::resources::InterfaceId::AccessPoint)
            .unwrap();
        core::mem::forget(packet);
        let ptr = &*resources as *const Resources;
        let mut sdk = Sdk::new(&resources);
        sdk.started = true;
        let wifi = Wifi {
            sdk,
            resources: Some(resources),
            station_mac: [0; 6],
            ap_mac: [0; 6],
        };
        let before = provider.live.load(Ordering::SeqCst);
        drop(wifi);
        assert_eq!(provider.live.load(Ordering::SeqCst), before);
        // A native shutdown would reset ap_started. Busy lease admission must
        // retain both the callback memory and the running SDK instead.
        assert!(unsafe { &*ptr }.state.with(|s| s.ap_started));
    }
    #[test]
    fn failed_teardown_retains_callback_storage() {
        let (provider, allocator) = TrackingAllocator::new(32);
        let resources = Wifi::allocate_resources(&InitConfig::default(), allocator).unwrap();
        let ptr = &*resources as *const Resources;
        let mut sdk = Sdk::new(&resources);
        sdk.fail_next = Some(DriverError::Sdk(9));
        let wifi = Wifi {
            sdk,
            resources: Some(resources),
            station_mac: [0; 6],
            ap_mac: [0; 6],
        };
        let before = provider.live.load(Ordering::SeqCst);
        drop(wifi);
        assert_eq!(provider.live.load(Ordering::SeqCst), before);
        assert_eq!(unsafe { &*ptr }.frame_bytes, 1514);
    }
}
