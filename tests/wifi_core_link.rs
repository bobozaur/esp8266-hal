use core::ffi::c_void;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::task::{RawWaker, RawWakerVTable, Waker};

use esp8266_hal::wifi_core::{
    DriverError, LinkState, RawRxRecord, WifiDataLink, WifiEvent, WifiResources,
};

static LINK_WAKES: AtomicUsize = AtomicUsize::new(0);
static RX_WAKES: AtomicUsize = AtomicUsize::new(0);
static RELEASES: AtomicUsize = AtomicUsize::new(0);
static FAKE_RX_BYTES: [u8; 8] = [0xa5; 8];

unsafe fn clone_counter(data: *const ()) -> RawWaker {
    RawWaker::new(data, &VTABLE)
}

unsafe fn wake_counter(data: *const ()) {
    let counter = unsafe { &*(data as *const AtomicUsize) };
    counter.fetch_add(1, Ordering::SeqCst);
}

unsafe fn wake_counter_by_ref(data: *const ()) {
    unsafe { wake_counter(data) };
}

unsafe fn drop_counter(_: *const ()) {}

static VTABLE: RawWakerVTable = RawWakerVTable::new(
    clone_counter,
    wake_counter,
    wake_counter_by_ref,
    drop_counter,
);

fn counter_waker(counter: &'static AtomicUsize) -> Waker {
    let raw = RawWaker::new(counter as *const AtomicUsize as *const (), &VTABLE);
    unsafe { Waker::from_raw(raw) }
}

unsafe fn count_release(_: *mut c_void, _: *mut c_void) {
    RELEASES.fetch_add(1, Ordering::SeqCst);
}

fn fake_record(len: u16) -> RawRxRecord {
    unsafe {
        RawRxRecord::from_raw(
            core::ptr::null_mut(),
            len,
            core::ptr::null_mut(),
            Some(count_release),
        )
    }
}

#[test]
fn publishing_link_events_updates_state_and_wakes_waiters() {
    LINK_WAKES.store(0, Ordering::SeqCst);

    let resources = WifiResources::<4, 2, 1>::new();
    let waker = counter_waker(&LINK_WAKES);
    resources.register_link_waker(&waker);

    assert_eq!(resources.link_state(), LinkState::Down);
    assert_eq!(resources.last_disconnect_reason(), None);

    resources.publish_event(WifiEvent::StaConnected).unwrap();
    assert_eq!(resources.link_state(), LinkState::Up);
    assert_eq!(LINK_WAKES.load(Ordering::SeqCst), 1);

    resources
        .publish_event(WifiEvent::StaDisconnected { reason: Some(7) })
        .unwrap();
    assert_eq!(resources.link_state(), LinkState::Down);
    assert_eq!(resources.last_disconnect_reason(), Some(7));
    assert_eq!(LINK_WAKES.load(Ordering::SeqCst), 2);

    resources
        .publish_event(WifiEvent::StaDisconnected { reason: Some(255) })
        .unwrap();
    assert_eq!(resources.last_disconnect_reason(), Some(255));
}

#[test]
fn rx_enqueue_wakes_waiters_and_overflow_releases_immediately() {
    RX_WAKES.store(0, Ordering::SeqCst);
    RELEASES.store(0, Ordering::SeqCst);

    let resources = WifiResources::<4, 1, 1>::new();
    let waker = counter_waker(&RX_WAKES);
    resources.register_rx_waker(&waker);

    assert!(resources.enqueue_rx(fake_record(32)).is_ok());
    assert_eq!(resources.queued_rx(), 1);
    assert_eq!(RX_WAKES.load(Ordering::SeqCst), 1);

    assert_eq!(
        resources.enqueue_rx(fake_record(64)),
        Err(DriverError::PacketOverflow)
    );
    assert_eq!(resources.rx_drops(), 1);
    assert_eq!(RELEASES.load(Ordering::SeqCst), 1);

    let mut frame = [0; 32];
    assert_eq!(resources.dequeue_rx().unwrap().copy_into(&mut frame), 32);
    assert_eq!(resources.queued_rx(), 0);
    assert_eq!(resources.rx_drops(), 1);
    assert_eq!(RELEASES.load(Ordering::SeqCst), 2);
}

#[test]
fn event_overflow_is_counted() {
    let resources = WifiResources::<1, 1, 1>::new();
    let _cursor = resources.claim_event_cursor().unwrap();

    resources.publish_event(WifiEvent::StaStarted).unwrap();
    assert_eq!(
        resources.publish_event(WifiEvent::StaConnected),
        Err(DriverError::EventOverflow)
    );
    assert_eq!(resources.event_overflows(), 1);
}

#[test]
fn no_subscriber_event_discard_is_counted() {
    let resources = WifiResources::<1, 1, 1>::new();

    resources.publish_event(WifiEvent::StaConnected).unwrap();

    assert_eq!(resources.link_state(), LinkState::Up);
    assert_eq!(resources.event_discards(), 1);
    assert_eq!(resources.event_overflows(), 0);
}

#[test]
fn zero_capacity_sta_rx_releases_rejected_frame_without_wake() {
    RX_WAKES.store(0, Ordering::SeqCst);
    RELEASES.store(0, Ordering::SeqCst);

    let resources = WifiResources::<4, 0, 1>::new();
    let waker = counter_waker(&RX_WAKES);
    resources.register_rx_waker(&waker);

    assert_eq!(
        resources.enqueue_rx(fake_record(32)),
        Err(DriverError::PacketOverflow)
    );
    assert_eq!(resources.queued_rx(), 0);
    assert_eq!(resources.rx_drops(), 1);
    assert_eq!(RX_WAKES.load(Ordering::SeqCst), 0);
    assert_eq!(RELEASES.load(Ordering::SeqCst), 1);
}

#[derive(Default)]
struct FakeDataLink {
    rx_len: Option<usize>,
    tx_attempts: usize,
}

impl WifiDataLink for FakeDataLink {
    fn register_link_waker(&self, _waker: &Waker) {}

    fn register_rx_waker(&self, _waker: &Waker) {}

    fn link_state(&self) -> LinkState {
        LinkState::Up
    }

    fn mac_address(&self) -> Result<[u8; 6], DriverError> {
        Ok([1, 2, 3, 4, 5, 6])
    }

    fn refresh_rx(&mut self) -> Result<(), DriverError> {
        self.rx_len = Some(8);
        Ok(())
    }

    fn has_rx_data(&self) -> bool {
        self.rx_len.is_some()
    }

    fn queued_rx(&self) -> usize {
        usize::from(self.rx_len.is_some())
    }

    fn rx_drops(&self) -> u32 {
        0
    }

    fn receive_raw(&mut self) -> Option<RawRxRecord> {
        let len = self.rx_len.take()?;
        Some(unsafe {
            RawRxRecord::from_raw(
                FAKE_RX_BYTES.as_ptr().cast_mut().cast(),
                len as u16,
                core::ptr::null_mut(),
                None,
            )
        })
    }

    fn transmit(&mut self, _frame: &[u8]) -> Result<(), DriverError> {
        self.tx_attempts += 1;
        if self.tx_attempts == 1 {
            Err(DriverError::WouldBlock)
        } else {
            Ok(())
        }
    }
}

#[test]
fn data_link_contract_covers_rx_tx_and_backpressure() {
    let mut link = FakeDataLink::default();
    let waker = counter_waker(&LINK_WAKES);

    link.register_link_waker(&waker);
    link.register_rx_waker(&waker);
    assert_eq!(link.mac_address().unwrap(), [1, 2, 3, 4, 5, 6]);
    assert_eq!(link.link_state(), LinkState::Up);

    link.refresh_rx().unwrap();
    assert!(link.has_rx_data());
    assert_eq!(link.queued_rx(), 1);

    let mut frame = [0; 16];
    assert_eq!(link.receive(&mut frame), Some(8));
    assert_eq!(&frame[..8], &[0xa5; 8]);
    assert!(!link.has_rx_data());

    assert_eq!(link.transmit(&[1, 2, 3]), Err(DriverError::WouldBlock));
    assert_eq!(link.transmit(&[1, 2, 3]), Ok(()));
}
