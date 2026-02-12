//! FreeRTOS-compatible semaphore/mutex wrappers.
//!
//! In FreeRTOS, semaphores and mutexes are implemented as queues.
//! The C macros expand to `xQueue*` calls. These Rust functions provide
//! the `xSemaphore*` symbols that the `__wifi_*` adapter and some C code
//! may call as real functions (not through macros).

use core::ffi::c_void;

use super::queue;

// Queue type tags (must match queue.rs).
const QUEUE_TYPE_MUTEX: u8 = 1;
const QUEUE_TYPE_RECURSIVE_MUTEX: u8 = 4;

/// `xSemaphoreCreateCounting` — create a counting semaphore.
#[no_mangle]
pub unsafe extern "C" fn xSemaphoreCreateCounting(max: u32, init: u32) -> *mut c_void {
    unsafe { queue::xQueueCreateCountingSemaphore(max, init) }
}

/// `xSemaphoreCreateBinary` — create a binary semaphore (max=1, init=0).
#[no_mangle]
pub unsafe extern "C" fn xSemaphoreCreateBinary() -> *mut c_void {
    unsafe { queue::xQueueCreateCountingSemaphore(1, 0) }
}

/// `xSemaphoreCreateMutex` — create a mutex.
#[no_mangle]
pub unsafe extern "C" fn xSemaphoreCreateMutex() -> *mut c_void {
    unsafe { queue::xQueueCreateMutex(QUEUE_TYPE_MUTEX) }
}

/// `xSemaphoreCreateRecursiveMutex` — create a recursive mutex.
#[no_mangle]
pub unsafe extern "C" fn xSemaphoreCreateRecursiveMutex() -> *mut c_void {
    unsafe { queue::xQueueCreateMutex(QUEUE_TYPE_RECURSIVE_MUTEX) }
}

/// `vSemaphoreDelete` — free a semaphore/mutex.
#[no_mangle]
pub unsafe extern "C" fn vSemaphoreDelete(sem: *mut c_void) {
    unsafe { queue::vQueueDelete(sem) }
}

/// `xSemaphoreTake` — take (decrement) a semaphore.
#[no_mangle]
pub unsafe extern "C" fn xSemaphoreTake(sem: *mut c_void, timeout: u32) -> i32 {
    unsafe { queue::xQueueSemaphoreTake(sem, timeout) }
}

/// `xSemaphoreGive` — give (increment) a semaphore.
///
/// For semaphores, this increments the count. For mutexes, this releases
/// the lock, clears ownership, and restores priority inheritance.
#[no_mangle]
pub unsafe extern "C" fn xSemaphoreGive(sem: *mut c_void) -> i32 {
    unsafe { queue::xQueueGiveMutexAware(sem) }
}

/// `xSemaphoreTakeRecursive` — take a recursive mutex.
#[no_mangle]
pub unsafe extern "C" fn xSemaphoreTakeRecursive(sem: *mut c_void, timeout: u32) -> i32 {
    unsafe { queue::xQueueTakeMutexRecursive(sem, timeout) }
}

/// `xSemaphoreGiveRecursive` — give a recursive mutex.
#[no_mangle]
pub unsafe extern "C" fn xSemaphoreGiveRecursive(sem: *mut c_void) -> i32 {
    unsafe { queue::xQueueGiveMutexRecursive(sem) }
}
