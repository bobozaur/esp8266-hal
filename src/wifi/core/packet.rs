use super::DriverError;
use core::ffi::c_void;

pub type ReleaseFn = unsafe fn(*mut c_void, *mut c_void);

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

    pub fn try_acquire_copy(
        &mut self,
        src: *const u8,
        len: u16,
    ) -> Result<OwnedRxLease, DriverError> {
        if SLOTS == 0 {
            return Err(DriverError::PacketOverflow);
        }
        if len as usize > BYTES {
            return Err(DriverError::PayloadTooLong);
        }
        if len != 0 && src.is_null() {
            return Err(DriverError::BackendRejected);
        }

        let Some(slot) = self.slots.iter_mut().find(|slot| !slot.occupied) else {
            return Err(DriverError::PacketOverflow);
        };

        if len != 0 {
            unsafe { core::ptr::copy_nonoverlapping(src, slot.data.as_mut_ptr(), len as usize) };
        }

        slot.occupied = true;
        Ok(OwnedRxLease {
            buffer: slot.data.as_mut_ptr() as *mut c_void,
            len,
            slot: slot as *mut OwnedRxSlot<BYTES> as *mut c_void,
        })
    }
}

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

pub struct ReleaseToken {
    buffer: *mut c_void,
    eb: *mut c_void,
    release_fn: Option<ReleaseFn>,
}

impl ReleaseToken {
    pub const unsafe fn from_raw(
        buffer: *mut c_void,
        eb: *mut c_void,
        release_fn: Option<ReleaseFn>,
    ) -> Self {
        Self {
            buffer,
            eb,
            release_fn,
        }
    }

    pub unsafe fn fake() -> Self {
        Self::from_raw(core::ptr::null_mut(), core::ptr::null_mut(), None)
    }

    pub fn buffer(&self) -> *mut c_void {
        self.buffer
    }

    pub fn eb(&self) -> *mut c_void {
        self.eb
    }

    pub fn release(self) {
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

pub struct RawRxRecord {
    pub buffer: *mut c_void,
    pub len: u16,
    pub release: ReleaseToken,
}

impl RawRxRecord {
    pub const unsafe fn from_raw(
        buffer: *mut c_void,
        len: u16,
        eb: *mut c_void,
        release_fn: Option<ReleaseFn>,
    ) -> Self {
        Self {
            buffer,
            len,
            release: ReleaseToken::from_raw(buffer, eb, release_fn),
        }
    }

    pub unsafe fn fake(len: u16) -> Self {
        Self {
            buffer: core::ptr::null_mut(),
            len,
            release: ReleaseToken::fake(),
        }
    }

    pub fn copy_into(self, dst: &mut [u8]) -> usize {
        let len = self.len as usize;
        let copy_len = len.min(dst.len());
        if copy_len > 0 && !self.buffer.is_null() {
            unsafe {
                core::ptr::copy_nonoverlapping(self.buffer as *const u8, dst.as_mut_ptr(), copy_len)
            };
        }
        self.release.release();
        copy_len
    }
}

pub struct RxPushError {
    error: DriverError,
    record: RawRxRecord,
}

impl RxPushError {
    pub fn error(&self) -> DriverError {
        self.error.clone()
    }

    pub fn release(self) {
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

pub struct RxRing<const N: usize> {
    entries: [Option<RawRxRecord>; N],
    head: usize,
    tail: usize,
    len: usize,
    drops: u32,
}

impl<const N: usize> RxRing<N> {
    pub const fn new() -> Self {
        Self {
            entries: [const { None }; N],
            head: 0,
            tail: 0,
            len: 0,
            drops: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub const fn capacity(&self) -> usize {
        N
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn drops(&self) -> u32 {
        self.drops
    }

    pub fn try_push(&mut self, record: RawRxRecord) -> Result<(), RxPushError> {
        if N == 0 || self.len == N {
            self.drops = self.drops.wrapping_add(1);
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

#[cfg(test)]
mod tests {
    extern crate std;

    use self::std::format;
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn ring_counts_drops_when_full() {
        let mut ring = RxRing::<1>::new();
        assert!(ring.try_push(unsafe { RawRxRecord::fake(32) }).is_ok());
        assert!(ring.try_push(unsafe { RawRxRecord::fake(64) }).is_err());
        assert_eq!(ring.drops(), 1);
    }

    #[test]
    fn zero_capacity_ring_rejects_without_storing() {
        let mut ring = RxRing::<0>::new();
        let rejected = ring
            .try_push(unsafe { RawRxRecord::fake(32) })
            .expect_err("zero-capacity ring must reject");

        assert_eq!(rejected.error(), DriverError::PacketOverflow);
        assert_eq!(ring.len(), 0);
        assert_eq!(ring.drops(), 1);
        assert!(ring.pop().is_none());
    }

    #[test]
    fn owned_pool_reuses_slot_after_release() {
        let mut pool = OwnedRxPool::<1, 8>::new();
        let bytes = [1_u8, 2, 3, 4];

        let first = pool
            .try_acquire_copy(bytes.as_ptr(), bytes.len() as u16)
            .expect("first slot");
        assert!(pool
            .try_acquire_copy(bytes.as_ptr(), bytes.len() as u16)
            .is_err());

        unsafe { release_owned_rx_slot::<8>(first.buffer, first.slot) };

        let second = pool
            .try_acquire_copy(bytes.as_ptr(), bytes.len() as u16)
            .expect("reused slot");
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
        ring.try_push(unsafe {
            RawRxRecord::from_raw(
                core::ptr::null_mut(),
                8,
                core::ptr::null_mut(),
                Some(count_release),
            )
        })
        .unwrap();

        let mut dst = [0; 4];
        assert_eq!(ring.pop().unwrap().copy_into(&mut dst), 4);
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
            let _record = unsafe {
                RawRxRecord::from_raw(
                    core::ptr::null_mut(),
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
            ring.try_push(unsafe {
                RawRxRecord::from_raw(
                    core::ptr::null_mut(),
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
            pool.try_acquire_copy(bytes.as_ptr(), bytes.len() as u16),
            Err(DriverError::PayloadTooLong)
        ));
    }

    proptest! {
        #[test]
        fn ring_len_never_exceeds_capacity(ops in proptest::collection::vec(0u8..3, 1..32)) {
            let mut ring = RxRing::<4>::new();
            for op in ops {
                match op {
                    0 => { let _ = ring.try_push(unsafe { RawRxRecord::fake(64) }); }
                    _ => { let _ = ring.pop(); }
                }
                prop_assert!(ring.len() <= 4);
            }
        }
    }
}
