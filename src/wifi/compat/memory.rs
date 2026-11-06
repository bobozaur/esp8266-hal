//! SDK allocation ABI over the application's permanent DRAM allocator.
//!
//! C free/realloc omit allocation layouts, so SDK allocations carry a compact
//! header. Rust-owned queues use the same provider without this C ABI header.

// These exported names are fixed by the native SDK ABI, including host tests.
#![allow(non_snake_case)]

use core::alloc::Layout;
use core::ffi::c_void;
use core::ptr;

use crate::wifi_core::WifiAllocator;

static mut ALLOCATOR: Option<&'static dyn WifiAllocator> = None;

/// Installed exactly once before any SDK tasks or interrupts can allocate.
pub(crate) unsafe fn install(allocator: &'static dyn WifiAllocator) {
    unsafe { ALLOCATOR = Some(allocator) };
}

pub(crate) fn allocator() -> &'static dyn WifiAllocator {
    // SAFETY: the provider is permanent and never changes after installation.
    unsafe { ALLOCATOR.expect("runtime allocator is not initialized") }
}

unsafe fn alloc_raw(layout: Layout) -> *mut u8 {
    // Before runtime setup, SDK allocation fails without touching a heap.
    unsafe { ALLOCATOR.map_or(ptr::null_mut(), |allocator| allocator.alloc(layout)) }
}

unsafe fn dealloc_raw(pointer: *mut u8, layout: Layout) {
    // SAFETY: all live SDK allocations came from the installed provider.
    unsafe { allocator().dealloc(pointer, layout) };
}

fn total_free() -> usize {
    // SAFETY: installation precedes scheduler activation; the provider is Sync.
    unsafe { ALLOCATOR.map_or(0, |allocator| allocator.free_dram_bytes()) }
}

// The pinned ESP8266 SDK (vendor/VERSION: core 231e0e2, pp 25e5ef2)
// ignores allocation failures in its radio attach and config loading. Reserve
// eight persistent first-attach allocations and one temporary config buffer
// before entering it; handoff preserves the normal
// C header and allocator ownership. This is not a separate heap or free list.
#[derive(Clone, Copy)]
pub(crate) enum InitAllocation {
    Hardware,
    TxDescriptors,
    RxDescriptors,
    PacketDescriptors,
    PpQueue,
    PpQueueStorage,
    PpStack,
    MiscNvs,
    WifiNvs,
}

/// Dimensions copied by wifi_menuconfig_init into g_wifi_menuconfig.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct InitBufferLayout {
    pub ampdu_buffers: usize,
    pub ampdu_buffer_len: usize,
    pub max_packet_len: usize,
    pub rx_buffer_len: usize,
    pub rx_buffers: usize,
    pub rx_packets: usize,
    pub tx_buffers: usize,
    pub continuous_rx_buffers: usize,
}

impl InitBufferLayout {
    pub(crate) fn allocation_sizes(
        self,
        first_attach: bool,
        pp_queue_bytes: usize,
        pp_stack_bytes: usize,
    ) -> Option<[usize; INIT_ALLOCATION_COUNT]> {
        // wifi_nvs_deinit resets its flag, unlike the permanent radio attach.
        let mut sizes = [0; INIT_ALLOCATION_COUNT];
        sizes[InitAllocation::WifiNvs as usize] = 1024;
        if first_attach {
            // wdev.c:1151 combines RX data, AMPDU storage and 12-byte DMA
            // descriptors. esf_buf.c:720/734/743 allocate its three indexes.
            let descriptors = self
                .rx_buffers
                .checked_add(self.ampdu_buffers)?
                .checked_add(self.tx_buffers)?
                .checked_mul(12)?;
            sizes[InitAllocation::Hardware as usize] = self
                .ampdu_buffers
                .checked_mul(self.ampdu_buffer_len)?
                .checked_add(self.tx_buffers.checked_mul(4)?)?
                .checked_add(self.max_packet_len)?
                .checked_add(self.rx_buffers.checked_mul(self.rx_buffer_len)?)?
                .checked_add(descriptors)?;
            sizes[InitAllocation::TxDescriptors as usize] =
                self.tx_buffers.checked_add(8)?.checked_mul(28)?;
            sizes[InitAllocation::RxDescriptors as usize] =
                self.rx_packets.checked_add(1)?.checked_mul(8)?;
            sizes[InitAllocation::PacketDescriptors as usize] = self
                .tx_buffers
                .checked_add(self.rx_packets)?
                .checked_add(9)?
                .checked_mul(68)?;
            sizes[InitAllocation::PpQueue as usize] = pp_queue_bytes;
            sizes[InitAllocation::PpQueueStorage as usize] = 75 * 8;
            sizes[InitAllocation::PpStack as usize] = pp_stack_bytes;
            // misc_nvs_init ignores this allocation failure; STA later
            // dereferences g_misc_nvs. Unlike WiFi NVS, this object persists.
            sizes[InitAllocation::MiscNvs as usize] = 36;
        }
        Some(sizes)
    }
}

const INIT_ALLOCATION_COUNT: usize = 9;
static INIT_RESERVATIONS_ACTIVE: portable_atomic::AtomicBool =
    portable_atomic::AtomicBool::new(false);
static mut INIT_RESERVATIONS: [(*mut c_void, usize); INIT_ALLOCATION_COUNT] =
    [(ptr::null_mut(), 0); INIT_ALLOCATION_COUNT];

/// Called under the WiFi singleton guard, before any native initialization.
pub(crate) unsafe fn reserve_init_allocations(sizes: [usize; INIT_ALLOCATION_COUNT]) -> bool {
    unsafe {
        debug_assert!(!INIT_RESERVATIONS_ACTIVE.load(portable_atomic::Ordering::Relaxed));
        for (index, size) in sizes.into_iter().enumerate() {
            if size == 0 {
                continue;
            }
            let pointer = zalloc(size);
            if pointer.is_null() {
                release_init_allocations();
                return false;
            }
            INIT_RESERVATIONS[index] = (pointer, size);
        }
        INIT_RESERVATIONS_ACTIVE.store(true, portable_atomic::Ordering::Release);
        true
    }
}

/// An allocation becomes SDK-owned exactly once, at its audited call boundary.
pub(crate) fn take_init_allocation(kind: InitAllocation, size: usize) -> Option<*mut c_void> {
    if !INIT_RESERVATIONS_ACTIVE.load(portable_atomic::Ordering::Acquire) {
        return None;
    }
    critical_section::with(|_| unsafe {
        let slot = &mut INIT_RESERVATIONS[kind as usize];
        if slot.0.is_null() || slot.1 != size {
            return None;
        }
        let pointer = slot.0;
        *slot = (ptr::null_mut(), 0);
        Some(pointer)
    })
}

/// Only unconsumed reservations remain HAL-owned, even after failed cleanup.
#[allow(clippy::needless_range_loop)] // Avoid borrowing the whole mutable registry across free.
pub(crate) unsafe fn release_init_allocations() {
    unsafe {
        INIT_RESERVATIONS_ACTIVE.store(false, portable_atomic::Ordering::Release);
        for index in 0..INIT_ALLOCATION_COUNT {
            let pointer = INIT_RESERVATIONS[index].0;
            INIT_RESERVATIONS[index] = (ptr::null_mut(), 0);
            free(pointer);
        }
    }
}

// File/line identities are part of this narrow adaptation to the pinned blobs.
// Matching them as well as size/caps prevents unrelated init-time allocations
// from consuming a reserved buffer of coincidentally equal size.
unsafe fn init_allocation_site(
    size: usize,
    caps: u32,
    file: *const u8,
    line: usize,
    zeroed: bool,
) -> Option<*mut c_void> {
    if !INIT_RESERVATIONS_ACTIVE.load(portable_atomic::Ordering::Acquire) {
        return None;
    }
    let (kind, expected_file): (InitAllocation, &[u8]) = match (zeroed, caps, line) {
        (true, 4, 1151) => (InitAllocation::Hardware, b"wdev.c\0"),
        (true, 4, 720) => (InitAllocation::TxDescriptors, b"esf_buf.c\0"),
        (true, 4, 734) => (InitAllocation::RxDescriptors, b"esf_buf.c\0"),
        (true, 4, 743) => (InitAllocation::PacketDescriptors, b"esf_buf.c\0"),
        (false, 2, 46) => (InitAllocation::MiscNvs, b"misc_nvs.c\0"),
        (false, 4, 537) => (InitAllocation::WifiNvs, b"ieee80211_nvs.c\0"),
        _ => return None,
    };
    if file.is_null() {
        return None;
    }
    for (index, expected) in expected_file.iter().enumerate() {
        // SAFETY: native heap ABI supplies a readable NUL-terminated filename.
        if unsafe { file.add(index).read() } != *expected {
            return None;
        }
    }
    take_init_allocation(kind, size)
}

// ---------------------------------------------------------------------------
// Size-tracking wrapper
//
// GlobalAlloc::dealloc requires a Layout, but C code calls free(ptr) without
// a size. We store the allocation size in a four-byte header before the pointer.
// ---------------------------------------------------------------------------

const HEADER_SIZE: usize = core::mem::size_of::<u32>();

/// Return the usable size recorded by this C allocation boundary.
///
/// # Safety
/// `pointer` must be the exact, non-null base of a live allocation returned by
/// this module's C allocation functions. Its owner must keep it alive.
pub(crate) unsafe fn allocation_size(pointer: *const c_void) -> usize {
    // SAFETY: the live allocation's immutable header precedes its user bytes.
    unsafe { pointer.cast::<u8>().sub(HEADER_SIZE).cast::<u32>().read() as usize - HEADER_SIZE }
}

fn c_malloc(size: usize) -> *mut c_void {
    if size == 0 {
        return ptr::null_mut();
    }
    let Some(total) = size.checked_add(HEADER_SIZE) else {
        return ptr::null_mut();
    };
    let Ok(total_word) = u32::try_from(total) else {
        return ptr::null_mut();
    };
    critical_section::with(|_| unsafe {
        let layout = match Layout::from_size_align(total, 4) {
            Ok(l) => l,
            Err(_) => return ptr::null_mut(),
        };
        let raw = alloc_raw(layout);
        if raw.is_null() {
            return ptr::null_mut();
        }
        let header = raw as *mut u32;
        header.write(total_word);
        raw.add(HEADER_SIZE).cast()
    })
}

fn c_free(ptr: *mut c_void) {
    if ptr.is_null() {
        return;
    }
    critical_section::with(|_| unsafe {
        let raw = ptr.cast::<u8>().sub(HEADER_SIZE);
        let total = raw.cast::<u32>().read() as usize;
        let layout = Layout::from_size_align_unchecked(total, 4);
        dealloc_raw(raw, layout);
    })
}

fn c_realloc(ptr: *mut c_void, new_size: usize) -> *mut c_void {
    if ptr.is_null() {
        return c_malloc(new_size);
    }
    if new_size == 0 {
        c_free(ptr);
        return ptr::null_mut();
    }
    // SAFETY: realloc's caller owns this exact live C allocation.
    let old_usable = unsafe { allocation_size(ptr) };
    if new_size <= old_usable {
        return ptr;
    }
    let new_ptr = c_malloc(new_size);
    if !new_ptr.is_null() {
        unsafe {
            ptr::copy_nonoverlapping(
                ptr as *const u8,
                new_ptr as *mut u8,
                old_usable.min(new_size),
            );
        }
        c_free(ptr);
    }
    new_ptr
}

// ---------------------------------------------------------------------------
// Native allocation ABI and compat-layer callers share one allocation owner.
// ---------------------------------------------------------------------------

#[cfg_attr(target_arch = "xtensa", no_mangle)]
pub unsafe extern "C" fn malloc(size: usize) -> *mut c_void {
    c_malloc(size)
}
#[cfg_attr(target_arch = "xtensa", no_mangle)]
pub unsafe extern "C" fn free(ptr: *mut c_void) {
    c_free(ptr)
}

#[cfg_attr(target_arch = "xtensa", no_mangle)]
pub unsafe extern "C" fn realloc(ptr: *mut c_void, size: usize) -> *mut c_void {
    c_realloc(ptr, size)
}

#[cfg_attr(target_arch = "xtensa", no_mangle)]
pub unsafe extern "C" fn calloc(count: usize, size: usize) -> *mut c_void {
    let Some(total) = count.checked_mul(size) else {
        return ptr::null_mut();
    };
    let p = c_malloc(total);
    if !p.is_null() {
        unsafe { ptr::write_bytes(p as *mut u8, 0, total) };
    }
    p
}

#[cfg_attr(target_arch = "xtensa", no_mangle)]
pub unsafe extern "C" fn zalloc(size: usize) -> *mut c_void {
    let p = c_malloc(size);
    if !p.is_null() {
        unsafe { ptr::write_bytes(p as *mut u8, 0, size) };
    }
    p
}

// Newlib's reentrant entrypoints must use the same header and provider as
// radio allocations. The retained formatting code cannot select its own heap.
#[cfg_attr(target_arch = "xtensa", no_mangle)]
pub unsafe extern "C" fn _malloc_r(_reent: *mut c_void, size: usize) -> *mut c_void {
    c_malloc(size)
}

#[cfg_attr(target_arch = "xtensa", no_mangle)]
pub unsafe extern "C" fn _calloc_r(_reent: *mut c_void, count: usize, size: usize) -> *mut c_void {
    unsafe { calloc(count, size) }
}

#[cfg_attr(target_arch = "xtensa", no_mangle)]
pub unsafe extern "C" fn _realloc_r(
    _reent: *mut c_void,
    pointer: *mut c_void,
    size: usize,
) -> *mut c_void {
    c_realloc(pointer, size)
}

#[cfg_attr(target_arch = "xtensa", no_mangle)]
pub unsafe extern "C" fn _free_r(_reent: *mut c_void, pointer: *mut c_void) {
    c_free(pointer)
}

// ---------------------------------------------------------------------------
// FreeRTOS-compatible symbols
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// heap_caps_* symbols (SDK C code)
// ---------------------------------------------------------------------------

#[cfg_attr(target_arch = "xtensa", no_mangle)]
pub unsafe extern "C" fn _heap_caps_malloc(
    size: usize,
    caps: u32,
    file: *const u8,
    line: usize,
) -> *mut c_void {
    unsafe { init_allocation_site(size, caps, file, line, false) }.unwrap_or_else(|| c_malloc(size))
}

#[cfg_attr(target_arch = "xtensa", no_mangle)]
pub unsafe extern "C" fn _heap_caps_free(ptr: *mut c_void, _file: *const u8, _line: usize) {
    c_free(ptr)
}

#[cfg_attr(target_arch = "xtensa", no_mangle)]
pub unsafe extern "C" fn _heap_caps_zalloc(
    size: usize,
    caps: u32,
    file: *const u8,
    line: usize,
) -> *mut c_void {
    unsafe { init_allocation_site(size, caps, file, line, true) }
        .unwrap_or_else(|| unsafe { zalloc(size) })
}

#[cfg_attr(target_arch = "xtensa", no_mangle)]
pub unsafe extern "C" fn _heap_caps_realloc(
    ptr: *mut c_void,
    size: usize,
    _caps: u32,
    _file: *const u8,
    _line: usize,
) -> *mut c_void {
    c_realloc(ptr, size)
}

#[cfg_attr(target_arch = "xtensa", no_mangle)]
pub unsafe extern "C" fn heap_caps_get_dram_free_size() -> u32 {
    total_free() as u32
}

#[cfg_attr(target_arch = "xtensa", no_mangle)]
pub unsafe extern "C" fn esp_get_free_heap_size() -> u32 {
    total_free() as u32
}

#[cfg(all(test, not(target_arch = "xtensa")))]
pub(crate) mod tests {
    extern crate std;

    use core::alloc::{GlobalAlloc, Layout};
    use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::alloc::System;
    use std::sync::{Mutex, MutexGuard, Once};

    use super::*;

    struct TestAllocator {
        fail: AtomicBool,
        live: AtomicUsize,
        budget: AtomicUsize,
    }

    unsafe impl GlobalAlloc for TestAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if self.fail.load(Ordering::Relaxed) {
                return ptr::null_mut();
            }
            let budget = self.budget.load(Ordering::Relaxed);
            if budget == 0 {
                return ptr::null_mut();
            }
            if budget != usize::MAX {
                self.budget.fetch_sub(1, Ordering::Relaxed);
            }
            // SAFETY: forward the requested layout to the host allocator.
            let pointer = unsafe { System.alloc(layout) };
            if !pointer.is_null() {
                self.live.fetch_add(1, Ordering::Relaxed);
            }
            pointer
        }

        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            self.live.fetch_sub(1, Ordering::Relaxed);
            // SAFETY: the bridge must return the original pointer and layout.
            unsafe { System.dealloc(pointer, layout) };
        }
    }

    // SAFETY: this models the provider's byte-addressable RAM and synchronized
    // allocation contract on the host; target DRAM placement is hardware-tested.
    unsafe impl WifiAllocator for TestAllocator {
        fn free_dram_bytes(&self) -> usize {
            1234
        }
    }

    static PROVIDER: TestAllocator = TestAllocator {
        fail: AtomicBool::new(false),
        live: AtomicUsize::new(0),
        budget: AtomicUsize::new(usize::MAX),
    };

    /// Serialize tests that use the native allocator's permanent registry.
    pub(crate) fn allocator_guard() -> MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        static INSTALL: Once = Once::new();
        let guard = LOCK.lock().unwrap();
        INSTALL.call_once(|| unsafe { install(&PROVIDER) });
        guard
    }

    #[test]
    fn native_buffer_dimensions_match_pinned_sdk_layout_and_reinit_lifetime() {
        let mut layout = InitBufferLayout {
            ampdu_buffers: 5,
            ampdu_buffer_len: 256,
            max_packet_len: 1076,
            rx_buffer_len: 524,
            rx_buffers: 14,
            rx_packets: 4,
            tx_buffers: 4,
            continuous_rx_buffers: 14,
        };
        assert_eq!(
            layout.allocation_sizes(true, 168, 2992),
            Some([9984, 336, 40, 1156, 168, 600, 2992, 36, 1024])
        );
        assert_eq!(
            layout.allocation_sizes(false, 168, 2992),
            Some([0, 0, 0, 0, 0, 0, 0, 0, 1024])
        );
        layout.rx_buffer_len = usize::MAX;
        assert!(layout.allocation_sizes(true, 168, 2992).is_none());
    }

    #[test]
    fn c_abi_preserves_allocation_on_realloc_failure_and_releases_its_layout() {
        let _guard = allocator_guard();
        assert!(c_malloc(0).is_null());
        assert!(c_malloc(usize::MAX).is_null());
        assert!(unsafe { calloc(usize::MAX, 2) }.is_null());
        assert_eq!(total_free(), 1234);

        let pointer = unsafe { calloc(4, 16) }.cast::<u8>();
        assert!(!pointer.is_null());
        assert_eq!(pointer as usize % 4, 0);
        unsafe {
            assert!(core::slice::from_raw_parts(pointer, 64)
                .iter()
                .all(|byte| *byte == 0));
            pointer.write_bytes(0x5a, 64);
        }
        PROVIDER.fail.store(true, Ordering::Relaxed);
        assert!(c_realloc(pointer.cast(), 128).is_null());
        unsafe {
            assert!(core::slice::from_raw_parts(pointer, 64)
                .iter()
                .all(|byte| *byte == 0x5a));
        }
        assert_eq!(PROVIDER.live.load(Ordering::Relaxed), 1);

        PROVIDER.fail.store(false, Ordering::Relaxed);
        let grown = c_realloc(pointer.cast(), 128).cast::<u8>();
        assert!(!grown.is_null());
        unsafe {
            assert!(core::slice::from_raw_parts(grown, 64)
                .iter()
                .all(|byte| *byte == 0x5a));
        }
        assert!(c_realloc(grown.cast(), 0).is_null());
        assert_eq!(PROVIDER.live.load(Ordering::Relaxed), 0);
        c_free(ptr::null_mut());

        // Native and reentrant entrypoints can exchange ownership; neither
        // introduces another heap or a different allocation header.
        let pointer = unsafe { malloc(4) }.cast::<u32>();
        assert!(!pointer.is_null());
        unsafe { pointer.write(0x1234_5678) };
        let pointer = unsafe { _realloc_r(ptr::null_mut(), pointer.cast(), 64) }.cast::<u32>();
        assert!(!pointer.is_null());
        assert_eq!(unsafe { pointer.read() }, 0x1234_5678);
        unsafe { _free_r(ptr::null_mut(), pointer.cast()) };
        let pointer = unsafe { _calloc_r(ptr::null_mut(), 4, 4) }.cast::<u32>();
        assert!(!pointer.is_null());
        assert_eq!(unsafe { pointer.read() }, 0);
        unsafe { free(pointer.cast()) };
        assert_eq!(PROVIDER.live.load(Ordering::Relaxed), 0);

        // A failed reservation at every allocation boundary returns every
        // earlier allocation, before native code can observe any pointer.
        let sizes = [64, 32, 16, 128, 168, 600, 2992, 36, 1024];
        for failure_at in 0..INIT_ALLOCATION_COUNT {
            PROVIDER.budget.store(failure_at, Ordering::Relaxed);
            assert!(!unsafe { reserve_init_allocations(sizes) });
            assert_eq!(PROVIDER.live.load(Ordering::Relaxed), 0);
        }
        PROVIDER.budget.store(usize::MAX, Ordering::Relaxed);
        assert!(unsafe { reserve_init_allocations(sizes) });
        assert_eq!(PROVIDER.live.load(Ordering::Relaxed), INIT_ALLOCATION_COUNT);
        // Wrong file/caps/size cannot steal a reservation.
        assert!(
            unsafe { init_allocation_site(64, 4, c"other.c".as_ptr().cast(), 1151, true) }
                .is_none()
        );
        assert!(
            unsafe { init_allocation_site(64, 2, c"wdev.c".as_ptr().cast(), 1151, true) }.is_none()
        );
        assert!(take_init_allocation(InitAllocation::Hardware, 63).is_none());
        let handed_off = unsafe { _heap_caps_zalloc(64, 4, c"wdev.c".as_ptr().cast(), 1151) };
        assert!(!handed_off.is_null());
        assert!(take_init_allocation(InitAllocation::Hardware, 64).is_none());
        assert_eq!(PROVIDER.live.load(Ordering::Relaxed), INIT_ALLOCATION_COUNT);
        // Native ownership survives releasing the unconsumed reservations.
        unsafe { release_init_allocations() };
        assert_eq!(PROVIDER.live.load(Ordering::Relaxed), 1);
        c_free(handed_off);
        assert_eq!(PROVIDER.live.load(Ordering::Relaxed), 0);
        // Reinitialization reserves only the temporary NVS buffer.
        assert!(unsafe { reserve_init_allocations([0, 0, 0, 0, 0, 0, 0, 0, 1024]) });
        let temporary =
            unsafe { _heap_caps_malloc(1024, 4, c"ieee80211_nvs.c".as_ptr().cast(), 537) };
        unsafe { release_init_allocations() };
        assert_eq!(PROVIDER.live.load(Ordering::Relaxed), 1);
        c_free(temporary);
        assert_eq!(PROVIDER.live.load(Ordering::Relaxed), 0);
    }
}
