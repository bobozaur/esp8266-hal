use super::queue::Queue;
use core::ffi::c_void;

pub(crate) type ReleaseFn = unsafe fn(*mut c_void, *mut c_void);

/// One receive buffer and its sole release responsibility.
pub(crate) struct RawRxRecord {
    buffer: *mut c_void,
    len: u16,
    eb: *mut c_void,
    release_fn: Option<ReleaseFn>,
}

impl RawRxRecord {
    /// Takes ownership of a receive buffer and its optional release hook.
    ///
    /// # Safety
    /// For nonzero `len`, `buffer` must point to `len` initialized bytes that
    /// remain valid and unchanged until release. The caller transfers sole
    /// release responsibility: the hook must be valid exactly once with these
    /// arguments, after all byte borrows end. Its target must outlive the record.
    pub(crate) const unsafe fn from_raw(
        buffer: *mut c_void,
        len: u16,
        eb: *mut c_void,
        release_fn: Option<ReleaseFn>,
    ) -> Self {
        Self {
            buffer,
            len,
            eb,
            release_fn,
        }
    }

    pub(crate) const fn len(&self) -> usize {
        self.len as usize
    }

    pub(crate) fn as_slice(&self) -> &[u8] {
        if self.len == 0 {
            &[]
        } else {
            // SAFETY: construction transfers an immutable live buffer lease.
            unsafe { core::slice::from_raw_parts(self.buffer.cast(), self.len()) }
        }
    }
}

impl Drop for RawRxRecord {
    fn drop(&mut self) {
        if let Some(release) = self.release_fn.take() {
            // SAFETY: construction transfers the sole live release right.
            unsafe { release(self.buffer, self.eb) };
        }
    }
}

pub(crate) type RxRing = Queue<RawRxRecord>;

#[cfg(test)]
mod tests {
    extern crate std;

    use self::std::format;
    use super::super::{allocator::test_allocator, DriverError};
    use super::*;
    use proptest::prelude::*;

    fn fake_record(len: u16) -> RawRxRecord {
        static BYTES: [u8; 64] = [0; 64];
        assert!(len as usize <= BYTES.len());
        unsafe {
            RawRxRecord::from_raw(
                BYTES.as_ptr().cast_mut().cast(),
                len,
                core::ptr::null_mut(),
                None,
            )
        }
    }

    #[test]
    fn ring_rejects_when_full() {
        let mut ring = RxRing::try_new(1, test_allocator()).unwrap();
        assert!(ring.try_push(fake_record(32)).is_ok());
        assert!(ring.try_push(fake_record(64)).is_err());
    }

    #[test]
    fn zero_capacity_ring_rejects_without_storing() {
        let mut ring = RxRing::try_new(0, test_allocator()).unwrap();
        let rejected = ring
            .try_push(fake_record(32))
            .expect_err("zero-capacity ring must reject");

        assert_eq!(rejected.len(), 32);
        assert_eq!(ring.len(), 0);
        assert!(ring.pop().is_none());
    }

    #[test]
    fn interface_receive_releases_whole_record_on_copy_or_short_buffer() {
        use super::super::{
            resources::{InterfaceId, Resources},
            Interface, QueueConfig, WifiEvent,
        };
        use core::sync::atomic::{AtomicUsize, Ordering};
        static RELEASES: AtomicUsize = AtomicUsize::new(0);
        unsafe fn count_release(_: *mut c_void, _: *mut c_void) {
            RELEASES.fetch_add(1, Ordering::SeqCst);
        }
        RELEASES.store(0, Ordering::SeqCst);
        let resources = Resources::try_new(
            &QueueConfig {
                station_rx: 1,
                ..QueueConfig::default()
            },
            test_allocator(),
        )
        .unwrap();
        resources.publish_event(WifiEvent::StaConnected).unwrap();
        let mut interface = Interface {
            resources: &resources,
            id: InterfaceId::Station,
            mac: [2, 0, 0, 0, 0, 1],
        };
        let bytes = [0xa5; 8];
        for (index, capacity) in [4, 8].into_iter().enumerate() {
            // SAFETY: bytes outlives this receive and there is one release owner.
            let record = unsafe {
                RawRxRecord::from_raw(
                    bytes.as_ptr().cast_mut().cast(),
                    8,
                    core::ptr::null_mut(),
                    Some(count_release),
                )
            };
            resources.enqueue_rx(InterfaceId::Station, record).unwrap();
            let mut destination = [0; 8];
            let result = interface.receive(&mut destination[..capacity]);
            if capacity < bytes.len() {
                assert_eq!(result, Err(DriverError::PacketOverflow));
                assert_eq!(destination, [0; 8]);
            } else {
                assert_eq!(result, Ok(Some(8)));
                assert_eq!(destination, bytes);
            }
            assert_eq!(RELEASES.load(Ordering::SeqCst), index + 1);
            assert_eq!(interface.receive(&mut destination), Ok(None));
        }
    }

    #[test]
    fn dropped_record_releases_ownership_once() {
        static RELEASES: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

        unsafe fn count_release(_: *mut core::ffi::c_void, _: *mut core::ffi::c_void) {
            RELEASES.fetch_add(1, core::sync::atomic::Ordering::SeqCst);
        }

        RELEASES.store(0, core::sync::atomic::Ordering::SeqCst);
        {
            let bytes = [0_u8; 8];
            let _record = unsafe {
                RawRxRecord::from_raw(
                    bytes.as_ptr().cast_mut().cast(),
                    8,
                    core::ptr::null_mut(),
                    Some(count_release),
                )
            };
        }

        assert_eq!(RELEASES.load(core::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn dropped_ring_releases_queued_records() {
        static RELEASES: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

        unsafe fn count_release(_: *mut core::ffi::c_void, _: *mut core::ffi::c_void) {
            RELEASES.fetch_add(1, core::sync::atomic::Ordering::SeqCst);
        }

        RELEASES.store(0, core::sync::atomic::Ordering::SeqCst);
        {
            let mut ring = RxRing::try_new(2, test_allocator()).unwrap();
            let bytes = [0_u8; 8];
            assert!(ring
                .try_push(unsafe {
                    RawRxRecord::from_raw(
                        bytes.as_ptr().cast_mut().cast(),
                        8,
                        core::ptr::null_mut(),
                        Some(count_release),
                    )
                })
                .is_ok());
            drop(ring);
        }

        assert_eq!(RELEASES.load(core::sync::atomic::Ordering::SeqCst), 1);
    }

    proptest! {
        #[test]
        fn ring_len_never_exceeds_capacity(ops in proptest::collection::vec(0u8..3, 1..32)) {
            let mut ring = RxRing::try_new(4, test_allocator()).unwrap();
            for op in ops {
                match op {
                    0 => { let _ = ring.try_push(fake_record(64)); }
                    _ => { let _ = ring.pop(); }
                }
                prop_assert!(ring.len() <= 4);
            }
        }
    }
}
