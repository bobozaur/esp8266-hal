use core::ffi::c_void;
use core::sync::atomic::{AtomicUsize, Ordering};

use esp8266_hal::wifi_core::{DriverError, RawRxRecord, RxRing};

unsafe fn count_release(_: *mut c_void, counter: *mut c_void) {
    let counter = unsafe { &*(counter as *const AtomicUsize) };
    counter.fetch_add(1, Ordering::SeqCst);
}

fn counted_record(len: u16, counter: &AtomicUsize) -> RawRxRecord {
    unsafe {
        RawRxRecord::from_raw(
            core::ptr::null_mut(),
            len,
            counter as *const AtomicUsize as *mut c_void,
            Some(count_release),
        )
    }
}

#[test]
fn rx_ring_preserves_fifo_order_and_releases_on_copy() {
    let releases = AtomicUsize::new(0);
    let mut ring = RxRing::<2>::new();

    ring.try_push(counted_record(10, &releases)).unwrap();
    ring.try_push(counted_record(20, &releases)).unwrap();

    let mut dst = [0; 64];
    assert_eq!(ring.pop().unwrap().copy_into(&mut dst), 10);
    assert_eq!(ring.pop().unwrap().copy_into(&mut dst), 20);
    assert!(ring.pop().is_none());
    assert_eq!(releases.load(Ordering::SeqCst), 2);
}

#[test]
fn rejected_rx_record_is_returned_for_explicit_release() {
    let releases = AtomicUsize::new(0);
    let mut ring = RxRing::<0>::new();

    let rejected = ring
        .try_push(counted_record(10, &releases))
        .expect_err("zero-capacity ring rejects");

    assert_eq!(rejected.error(), DriverError::PacketOverflow);
    assert_eq!(releases.load(Ordering::SeqCst), 0);
    rejected.release();
    assert_eq!(releases.load(Ordering::SeqCst), 1);
}
