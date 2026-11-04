use super::{allocator::RuntimeAllocator, queue::Queue, DriverError, InitError};
use allocator_api2::vec::Vec;
use core::{cell::UnsafeCell, ffi::c_void};

pub(crate) type ReleaseFn = unsafe fn(*mut c_void, *mut c_void);

#[derive(Clone, Copy)]
pub(crate) struct OwnedRxLease {
    pub buffer: *mut c_void,
    pub len: u16,
    pub slot: *mut c_void,
}

/// Copied AP packets have stationary bytes and independently leased slots.
/// Neither vector is resized after publication. Shared pool access never creates
/// an exclusive reference covering bytes already borrowed by another packet.
pub(crate) struct OwnedRxPool {
    occupied: Vec<UnsafeCell<bool>, RuntimeAllocator>,
    bytes: Vec<UnsafeCell<u8>, RuntimeAllocator>,
    frame_bytes: usize,
}

impl OwnedRxPool {
    pub(crate) fn try_new(
        slots: usize,
        frame_bytes: usize,
        allocator: RuntimeAllocator,
    ) -> Result<Self, InitError> {
        let total_bytes = slots
            .checked_mul(frame_bytes)
            .ok_or(InitError::OutOfMemory)?;
        let mut occupied = Vec::new_in(allocator);
        occupied
            .try_reserve_exact(slots)
            .map_err(|_| InitError::OutOfMemory)?;
        occupied.resize_with(slots, || UnsafeCell::new(false));
        let mut bytes = Vec::new_in(allocator);
        bytes
            .try_reserve_exact(total_bytes)
            .map_err(|_| InitError::OutOfMemory)?;
        bytes.resize_with(total_bytes, || UnsafeCell::new(0));
        Ok(Self {
            occupied,
            bytes,
            frame_bytes,
        })
    }

    pub(crate) fn try_acquire_copy(&self, src: &[u8]) -> Result<OwnedRxLease, DriverError> {
        if src.len() > self.frame_bytes || src.len() > u16::MAX as usize {
            return Err(DriverError::PayloadTooLong);
        }
        critical_section::with(|_| {
            for (index, occupied) in self.occupied.iter().enumerate() {
                // SAFETY: admission and release use the same critical section;
                // a free slot has no outstanding byte borrow or release owner.
                unsafe {
                    if *occupied.get() {
                        continue;
                    }
                    let buffer = self
                        .bytes
                        .as_ptr()
                        .add(index * self.frame_bytes)
                        .cast::<u8>()
                        .cast_mut();
                    core::ptr::copy_nonoverlapping(src.as_ptr(), buffer, src.len());
                    *occupied.get() = true;
                    return Ok(OwnedRxLease {
                        buffer: buffer.cast(),
                        len: src.len() as u16,
                        slot: occupied.get().cast(),
                    });
                }
            }
            Err(DriverError::PacketOverflow)
        })
    }
}

/// Release one copied AP packet slot without touching its bytes.
///
/// # Safety
/// `slot` must be null or an unreleased live slot returned by this pool. The
/// pool must outlive the lease and all byte borrows must have ended.
pub(crate) unsafe fn release_owned_rx_slot(_buffer: *mut c_void, slot: *mut c_void) {
    if !slot.is_null() {
        critical_section::with(|_| {
            // SAFETY: the caller supplies the live slot and sole release right.
            unsafe { *slot.cast::<bool>() = false };
        });
    }
}

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
    use super::super::allocator::test_allocator;
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
    fn owned_pool_reuses_slot_after_release() {
        let pool = OwnedRxPool::try_new(1, 8, test_allocator()).unwrap();
        let bytes = [1_u8, 2, 3, 4];

        let first = pool.try_acquire_copy(&bytes).expect("first slot");
        assert!(pool.try_acquire_copy(&bytes).is_err());

        unsafe { release_owned_rx_slot(first.buffer, first.slot) };

        let second = pool.try_acquire_copy(&bytes).expect("reused slot");
        assert!(!second.buffer.is_null());
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
            resources.enqueue_rx(record).unwrap();
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

    #[test]
    fn owned_pool_rejects_oversize_packets() {
        let pool = OwnedRxPool::try_new(1, 4, test_allocator()).unwrap();
        let bytes = [0_u8; 8];

        assert!(matches!(
            pool.try_acquire_copy(&bytes),
            Err(DriverError::PayloadTooLong)
        ));
    }

    #[test]
    fn pool_preserves_borrowed_packet_while_other_slots_change() {
        let pool = OwnedRxPool::try_new(2, 8, test_allocator()).unwrap();
        let first = pool.try_acquire_copy(b"first").unwrap();
        // SAFETY: the first lease remains occupied throughout this borrow.
        let borrowed = unsafe { core::slice::from_raw_parts(first.buffer.cast::<u8>(), 5) };
        let second = pool.try_acquire_copy(b"second").unwrap();
        assert_eq!(borrowed, b"first");
        // SAFETY: no byte borrow of the second lease remains.
        unsafe { release_owned_rx_slot(second.buffer, second.slot) };
        let third = pool.try_acquire_copy(b"third").unwrap();
        assert_eq!(third.slot, second.slot);
        assert_eq!(borrowed, b"first");
        // SAFETY: both leases are live and their last byte uses have ended.
        unsafe {
            release_owned_rx_slot(first.buffer, first.slot);
            release_owned_rx_slot(third.buffer, third.slot);
        }
    }

    #[test]
    fn partial_pool_allocation_failure_releases_metadata() {
        use super::super::allocator::WifiAllocator;
        use core::{
            alloc::{GlobalAlloc, Layout},
            sync::atomic::{AtomicUsize, Ordering},
        };
        struct FailSecondAllocation {
            calls: AtomicUsize,
            live: AtomicUsize,
        }
        unsafe impl GlobalAlloc for FailSecondAllocation {
            unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
                if self.calls.fetch_add(1, Ordering::SeqCst) != 0 {
                    return core::ptr::null_mut();
                }
                // SAFETY: this host fixture delegates the unchanged layout.
                let ptr = unsafe { std::alloc::System.alloc(layout) };
                if !ptr.is_null() {
                    self.live.fetch_add(1, Ordering::SeqCst);
                }
                ptr
            }
            unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
                self.live.fetch_sub(1, Ordering::SeqCst);
                // SAFETY: this provider created ptr with this layout.
                unsafe { std::alloc::System.dealloc(ptr, layout) };
            }
        }
        // SAFETY: permanent synchronized host fixture, with fallible allocation;
        // no native SDK uses its statistics or DRAM requirements in this test.
        unsafe impl WifiAllocator for FailSecondAllocation {
            fn free_dram_bytes(&self) -> usize {
                0
            }
        }
        static PROVIDER: FailSecondAllocation = FailSecondAllocation {
            calls: AtomicUsize::new(0),
            live: AtomicUsize::new(0),
        };
        let result = OwnedRxPool::try_new(2, 8, RuntimeAllocator::new(&PROVIDER));
        assert!(matches!(result, Err(InitError::OutOfMemory)));
        assert_eq!(PROVIDER.live.load(Ordering::SeqCst), 0);
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
