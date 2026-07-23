//! FreeRTOS-compatible queue, semaphore, and mutex implementation.
//!
//! In FreeRTOS, semaphores and mutexes are implemented as special queues.
//! The C macros `xSemaphoreTake`, `xSemaphoreCreateMutex`, etc. expand to
//! `xQueueSemaphoreTake`, `xQueueCreateMutex`, etc. We provide all of these
//! symbols from a single unified queue implementation.
//!
//! Tasks that block on a full/empty queue enter the Blocked state and are
//! woken directly when space becomes available or an item arrives, rather
//! than busy-waiting.

use core::ffi::c_void;

use super::critical;
use super::memory;
use super::task;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const PD_PASS: i32 = 1;
const PD_FALSE: i32 = 0;

/// `queueSEND_TO_BACK`
const QUEUE_SEND_TO_BACK: i32 = 0;
/// `queueSEND_TO_FRONT`
const QUEUE_SEND_TO_FRONT: i32 = 1;
/// `queueOVERWRITE`
const QUEUE_OVERWRITE: i32 = 2;

/// Maximum number of tasks that can block on a single queue.
const MAX_WAITERS: usize = 8;

/// Queue type tags (matching FreeRTOS queueQUEUE_TYPE_*).
const QUEUE_TYPE_BASE: u8 = 0;
const QUEUE_TYPE_MUTEX: u8 = 1;
const QUEUE_TYPE_COUNTING_SEMAPHORE: u8 = 2;
#[allow(dead_code)]
const QUEUE_TYPE_BINARY_SEMAPHORE: u8 = 3;
const QUEUE_TYPE_RECURSIVE_MUTEX: u8 = 4;
const QUEUE_UNLOCKED: i8 = -1;
const QUEUE_LOCKED_UNMODIFIED: i8 = 0;

// ---------------------------------------------------------------------------
// Queue data structure
// ---------------------------------------------------------------------------

/// A waiter entry: task index waiting to send or receive.
#[derive(Clone, Copy)]
struct Waiter {
    task_idx: usize,
    active: bool,
}

/// Wait list for tasks blocked on a queue operation.
struct WaitList {
    entries: [Waiter; MAX_WAITERS],
}

impl WaitList {
    const fn new() -> Self {
        Self {
            entries: [Waiter {
                task_idx: 0,
                active: false,
            }; MAX_WAITERS],
        }
    }

    /// Add a task to the wait list.
    #[inline(never)]
    fn add(&mut self, task_idx: usize) {
        let mut free_slot = MAX_WAITERS;
        let mut slot = 0usize;

        while slot < MAX_WAITERS {
            let entry = &mut self.entries[slot];
            if entry.active {
                if entry.task_idx == task_idx {
                    return;
                }
            } else if free_slot == MAX_WAITERS {
                free_slot = slot;
            }

            slot += 1;
        }

        if free_slot < MAX_WAITERS {
            self.entries[free_slot].task_idx = task_idx;
            self.entries[free_slot].active = true;
        }
        // If full, the waiter is silently dropped (shouldn't happen with MAX_WAITERS=8).
    }

    /// Remove a task from the wait list.
    #[inline(never)]
    fn remove(&mut self, task_idx: usize) {
        let mut slot = 0usize;
        while slot < MAX_WAITERS {
            let entry = &mut self.entries[slot];
            if entry.active && entry.task_idx == task_idx {
                entry.active = false;
                return;
            }
            slot += 1;
        }
    }

    /// Wake the highest-priority waiting task. Returns true if the woken task
    /// has higher priority than the current task (needs context switch).
    #[inline(never)]
    unsafe fn wake_highest_priority(&mut self) -> bool {
        unsafe {
            let mut best: Option<usize> = None;
            let mut best_prio: u32 = 0;
            let mut best_slot: usize = 0;
            let mut slot = 0usize;

            while slot < MAX_WAITERS {
                let entry = &mut self.entries[slot];
                if entry.active {
                    let prio = task::task_priority(entry.task_idx);
                    if best.is_none() || prio > best_prio {
                        best = Some(entry.task_idx);
                        best_prio = prio;
                        best_slot = slot;
                    }
                }
                slot += 1;
            }

            if let Some(task_idx) = best {
                self.entries[best_slot].active = false;
                task::wake_task(task_idx)
            } else {
                false
            }
        }
    }
}

#[repr(C)]
struct RawQueue {
    /// Heap-allocated ring buffer.
    storage: *mut u8,
    /// Size of each item in bytes.
    item_size: usize,
    /// Maximum number of items.
    capacity: usize,
    /// Read index.
    head: usize,
    /// Write index.
    tail: usize,
    /// Current number of items.
    count: usize,
    /// Queue type (base, mutex, semaphore, etc.).
    queue_type: u8,
    /// Tasks waiting to receive (queue empty).
    rx_waiters: WaitList,
    /// Tasks waiting to send (queue full).
    tx_waiters: WaitList,
    /// For mutexes: task index of the holder.
    mutex_holder: usize,
    /// For recursive mutexes: nesting count.
    recursive_count: u32,
    /// Deferred receive wake count while the queue is locked.
    tx_lock: i8,
    /// Deferred send wake count while the queue is locked.
    rx_lock: i8,
}

impl RawQueue {
    #[inline]
    fn offset(&self, index: usize) -> usize {
        index * self.item_size
    }

    #[inline]
    unsafe fn write_slot(&mut self, slot: usize, item: *const u8) {
        if self.item_size > 0 {
            unsafe {
                let dst = self.storage.add(self.offset(slot));
                core::ptr::copy_nonoverlapping(item, dst, self.item_size);
            }
        }
    }

    #[inline]
    unsafe fn read_slot(&self, slot: usize, buf: *mut u8) {
        if self.item_size > 0 && !buf.is_null() {
            unsafe {
                let src = self.storage.add(self.offset(slot));
                core::ptr::copy_nonoverlapping(src, buf, self.item_size);
            }
        }
    }

    unsafe fn send_back(&mut self, item: *const u8) -> bool {
        unsafe {
            if self.count >= self.capacity {
                return false;
            }
            self.write_slot(self.tail, item);
            self.tail = (self.tail + 1) % self.capacity;
            self.count += 1;
            true
        }
    }

    unsafe fn send_front(&mut self, item: *const u8) -> bool {
        unsafe {
            if self.count >= self.capacity {
                return false;
            }
            self.head = if self.head == 0 {
                self.capacity - 1
            } else {
                self.head - 1
            };
            self.write_slot(self.head, item);
            self.count += 1;
            true
        }
    }

    unsafe fn overwrite(&mut self, item: *const u8) {
        unsafe {
            if self.count >= self.capacity {
                let slot = if self.tail == 0 {
                    self.capacity - 1
                } else {
                    self.tail - 1
                };
                self.write_slot(slot, item);
            } else {
                self.send_back(item);
            }
        }
    }

    unsafe fn receive(&mut self, buf: *mut u8) -> bool {
        unsafe {
            if self.count == 0 {
                return false;
            }
            self.read_slot(self.head, buf);
            self.head = (self.head + 1) % self.capacity;
            self.count -= 1;
            true
        }
    }

    unsafe fn peek(&self, buf: *mut u8) -> bool {
        if self.count == 0 {
            return false;
        }
        unsafe {
            self.read_slot(self.head, buf);
        }
        true
    }
}

// ---------------------------------------------------------------------------
// Queue creation
// ---------------------------------------------------------------------------

/// Internal: allocate a queue with given type.
unsafe fn create_queue(cap: usize, item_size: usize, qtype: u8) -> *mut c_void {
    unsafe {
        let q = memory::malloc(core::mem::size_of::<RawQueue>()) as *mut RawQueue;
        if q.is_null() {
            return core::ptr::null_mut();
        }

        let storage = if item_size > 0 && cap > 0 {
            let buf = memory::malloc(cap * item_size) as *mut u8;
            if buf.is_null() {
                memory::free(q as *mut c_void);
                return core::ptr::null_mut();
            }
            buf
        } else {
            core::ptr::null_mut()
        };

        (*q) = RawQueue {
            storage,
            item_size,
            capacity: cap,
            head: 0,
            tail: 0,
            count: 0,
            queue_type: qtype,
            rx_waiters: WaitList::new(),
            tx_waiters: WaitList::new(),
            mutex_holder: usize::MAX,
            recursive_count: 0,
            tx_lock: QUEUE_UNLOCKED,
            rx_lock: QUEUE_UNLOCKED,
        };

        q as *mut c_void
    }
}

/// `xQueueCreate` — create a data queue.
#[no_mangle]
pub unsafe extern "C" fn xQueueCreate(queue_len: u32, item_size: u32) -> *mut c_void {
    unsafe {
        if queue_len == 8 {
            crate::logger::write_byte(b'L');
        }
        let q = create_queue(queue_len as usize, item_size as usize, QUEUE_TYPE_BASE);
        if queue_len == 8 {
            crate::logger::write_byte(if q.is_null() { b'!' } else { b'l' });
        }
        q
    }
}

/// `xQueueGenericCreate` — create a queue with type tag.
#[no_mangle]
pub unsafe extern "C" fn xQueueGenericCreate(
    queue_len: u32,
    item_size: u32,
    queue_type: u8,
) -> *mut c_void {
    unsafe { create_queue(queue_len as usize, item_size as usize, queue_type) }
}

/// `xQueueCreateMutex` — create a mutex (queue of length 1, item_size 0).
#[no_mangle]
pub unsafe extern "C" fn xQueueCreateMutex(mutex_type: u8) -> *mut c_void {
    unsafe {
        if mutex_type == QUEUE_TYPE_RECURSIVE_MUTEX {
            crate::logger::write_byte(b'U');
        }
        let q = create_queue(1, 0, mutex_type);
        if !q.is_null() {
            // Mutex starts "given" (count=1 = available).
            let rq = &mut *(q as *mut RawQueue);
            rq.count = 1;
            rq.mutex_holder = usize::MAX;
        }
        if mutex_type == QUEUE_TYPE_RECURSIVE_MUTEX {
            crate::logger::write_byte(if q.is_null() { b'!' } else { b'u' });
        }
        q
    }
}

/// `xQueueCreateCountingSemaphore` — create a counting semaphore.
#[no_mangle]
pub unsafe extern "C" fn xQueueCreateCountingSemaphore(max: u32, init: u32) -> *mut c_void {
    unsafe {
        let q = create_queue(max as usize, 0, QUEUE_TYPE_COUNTING_SEMAPHORE);
        if !q.is_null() {
            let rq = &mut *(q as *mut RawQueue);
            rq.count = init as usize;
        }
        q
    }
}

/// `vQueueDelete` — delete a queue and free its memory.
#[no_mangle]
pub unsafe extern "C" fn vQueueDelete(queue: *mut c_void) {
    unsafe {
        if queue.is_null() {
            return;
        }
        let q = queue as *mut RawQueue;
        if !(*q).storage.is_null() {
            memory::free((*q).storage as *mut c_void);
        }
        memory::free(queue);
    }
}

// ---------------------------------------------------------------------------
// Send
// ---------------------------------------------------------------------------

/// `xQueueGenericSend` — send an item to a queue with blocking.
#[no_mangle]
pub unsafe extern "C" fn xQueueGenericSend(
    queue: *mut c_void,
    item: *const c_void,
    timeout: u32,
    pos: i32,
) -> i32 {
    unsafe {
        if queue.is_null() {
            return PD_FALSE;
        }
        let q = queue as *mut RawQueue;
        let task_idx = task::current_task_index();
        let mut entry_time_set = false;
        let mut ticks_to_wait = timeout;
        let mut timeout_state = task::new_timeout_state();

        loop {
            critical::vPortEnterCritical();
            let ok = try_send(q, item as *const u8, pos);
            if ok {
                (*q).tx_waiters.remove(task_idx);
                let need_switch = notify_queue_posted(q);
                critical::vPortExitCritical();
                if need_switch {
                    super::port::PendSV(1);
                }
                return PD_PASS;
            }

            if ticks_to_wait == 0 {
                critical::vPortExitCritical();
                return PD_FALSE;
            }

            if !entry_time_set {
                task::set_timeout_state(&mut timeout_state);
                entry_time_set = true;
            }
            critical::vPortExitCritical();

            task::vTaskSuspendAll();
            lock_queue(q);
            let timed_out = task::check_for_timeout(&mut timeout_state, &mut ticks_to_wait);
            if !timed_out {
                critical::vPortEnterCritical();
                let full = (*q).count >= (*q).capacity && pos != QUEUE_OVERWRITE;
                critical::vPortExitCritical();

                if full {
                    (*q).tx_waiters.add(task_idx);
                    task::block_current_task_on_queue_tx(queue, deadline_from_ticks(ticks_to_wait));

                    unlock_queue(q);
                    resume_all_and_maybe_yield();
                } else {
                    unlock_queue(q);
                    let _ = task::xTaskResumeAll();
                }
            } else {
                (*q).tx_waiters.remove(task_idx);
                unlock_queue(q);
                let _ = task::xTaskResumeAll();
                critical::vPortEnterCritical();
                let full = (*q).count >= (*q).capacity && pos != QUEUE_OVERWRITE;
                critical::vPortExitCritical();
                if full {
                    return PD_FALSE;
                }
            }
        }
    }
}

/// `xQueueGenericSendFromISR` — send from ISR context (never blocks).
#[no_mangle]
pub unsafe extern "C" fn xQueueGenericSendFromISR(
    queue: *mut c_void,
    item: *const c_void,
    higher_priority_task_woken: *mut i32,
    pos: i32,
) -> i32 {
    unsafe {
        if queue.is_null() {
            return PD_FALSE;
        }
        let q = queue as *mut RawQueue;
        let ok = try_send(q, item as *const u8, pos);
        if ok {
            let need_switch = notify_queue_posted(q);
            if !higher_priority_task_woken.is_null() {
                *higher_priority_task_woken = if need_switch { PD_PASS } else { PD_FALSE };
            }
            PD_PASS
        } else {
            PD_FALSE
        }
    }
}

/// `xQueueSendFromISR` — send to back from ISR (legacy API).
#[no_mangle]
pub unsafe extern "C" fn xQueueSendFromISR(
    queue: *mut c_void,
    item: *const c_void,
    higher_priority_task_woken: *mut i32,
) -> i32 {
    unsafe { xQueueGenericSendFromISR(queue, item, higher_priority_task_woken, QUEUE_SEND_TO_BACK) }
}

/// `xQueueGiveFromISR` — give a semaphore from ISR.
#[no_mangle]
pub unsafe extern "C" fn xQueueGiveFromISR(
    queue: *mut c_void,
    higher_priority_task_woken: *mut i32,
) -> i32 {
    unsafe {
        if queue.is_null() {
            return PD_FALSE;
        }
        let q = queue as *mut RawQueue;
        if (*q).count < (*q).capacity {
            (*q).count += 1;
            let need_switch = notify_queue_posted(q);
            if !higher_priority_task_woken.is_null() {
                *higher_priority_task_woken = if need_switch { PD_PASS } else { PD_FALSE };
            }
            PD_PASS
        } else {
            PD_FALSE
        }
    }
}

// ---------------------------------------------------------------------------
// Receive
// ---------------------------------------------------------------------------

/// `xQueueReceive` — receive an item from a queue.
#[no_mangle]
pub unsafe extern "C" fn xQueueReceive(
    queue: *mut c_void,
    buffer: *mut c_void,
    timeout: u32,
) -> i32 {
    unsafe { queue_receive_impl(queue, buffer, timeout, false) }
}

/// `xQueuePeek` — inspect the next item without consuming it.
#[no_mangle]
pub unsafe extern "C" fn xQueuePeek(queue: *mut c_void, buffer: *mut c_void, timeout: u32) -> i32 {
    unsafe { queue_receive_impl(queue, buffer, timeout, true) }
}

/// `xQueueGenericReceive` — ESP8266 port wrapper.
#[no_mangle]
pub unsafe extern "C" fn xQueueGenericReceive(
    queue: *mut c_void,
    buffer: *mut c_void,
    timeout: u32,
    just_peeking: i32,
) -> i32 {
    unsafe {
        if just_peeking == 0 {
            xQueueReceive(queue, buffer, timeout)
        } else {
            xQueuePeek(queue, buffer, timeout)
        }
    }
}

#[inline(never)]
unsafe fn queue_receive_impl(
    queue: *mut c_void,
    buffer: *mut c_void,
    timeout: u32,
    just_peeking: bool,
) -> i32 {
    unsafe {
        if queue.is_null() {
            return PD_FALSE;
        }
        let q = queue as *mut RawQueue;
        let task_idx = task::current_task_index();
        let mut entry_time_set = false;
        let mut ticks_to_wait = timeout;
        let mut timeout_state = task::new_timeout_state();

        loop {
            critical::vPortEnterCritical();
            let ok = if just_peeking {
                (*q).peek(buffer as *mut u8)
            } else {
                (*q).receive(buffer as *mut u8)
            };
            if ok {
                (*q).rx_waiters.remove(task_idx);
                if !just_peeking {
                    let need_switch = notify_queue_received(q);
                    critical::vPortExitCritical();
                    if need_switch {
                        super::port::PendSV(1);
                    }
                } else {
                    critical::vPortExitCritical();
                }
                return PD_PASS;
            }

            if ticks_to_wait == 0 {
                critical::vPortExitCritical();
                return PD_FALSE;
            }

            if !entry_time_set {
                task::set_timeout_state(&mut timeout_state);
                entry_time_set = true;
            }
            critical::vPortExitCritical();

            task::vTaskSuspendAll();
            lock_queue(q);
            let timed_out = task::check_for_timeout(&mut timeout_state, &mut ticks_to_wait);
            if !timed_out {
                critical::vPortEnterCritical();
                let queue_empty = (*q).count == 0;
                critical::vPortExitCritical();

                if queue_empty {
                    (*q).rx_waiters.add(task_idx);
                    task::block_current_task_on_queue_rx(queue, deadline_from_ticks(ticks_to_wait));
                    unlock_queue(q);
                    resume_all_and_maybe_yield();
                } else {
                    unlock_queue(q);
                    let _ = task::xTaskResumeAll();
                }
            } else {
                (*q).rx_waiters.remove(task_idx);
                unlock_queue(q);
                let _ = task::xTaskResumeAll();
                critical::vPortEnterCritical();
                let queue_empty = (*q).count == 0;
                critical::vPortExitCritical();
                if queue_empty {
                    return PD_FALSE;
                }
            }
        }
    }
}

/// `xQueueReceiveFromISR` — receive from ISR (never blocks).
#[no_mangle]
pub unsafe extern "C" fn xQueueReceiveFromISR(
    queue: *mut c_void,
    buffer: *mut c_void,
    higher_priority_task_woken: *mut i32,
) -> i32 {
    unsafe {
        if queue.is_null() {
            return PD_FALSE;
        }
        let q = queue as *mut RawQueue;
        if (*q).receive(buffer as *mut u8) {
            let need_switch = notify_queue_received(q);
            if !higher_priority_task_woken.is_null() {
                *higher_priority_task_woken = if need_switch { PD_PASS } else { PD_FALSE };
            }
            PD_PASS
        } else {
            PD_FALSE
        }
    }
}

// ---------------------------------------------------------------------------
// Semaphore take/give (queue-based, as FreeRTOS macros expand to these)
// ---------------------------------------------------------------------------

/// `xQueueSemaphoreTake` — take a semaphore (decrement count).
///
/// This is what the C macro `xSemaphoreTake()` expands to.
#[no_mangle]
pub unsafe extern "C" fn xQueueSemaphoreTake(queue: *mut c_void, timeout: u32) -> i32 {
    unsafe {
        if queue.is_null() {
            return PD_FALSE;
        }
        let q = queue as *mut RawQueue;
        let task_idx = task::current_task_index();
        let mut entry_time_set = false;
        let mut ticks_to_wait = timeout;
        let mut timeout_state = task::new_timeout_state();

        loop {
            critical::vPortEnterCritical();
            if (*q).count > 0 {
                (*q).count -= 1;
                (*q).rx_waiters.remove(task_idx);
                if (*q).queue_type == QUEUE_TYPE_MUTEX
                    || (*q).queue_type == QUEUE_TYPE_RECURSIVE_MUTEX
                {
                    (*q).mutex_holder = task::current_task_index();
                }
                let need_switch = notify_queue_received(q);
                critical::vPortExitCritical();
                if need_switch {
                    super::port::PendSV(1);
                }
                return PD_PASS;
            }

            if ticks_to_wait == 0 {
                critical::vPortExitCritical();
                return PD_FALSE;
            }

            // Priority inheritance: if this is a mutex, raise the holder's priority.
            if ((*q).queue_type == QUEUE_TYPE_MUTEX
                || (*q).queue_type == QUEUE_TYPE_RECURSIVE_MUTEX)
                && (*q).mutex_holder != usize::MAX
            {
                let my_prio = task::task_priority(task::current_task_index());
                task::raise_priority((*q).mutex_holder, my_prio);
            }

            if !entry_time_set {
                task::set_timeout_state(&mut timeout_state);
                entry_time_set = true;
            }
            critical::vPortExitCritical();

            task::vTaskSuspendAll();
            lock_queue(q);
            let timed_out = task::check_for_timeout(&mut timeout_state, &mut ticks_to_wait);
            critical::vPortEnterCritical();
            if !timed_out {
                if (*q).count == 0 {
                    (*q).rx_waiters.add(task_idx);
                    task::block_current_task_on_queue_rx(queue, deadline_from_ticks(ticks_to_wait));
                    critical::vPortExitCritical();

                    unlock_queue(q);
                    resume_all_and_maybe_yield();
                } else {
                    critical::vPortExitCritical();
                    unlock_queue(q);
                    let _ = task::xTaskResumeAll();
                }
            } else {
                (*q).rx_waiters.remove(task_idx);
                // Restore holder's priority if no other waiters remain.
                if ((*q).queue_type == QUEUE_TYPE_MUTEX
                    || (*q).queue_type == QUEUE_TYPE_RECURSIVE_MUTEX)
                    && (*q).mutex_holder != usize::MAX
                {
                    task::restore_priority((*q).mutex_holder);
                }
                critical::vPortExitCritical();
                unlock_queue(q);
                let _ = task::xTaskResumeAll();
                return PD_FALSE;
            }
        }
    }
}

/// `xQueueTakeMutexRecursive` — take a recursive mutex.
#[no_mangle]
pub unsafe extern "C" fn xQueueTakeMutexRecursive(queue: *mut c_void, timeout: u32) -> i32 {
    unsafe {
        if queue.is_null() {
            return PD_FALSE;
        }
        let q = queue as *mut RawQueue;
        let current = task::current_task_index();

        critical::vPortEnterCritical();

        // If we already hold it, just increment the count.
        if (*q).mutex_holder == current {
            (*q).recursive_count += 1;
            critical::vPortExitCritical();
            return PD_PASS;
        }
        critical::vPortExitCritical();

        // Otherwise, take it normally.
        let ret = xQueueSemaphoreTake(queue, timeout);
        if ret == PD_PASS {
            (*q).recursive_count = 1;
        }
        ret
    }
}

/// `xQueueGiveMutexRecursive` — give a recursive mutex.
#[no_mangle]
pub unsafe extern "C" fn xQueueGiveMutexRecursive(queue: *mut c_void) -> i32 {
    unsafe {
        if queue.is_null() {
            return PD_FALSE;
        }
        let q = queue as *mut RawQueue;
        let current = task::current_task_index();

        critical::vPortEnterCritical();

        if (*q).mutex_holder != current {
            critical::vPortExitCritical();
            return PD_FALSE;
        }

        (*q).recursive_count -= 1;
        if (*q).recursive_count > 0 {
            critical::vPortExitCritical();
            return PD_PASS;
        }

        // Fully released — restore priority and wake a waiter.
        (*q).mutex_holder = usize::MAX;
        task::restore_priority(current);
        (*q).count += 1;
        let need_switch = notify_queue_posted(q);
        critical::vPortExitCritical();

        if need_switch {
            super::port::PendSV(1);
        }
        PD_PASS
    }
}

// ---------------------------------------------------------------------------
// Mutex-aware give (used by xSemaphoreGive)
// ---------------------------------------------------------------------------

/// Give a semaphore or mutex. For plain mutexes, this clears ownership and
/// restores priority inheritance — unlike `xQueueGenericSend` which only
/// handles the data-queue / counting-semaphore path.
///
/// This is what the FreeRTOS `xSemaphoreGive` macro effectively does:
/// it checks the queue type and dispatches to the mutex path when needed.
#[allow(non_snake_case)] // FreeRTOS API naming convention
pub(crate) unsafe fn xQueueGiveMutexAware(queue: *mut c_void) -> i32 {
    unsafe {
        if queue.is_null() {
            return PD_FALSE;
        }
        let q = queue as *mut RawQueue;

        if (*q).queue_type == QUEUE_TYPE_MUTEX {
            // Plain mutex: clear holder, restore priority, increment count.
            critical::vPortEnterCritical();

            let current = task::current_task_index();
            if (*q).mutex_holder != current {
                // Not the holder — FreeRTOS returns failure here.
                critical::vPortExitCritical();
                return PD_FALSE;
            }

            (*q).mutex_holder = usize::MAX;
            task::restore_priority(current);
            (*q).count += 1;
            let need_switch = notify_queue_posted(q);
            critical::vPortExitCritical();

            if need_switch {
                super::port::PendSV(1);
            }
            PD_PASS
        } else if (*q).queue_type == QUEUE_TYPE_RECURSIVE_MUTEX {
            // Recursive mutex: delegate to the recursive give path.
            xQueueGiveMutexRecursive(queue)
        } else {
            // Counting semaphore / binary semaphore / data queue:
            // use the generic send path (increment count / enqueue item).
            xQueueGenericSend(queue, core::ptr::null(), 0, 0)
        }
    }
}

// ---------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------

/// `uxQueueMessagesWaiting` — items in the queue.
#[no_mangle]
pub unsafe extern "C" fn uxQueueMessagesWaiting(queue: *const c_void) -> u32 {
    unsafe {
        if queue.is_null() {
            return 0;
        }
        let q = &*(queue as *const RawQueue);
        q.count as u32
    }
}

/// `uxQueueSpacesAvailable` — free slots in the queue.
#[no_mangle]
pub unsafe extern "C" fn uxQueueSpacesAvailable(queue: *const c_void) -> u32 {
    unsafe {
        if queue.is_null() {
            return 0;
        }
        let q = &*(queue as *const RawQueue);
        (q.capacity - q.count) as u32
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

unsafe fn try_send(q: *mut RawQueue, item: *const u8, pos: i32) -> bool {
    unsafe {
        let q = &mut *q;
        match pos {
            QUEUE_SEND_TO_BACK => q.send_back(item),
            QUEUE_SEND_TO_FRONT => q.send_front(item),
            QUEUE_OVERWRITE => {
                q.overwrite(item);
                true
            }
            _ => q.send_back(item),
        }
    }
}

unsafe fn deadline_from_ticks(ticks_to_wait: u32) -> u64 {
    unsafe {
        if ticks_to_wait == 0xFFFF_FFFF {
            0
        } else {
            task::get_tick_count() + ticks_to_wait as u64
        }
    }
}

#[inline(never)]
pub(crate) unsafe fn resume_all_and_maybe_yield() {
    unsafe {
        let already_yielded = task::xTaskResumeAll() != 0;
        if !already_yielded {
            super::port::PendSV(1);
        }
    }
}

unsafe fn lock_queue(q: *mut RawQueue) {
    unsafe {
        critical::vPortEnterCritical();
        let q = &mut *q;
        if q.rx_lock == QUEUE_UNLOCKED {
            q.rx_lock = QUEUE_LOCKED_UNMODIFIED;
        }
        if q.tx_lock == QUEUE_UNLOCKED {
            q.tx_lock = QUEUE_LOCKED_UNMODIFIED;
        }
        critical::vPortExitCritical();
    }
}

unsafe fn unlock_queue(q: *mut RawQueue) {
    unsafe {
        critical::vPortEnterCritical();
        let q = &mut *q;
        let mut tx_lock = q.tx_lock;
        while tx_lock > QUEUE_LOCKED_UNMODIFIED {
            if q.rx_waiters.wake_highest_priority() {
                task::missed_yield();
            }
            tx_lock -= 1;
            if !has_active_waiter(&q.rx_waiters) {
                break;
            }
        }
        q.tx_lock = QUEUE_UNLOCKED;
        critical::vPortExitCritical();

        critical::vPortEnterCritical();
        let mut rx_lock = q.rx_lock;
        while rx_lock > QUEUE_LOCKED_UNMODIFIED {
            if q.tx_waiters.wake_highest_priority() {
                task::missed_yield();
            }
            rx_lock -= 1;
            if !has_active_waiter(&q.tx_waiters) {
                break;
            }
        }
        q.rx_lock = QUEUE_UNLOCKED;
        critical::vPortExitCritical();
    }
}

unsafe fn notify_queue_posted(q: *mut RawQueue) -> bool {
    unsafe {
        let q = &mut *q;
        if q.tx_lock == QUEUE_UNLOCKED {
            q.rx_waiters.wake_highest_priority()
        } else {
            q.tx_lock = q.tx_lock.saturating_add(1);
            false
        }
    }
}

unsafe fn notify_queue_received(q: *mut RawQueue) -> bool {
    unsafe {
        let q = &mut *q;
        if q.rx_lock == QUEUE_UNLOCKED {
            q.tx_waiters.wake_highest_priority()
        } else {
            q.rx_lock = q.rx_lock.saturating_add(1);
            false
        }
    }
}

fn has_active_waiter(list: &WaitList) -> bool {
    let mut slot = 0usize;
    while slot < MAX_WAITERS {
        if list.entries[slot].active {
            return true;
        }
        slot += 1;
    }
    false
}

pub(crate) unsafe fn remove_rx_waiter_for_task(queue: *mut c_void, task_idx: usize) {
    unsafe {
        if queue.is_null() {
            return;
        }
        let q = &mut *(queue as *mut RawQueue);
        q.rx_waiters.remove(task_idx);
    }
}

pub(crate) unsafe fn remove_tx_waiter_for_task(queue: *mut c_void, task_idx: usize) {
    unsafe {
        if queue.is_null() {
            return;
        }
        let q = &mut *(queue as *mut RawQueue);
        q.tx_waiters.remove(task_idx);
    }
}
