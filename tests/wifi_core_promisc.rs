use esp8266_hal::wifi_core::{
    DriverError, InitConfig, PromiscuousControlFilter, PromiscuousFilter, PromiscuousFrameInfo,
    PromiscuousPacketType, PromiscuousRxFrame, RawMode, WifiApBackend, WifiDriver,
    WifiFeatureBackend, WifiLifecycleBackend, WifiPacketBackend, WifiResources, WifiRuntimeBackend,
    WifiScanBackend, WifiStaBackend,
};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Default)]
struct FakePromiscState {
    enabled: bool,
    disabled: bool,
    enable_count: u8,
    disable_count: u8,
    packet_filter: Option<PromiscuousFilter>,
    control_filter: Option<PromiscuousControlFilter>,
    events: Vec<PromiscEvent>,
}

#[derive(Clone, Default)]
struct FakePromiscBackend {
    state: Rc<RefCell<FakePromiscState>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PromiscEvent {
    Enable(PromiscuousFilter),
    Control(PromiscuousControlFilter),
    Disable,
}

impl WifiRuntimeBackend for FakePromiscBackend {
    fn now_ms(&self) -> u64 {
        0
    }

    fn spin_once(&mut self) {}
}

impl WifiLifecycleBackend for FakePromiscBackend {
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

impl WifiFeatureBackend for FakePromiscBackend {
    fn enable_promiscuous(&mut self, filter: PromiscuousFilter) -> Result<(), DriverError> {
        let mut state = self.state.borrow_mut();
        state.enabled = true;
        state.enable_count = state.enable_count.saturating_add(1);
        state.packet_filter = Some(filter);
        state.events.push(PromiscEvent::Enable(filter));
        Ok(())
    }

    fn disable_promiscuous(&mut self) -> Result<(), DriverError> {
        let mut state = self.state.borrow_mut();
        state.disabled = true;
        state.disable_count = state.disable_count.saturating_add(1);
        state.events.push(PromiscEvent::Disable);
        Ok(())
    }

    fn set_promiscuous_control_filter(
        &mut self,
        filter: PromiscuousControlFilter,
    ) -> Result<(), DriverError> {
        let mut state = self.state.borrow_mut();
        state.control_filter = Some(filter);
        state.events.push(PromiscEvent::Control(filter));
        Ok(())
    }
}

impl WifiStaBackend for FakePromiscBackend {}
impl WifiApBackend for FakePromiscBackend {}
impl WifiPacketBackend for FakePromiscBackend {}
impl WifiScanBackend for FakePromiscBackend {}

#[test]
fn promiscuous_session_enables_dequeues_and_disables() {
    let resources = WifiResources::<8, 4, 2, 0, 0, 2>::new();
    let mut driver = WifiDriver::with_backend(&resources, FakePromiscBackend::default())
        .initialize(InitConfig::default())
        .unwrap()
        .enable_sta()
        .unwrap()
        .start()
        .unwrap();

    let mut session = driver.promiscuous().unwrap();
    session.start(PromiscuousFilter::all()).unwrap();
    assert!(session.is_active());
    assert_eq!(session.filter(), PromiscuousFilter::all());

    resources
        .enqueue_promiscuous_rx(
            PromiscuousRxFrame::new(
                PromiscuousFrameInfo {
                    rssi: -42,
                    channel: 6,
                    packet_type: PromiscuousPacketType::Management,
                },
                &[1, 2, 3],
            )
            .unwrap(),
        )
        .unwrap();

    let frame = session.receive().expect("promiscuous frame");
    assert_eq!(frame.info().channel, 6);
    assert_eq!(frame.payload(), &[1, 2, 3]);

    session.stop().unwrap();
    assert!(!session.is_active());
    assert_eq!(session.filter(), PromiscuousFilter::empty());
}

#[test]
fn promiscuous_session_sets_control_subtype_filter() {
    let resources = WifiResources::<8, 4, 2, 0, 0, 1>::new();
    let backend = FakePromiscBackend::default();
    let state = Rc::clone(&backend.state);
    let mut driver = WifiDriver::with_backend(&resources, backend)
        .initialize(InitConfig::default())
        .unwrap()
        .enable_sta()
        .unwrap()
        .start()
        .unwrap();

    let mut session = driver.promiscuous().unwrap();
    let filter = PromiscuousControlFilter::ACK.union(PromiscuousControlFilter::RTS);

    session.set_control_filter(filter).unwrap();

    let state = state.borrow();
    assert_eq!(state.control_filter, None);
    assert!(state.events.is_empty());
    assert_eq!(session.control_filter(), filter);
}

#[test]
fn inactive_control_filter_applies_after_start() {
    let resources = WifiResources::<8, 4, 2, 0, 0, 1>::new();
    let backend = FakePromiscBackend::default();
    let state = Rc::clone(&backend.state);
    let mut driver = WifiDriver::with_backend(&resources, backend)
        .initialize(InitConfig::default())
        .unwrap()
        .enable_sta()
        .unwrap()
        .start()
        .unwrap();

    let mut session = driver.promiscuous().unwrap();
    let control_filter = PromiscuousControlFilter::ACK;

    session.set_control_filter(control_filter).unwrap();
    session.start(PromiscuousFilter::all()).unwrap();

    assert_eq!(session.filter(), PromiscuousFilter::all());
    assert_eq!(session.control_filter(), control_filter);
    assert_eq!(
        state.borrow().events,
        [
            PromiscEvent::Enable(PromiscuousFilter::all()),
            PromiscEvent::Control(control_filter)
        ]
    );
}

#[test]
fn promiscuous_session_tracks_packet_filter_transitions() {
    let resources = WifiResources::<8, 4, 2, 0, 0, 1>::new();
    let backend = FakePromiscBackend::default();
    let state = Rc::clone(&backend.state);
    let mut driver = WifiDriver::with_backend(&resources, backend)
        .initialize(InitConfig::default())
        .unwrap()
        .enable_sta()
        .unwrap()
        .start()
        .unwrap();

    let mut session = driver.promiscuous().unwrap();
    session.start(PromiscuousFilter::CONTROL).unwrap();
    assert_eq!(session.filter(), PromiscuousFilter::CONTROL);
    assert_eq!(
        state.borrow().packet_filter,
        Some(PromiscuousFilter::CONTROL)
    );

    session.stop().unwrap();
    session.start(PromiscuousFilter::all()).unwrap();
    assert_eq!(session.filter(), PromiscuousFilter::all());
    assert_eq!(state.borrow().packet_filter, Some(PromiscuousFilter::all()));
}

#[test]
fn active_promiscuous_control_filter_change_applies_directly() {
    let resources = WifiResources::<8, 4, 2, 0, 0, 1>::new();
    let backend = FakePromiscBackend::default();
    let state = Rc::clone(&backend.state);
    let mut driver = WifiDriver::with_backend(&resources, backend)
        .initialize(InitConfig::default())
        .unwrap()
        .enable_sta()
        .unwrap()
        .start()
        .unwrap();

    let mut session = driver.promiscuous().unwrap();
    session.start(PromiscuousFilter::CONTROL).unwrap();
    session
        .set_control_filter(PromiscuousControlFilter::ACK)
        .unwrap();

    let state = state.borrow();
    assert_eq!(state.disable_count, 0);
    assert_eq!(state.enable_count, 1);
    assert_eq!(state.packet_filter, Some(PromiscuousFilter::CONTROL));
    assert_eq!(state.control_filter, Some(PromiscuousControlFilter::ACK));
    assert_eq!(
        state.events,
        [
            PromiscEvent::Enable(PromiscuousFilter::CONTROL),
            PromiscEvent::Control(PromiscuousControlFilter::ACK)
        ]
    );
    assert!(session.is_active());
    assert_eq!(session.filter(), PromiscuousFilter::CONTROL);
}

#[test]
fn stop_preserves_requested_control_filter_for_restart() {
    let resources = WifiResources::<8, 4, 2, 0, 0, 1>::new();
    let mut driver = WifiDriver::with_backend(&resources, FakePromiscBackend::default())
        .initialize(InitConfig::default())
        .unwrap()
        .enable_sta()
        .unwrap()
        .start()
        .unwrap();

    let mut session = driver.promiscuous().unwrap();
    session
        .set_control_filter(PromiscuousControlFilter::ACK)
        .unwrap();
    session.start(PromiscuousFilter::CONTROL).unwrap();
    session.stop().unwrap();

    assert!(!session.is_active());
    assert_eq!(session.filter(), PromiscuousFilter::empty());
    assert_eq!(session.control_filter(), PromiscuousControlFilter::ACK);
}
