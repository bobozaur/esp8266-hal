//! Rust helpers for the ingress worker's recursive queue-backed mutex.

#![allow(non_snake_case)]

use core::ffi::c_void;

use super::queue;

// Queue type tags (must match queue.rs).
const QUEUE_TYPE_RECURSIVE_MUTEX: u8 = 4;

/// `xSemaphoreCreateRecursiveMutex` — create a recursive mutex.
pub(crate) unsafe fn xSemaphoreCreateRecursiveMutex() -> *mut c_void {
    unsafe { queue::xQueueCreateMutex(QUEUE_TYPE_RECURSIVE_MUTEX) }
}

/// `vSemaphoreDelete` — free a semaphore/mutex.
pub(crate) unsafe fn vSemaphoreDelete(sem: *mut c_void) {
    unsafe { queue::vQueueDelete(sem) }
}

/// `xSemaphoreTakeRecursive` — take a recursive mutex.
pub(crate) unsafe fn xSemaphoreTakeRecursive(sem: *mut c_void, timeout: u32) -> i32 {
    unsafe { queue::xQueueTakeMutexRecursive(sem, timeout) }
}

/// `xSemaphoreGiveRecursive` — give a recursive mutex.
pub(crate) unsafe fn xSemaphoreGiveRecursive(sem: *mut c_void) -> i32 {
    unsafe { queue::xQueueGiveMutexRecursive(sem) }
}
