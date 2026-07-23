use core::sync::atomic::{AtomicUsize, Ordering};
use core::task::{RawWaker, RawWakerVTable, Waker};

use esp8266_hal::wifi_core::{DriverError, LinkState, WifiDataLink};

static LINK_WAKES: AtomicUsize = AtomicUsize::new(0);
static FAKE_RX_BYTES: [u8; 64] = [0xa5; 64];

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

#[derive(Default)]
struct FakeDataLink {
    rx_len: Option<usize>,
    tx_attempts: usize,
    refresh_error: bool,
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

    fn max_frame_size(&self) -> usize {
        1514
    }

    fn refresh_rx(&mut self) -> Result<(), DriverError> {
        if self.refresh_error {
            return Err(DriverError::BackendRejected);
        }
        self.rx_len = Some(8);
        Ok(())
    }

    fn has_rx_data(&self) -> bool {
        self.rx_len.is_some()
    }

    fn receive(&mut self, dst: &mut [u8]) -> Result<Option<usize>, DriverError> {
        let Some(len) = self.rx_len.take() else {
            return Ok(None);
        };
        if dst.len() < len {
            return Err(DriverError::PacketOverflow);
        }
        dst[..len].copy_from_slice(&FAKE_RX_BYTES[..len]);
        Ok(Some(len))
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

    let mut frame = [0; 16];
    assert_eq!(link.receive(&mut frame), Ok(Some(8)));
    assert_eq!(&frame[..8], &[0xa5; 8]);
    assert!(!link.has_rx_data());

    assert_eq!(link.transmit(&[1, 2, 3]), Err(DriverError::WouldBlock));
    assert_eq!(link.transmit(&[1, 2, 3]), Ok(()));
}

#[cfg(feature = "wifi-embassy-net")]
#[test]
fn embassy_link_rejects_zero_packet_channels_without_panicking() {
    use esp8266_hal::wifi::embassy_net::{self, NetDriverStateWithMtu};

    let mut no_rx = NetDriverStateWithMtu::<1514, 0, 1>::new();
    assert!(matches!(
        embassy_net::new_link(&mut no_rx, FakeDataLink::default()),
        Err(DriverError::PacketOverflow)
    ));

    let mut no_tx = NetDriverStateWithMtu::<1514, 1, 0>::new();
    assert!(matches!(
        embassy_net::new_link(&mut no_tx, FakeDataLink::default()),
        Err(DriverError::PacketOverflow)
    ));

    let mut state = NetDriverStateWithMtu::<1514, 1, 1>::new();
    assert!(matches!(
        embassy_net::new_link(
            &mut state,
            FakeDataLink {
                refresh_error: true,
                ..FakeDataLink::default()
            }
        ),
        Err(DriverError::BackendRejected)
    ));
}
