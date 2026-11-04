#[cfg(test)]
extern crate std;
#[cfg(any(target_arch = "xtensa", test))]
use allocator_api2::alloc::{AllocError, Allocator};
use core::alloc::GlobalAlloc;
#[cfg(any(target_arch = "xtensa", test))]
use core::{alloc::Layout, ptr::NonNull};

/// Application allocator shared by Rust Wi-Fi state and the native SDK.
///
/// # Safety
/// The provider must be initialized before runtime initialization and remain
/// alive for the boot. Every allocation must be byte-addressable DRAM suitable
/// for native SDK access, with the requested layout. Allocation, deallocation,
/// and statistics must support SDK task preemption and interrupt contexts,
/// including Wi-Fi NMI allocation paths. The HAL's `critical-section`
/// implementation excludes both ordinary interrupts and Wi-Fi NMI; protected
/// allocator operations must remain bounded and must not wait for interrupts.
/// Exhaustion returns null; statistics must not allocate and must truthfully
/// report available DRAM. The allocator must not reclaim live application stacks.
pub unsafe trait WifiAllocator: GlobalAlloc + Sync {
    fn free_dram_bytes(&self) -> usize;
}

#[cfg(any(target_arch = "xtensa", test))]
#[derive(Clone, Copy)]
pub(crate) struct RuntimeAllocator(&'static dyn WifiAllocator);

#[cfg(any(target_arch = "xtensa", test))]
impl RuntimeAllocator {
    pub(crate) const fn new(provider: &'static dyn WifiAllocator) -> Self {
        Self(provider)
    }
}

// SAFETY: WifiAllocator establishes the layout, memory-kind and lifetime
// guarantees. Every copy refers to the same permanent provider.
#[cfg(any(target_arch = "xtensa", test))]
unsafe impl Allocator for RuntimeAllocator {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        if layout.size() == 0 {
            return Ok(NonNull::slice_from_raw_parts(
                NonNull::new(layout.align() as *mut u8).unwrap(),
                0,
            ));
        }
        let ptr = NonNull::new(unsafe { self.0.alloc(layout) }).ok_or(AllocError)?;
        Ok(NonNull::slice_from_raw_parts(ptr, layout.size()))
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        if layout.size() != 0 {
            unsafe { self.0.dealloc(ptr.as_ptr(), layout) };
        }
    }
}

#[cfg(test)]
pub(crate) fn test_allocator() -> RuntimeAllocator {
    use std::alloc::System;
    struct TestAllocator;
    unsafe impl GlobalAlloc for TestAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            unsafe { System.alloc(layout) }
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            unsafe { System.dealloc(ptr, layout) }
        }
    }
    unsafe impl WifiAllocator for TestAllocator {
        fn free_dram_bytes(&self) -> usize {
            usize::MAX
        }
    }
    static PROVIDER: TestAllocator = TestAllocator;
    RuntimeAllocator::new(&PROVIDER)
}

#[cfg(test)]
pub(crate) struct TrackingAllocator {
    pub(crate) live: core::sync::atomic::AtomicUsize,
    pub(crate) live_bytes: core::sync::atomic::AtomicUsize,
    budget: core::sync::atomic::AtomicIsize,
}
#[cfg(test)]
impl TrackingAllocator {
    pub(crate) fn new(budget: isize) -> (&'static Self, RuntimeAllocator) {
        let provider = std::boxed::Box::leak(std::boxed::Box::new(Self {
            live: core::sync::atomic::AtomicUsize::new(0),
            live_bytes: core::sync::atomic::AtomicUsize::new(0),
            budget: core::sync::atomic::AtomicIsize::new(budget),
        }));
        (provider, RuntimeAllocator::new(provider))
    }
}
#[cfg(test)]
unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        use core::sync::atomic::Ordering;
        if self.budget.fetch_sub(1, Ordering::SeqCst) <= 0 {
            return core::ptr::null_mut();
        }
        let ptr = unsafe { std::alloc::System.alloc(layout) };
        if !ptr.is_null() {
            self.live.fetch_add(1, Ordering::SeqCst);
            self.live_bytes.fetch_add(layout.size(), Ordering::SeqCst);
        }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.live.fetch_sub(1, core::sync::atomic::Ordering::SeqCst);
        self.live_bytes
            .fetch_sub(layout.size(), core::sync::atomic::Ordering::SeqCst);
        unsafe { std::alloc::System.dealloc(ptr, layout) }
    }
}
#[cfg(test)]
unsafe impl WifiAllocator for TrackingAllocator {
    fn free_dram_bytes(&self) -> usize {
        usize::MAX
    }
}
