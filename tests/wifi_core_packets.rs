use core::ffi::c_void;
use core::sync::atomic::{AtomicUsize, Ordering};

use esp8266_hal::wifi_core::{DriverError, RawRxRecord, RxRing};

static RELEASES: AtomicUsize = AtomicUsize::new(0);

unsafe fn count_release(_: *mut c_void, _: *mut c_void) {
    RELEASES.fetch_add(1, Ordering::SeqCst);
}

fn counted_record(len: u16) -> RawRxRecord {
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
fn rx_ring_preserves_fifo_order_and_releases_on_copy() {
    RELEASES.store(0, Ordering::SeqCst);
    let mut ring = RxRing::<2>::new();

    ring.try_push(counted_record(10)).unwrap();
    ring.try_push(counted_record(20)).unwrap();

    let mut dst = [0; 64];
    assert_eq!(ring.pop().unwrap().copy_into(&mut dst), 10);
    assert_eq!(ring.pop().unwrap().copy_into(&mut dst), 20);
    assert!(ring.pop().is_none());
    assert_eq!(RELEASES.load(Ordering::SeqCst), 2);
}

#[test]
fn rejected_rx_record_is_returned_for_explicit_release() {
    RELEASES.store(0, Ordering::SeqCst);
    let mut ring = RxRing::<0>::new();

    let rejected = ring
        .try_push(counted_record(10))
        .expect_err("zero-capacity ring rejects");

    assert_eq!(rejected.error(), DriverError::PacketOverflow);
    assert_eq!(RELEASES.load(Ordering::SeqCst), 0);
    rejected.release();
    assert_eq!(RELEASES.load(Ordering::SeqCst), 1);
}
