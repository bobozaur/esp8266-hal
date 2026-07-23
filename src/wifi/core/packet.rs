use super::DriverError;
use core::ffi::c_void;

pub(crate) type ReleaseFn = unsafe fn(*mut c_void, *mut c_void);

#[derive(Clone, Copy)]
pub(crate) struct OwnedRxLease {
    pub buffer: *mut c_void,
    pub len: u16,
    pub slot: *mut c_void,
}

pub(crate) struct OwnedRxSlot<const BYTES: usize> {
    occupied: bool,
    data: [u8; BYTES],
}

impl<const BYTES: usize> OwnedRxSlot<BYTES> {
    pub const fn new() -> Self {
        Self {
            occupied: false,
            data: [0; BYTES],
        }
    }
}

pub(crate) struct OwnedRxPool<const SLOTS: usize, const BYTES: usize> {
    slots: [OwnedRxSlot<BYTES>; SLOTS],
}

impl<const SLOTS: usize, const BYTES: usize> OwnedRxPool<SLOTS, BYTES> {
    pub const fn new() -> Self {
        Self {
            slots: [const { OwnedRxSlot::new() }; SLOTS],
        }
    }

    pub fn try_acquire_copy(&mut self, src: &[u8]) -> Result<OwnedRxLease, DriverError> {
        if SLOTS == 0 {
            return Err(DriverError::PacketOverflow);
        }
        if src.len() > BYTES || src.len() > u16::MAX as usize {
            return Err(DriverError::PayloadTooLong);
        }

        let Some(slot) = self.slots.iter_mut().find(|slot| !slot.occupied) else {
            return Err(DriverError::PacketOverflow);
        };

        slot.data[..src.len()].copy_from_slice(src);

        slot.occupied = true;
        Ok(OwnedRxLease {
            buffer: slot.data.as_mut_ptr() as *mut c_void,
            len: src.len() as u16,
            slot: slot as *mut OwnedRxSlot<BYTES> as *mut c_void,
        })
    }
}

/// # Safety
///
/// `slot` must be either null or a live pointer returned by
/// [`OwnedRxPool::try_acquire_copy`] for the same `BYTES` value. The lease must
/// not already have been released.
pub(crate) unsafe fn release_owned_rx_slot<const BYTES: usize>(
    _buffer: *mut c_void,
    slot: *mut c_void,
) {
    if slot.is_null() {
        return;
    }

    critical_section::with(|_| unsafe {
        (*(slot as *mut OwnedRxSlot<BYTES>)).occupied = false;
    });
}

struct ReleaseToken {
    buffer: *mut c_void,
    eb: *mut c_void,
    release_fn: Option<ReleaseFn>,
}

impl ReleaseToken {
    const fn new(buffer: *mut c_void, eb: *mut c_void, release_fn: Option<ReleaseFn>) -> Self {
        Self {
            buffer,
            eb,
            release_fn,
        }
    }

    fn release(self) {
        let mut token = self;
        token.release_once();
    }

    fn release_once(&mut self) {
        if let Some(release_fn) = self.release_fn.take() {
            unsafe { release_fn(self.buffer, self.eb) };
        }
    }
}

impl Drop for ReleaseToken {
    fn drop(&mut self) {
        self.release_once();
    }
}

pub(crate) struct RawRxRecord {
    buffer: *mut c_void,
    len: u16,
    release: ReleaseToken,
}

impl RawRxRecord {
    /// Takes ownership of an SDK receive buffer and its optional release hook.
    ///
    /// # Safety
    ///
    /// For a nonzero `len`, `buffer` must be non-null, properly aligned, and
    /// point to `len` initialized bytes that remain valid and are not mutated
    /// until this record is released. When `release_fn` is present, calling it
    /// exactly once with `buffer` and `eb` must be valid and the caller must
    /// transfer the sole release responsibility to this record.
    pub(crate) const unsafe fn from_raw(
        buffer: *mut c_void,
        len: u16,
        eb: *mut c_void,
        release_fn: Option<ReleaseFn>,
    ) -> Self {
        Self {
            buffer,
            len,
            release: ReleaseToken::new(buffer, eb, release_fn),
        }
    }

    pub(crate) const fn len(&self) -> usize {
        self.len as usize
    }

    pub(crate) fn as_slice(&self) -> &[u8] {
        if self.len == 0 {
            &[]
        } else {
            unsafe { core::slice::from_raw_parts(self.buffer.cast(), self.len as usize) }
        }
    }

    pub(crate) fn copy_into(self, dst: &mut [u8]) -> Result<usize, DriverError> {
        let len = self.len();
        if dst.len() < len {
            self.release.release();
            return Err(DriverError::PacketOverflow);
        }
        dst[..len].copy_from_slice(self.as_slice());
        self.release.release();
        Ok(len)
    }
}

pub(crate) struct RxPushError {
    error: DriverError,
    record: RawRxRecord,
}

impl RxPushError {
    pub(crate) fn error(&self) -> DriverError {
        self.error.clone()
    }

    pub(crate) fn release(self) {
        self.record.release.release();
    }
}

impl core::fmt::Debug for RxPushError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RxPushError")
            .field("error", &self.error)
            .field("len", &self.record.len)
            .finish()
    }
}

pub(crate) struct RxRing<const N: usize> {
    entries: [Option<RawRxRecord>; N],
    head: usize,
    tail: usize,
    len: usize,
}

impl<const N: usize> RxRing<N> {
    pub const fn new() -> Self {
        Self {
            entries: [const { None }; N],
            head: 0,
            tail: 0,
            len: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn try_push(&mut self, record: RawRxRecord) -> Result<(), RxPushError> {
        if N == 0 || self.len == N {
            return Err(RxPushError {
                error: DriverError::PacketOverflow,
                record,
            });
        }

        self.entries[self.head] = Some(record);
        self.head = (self.head + 1) % N;
        self.len += 1;
        Ok(())
    }

    pub fn pop(&mut self) -> Option<RawRxRecord> {
        if N == 0 || self.len == 0 {
            return None;
        }

        let record = self.entries[self.tail].take();
        self.tail = (self.tail + 1) % N;
        self.len -= 1;
        record
    }
}

impl<const N: usize> Default for RxRing<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use self::std::format;
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
        let mut ring = RxRing::<1>::new();
        assert!(ring.try_push(fake_record(32)).is_ok());
        assert!(ring.try_push(fake_record(64)).is_err());
    }

    #[test]
    fn zero_capacity_ring_rejects_without_storing() {
        let mut ring = RxRing::<0>::new();
        let rejected = ring
            .try_push(fake_record(32))
            .expect_err("zero-capacity ring must reject");

        assert_eq!(rejected.error(), DriverError::PacketOverflow);
        assert_eq!(ring.len(), 0);
        assert!(ring.pop().is_none());
    }

    #[test]
    fn owned_pool_reuses_slot_after_release() {
        let mut pool = OwnedRxPool::<1, 8>::new();
        let bytes = [1_u8, 2, 3, 4];

        let first = pool.try_acquire_copy(&bytes).expect("first slot");
        assert!(pool.try_acquire_copy(&bytes).is_err());

        unsafe { release_owned_rx_slot::<8>(first.buffer, first.slot) };

        let second = pool.try_acquire_copy(&bytes).expect("reused slot");
        assert!(!second.buffer.is_null());
    }

    #[test]
    fn record_copy_releases_ownership_once() {
        static RELEASES: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

        unsafe fn count_release(_: *mut core::ffi::c_void, _: *mut core::ffi::c_void) {
            RELEASES.fetch_add(1, core::sync::atomic::Ordering::SeqCst);
        }

        RELEASES.store(0, core::sync::atomic::Ordering::SeqCst);
        let mut ring = RxRing::<1>::new();
        let bytes = [0_u8; 8];
        ring.try_push(unsafe {
            RawRxRecord::from_raw(
                bytes.as_ptr().cast_mut().cast(),
                8,
                core::ptr::null_mut(),
                Some(count_release),
            )
        })
        .unwrap();

        let mut dst = [0; 8];
        assert_eq!(ring.pop().unwrap().copy_into(&mut dst), Ok(8));
        assert_eq!(RELEASES.load(core::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn undersized_copy_drops_whole_record_and_releases_ownership() {
        static RELEASES: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

        unsafe fn count_release(_: *mut core::ffi::c_void, _: *mut core::ffi::c_void) {
            RELEASES.fetch_add(1, core::sync::atomic::Ordering::SeqCst);
        }

        RELEASES.store(0, core::sync::atomic::Ordering::SeqCst);
        let bytes = [0xa5_u8; 8];
        let record = unsafe {
            RawRxRecord::from_raw(
                bytes.as_ptr().cast_mut().cast(),
                bytes.len() as u16,
                core::ptr::null_mut(),
                Some(count_release),
            )
        };
        let mut dst = [0; 4];

        assert_eq!(record.copy_into(&mut dst), Err(DriverError::PacketOverflow));
        assert_eq!(dst, [0; 4]);
        assert_eq!(RELEASES.load(core::sync::atomic::Ordering::SeqCst), 1);
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
            let mut ring = RxRing::<2>::new();
            let bytes = [0_u8; 8];
            ring.try_push(unsafe {
                RawRxRecord::from_raw(
                    bytes.as_ptr().cast_mut().cast(),
                    8,
                    core::ptr::null_mut(),
                    Some(count_release),
                )
            })
            .unwrap();
        }

        assert_eq!(RELEASES.load(core::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn owned_pool_rejects_oversize_packets() {
        let mut pool = OwnedRxPool::<1, 4>::new();
        let bytes = [0_u8; 8];

        assert!(matches!(
            pool.try_acquire_copy(&bytes),
            Err(DriverError::PayloadTooLong)
        ));
    }

    proptest! {
        #[test]
        fn ring_len_never_exceeds_capacity(ops in proptest::collection::vec(0u8..3, 1..32)) {
            let mut ring = RxRing::<4>::new();
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
