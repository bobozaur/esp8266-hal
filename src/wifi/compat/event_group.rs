//! Minimal FreeRTOS event-group support for the SDK station-style wait path.
//!
//! This implements the subset used by the ESP8266 RTOS SDK station examples:
//! dynamic creation, wait bits, set bits, clear bits, and delete.

use core::ffi::c_void;

use super::{critical, memory, port, task};

const PD_FALSE: i32 = 0;
const MAX_EVENT_WAITERS: usize = 6;

#[derive(Clone, Copy)]
struct EventWaiter {
    task_idx: usize,
    bits_to_wait: u32,
    clear_on_exit: bool,
    wait_for_all: bool,
    active: bool,
}

#[repr(C)]
struct RawEventGroup {
    bits: u32,
    waiters: [EventWaiter; MAX_EVENT_WAITERS],
}

impl RawEventGroup {
    const fn new() -> Self {
        const EMPTY: EventWaiter = EventWaiter {
            task_idx: 0,
            bits_to_wait: 0,
            clear_on_exit: false,
            wait_for_all: false,
            active: false,
        };
        Self {
            bits: 0,
            waiters: [EMPTY; MAX_EVENT_WAITERS],
        }
    }

    fn add_or_update_waiter(
        &mut self,
        task_idx: usize,
        bits_to_wait: u32,
        clear_on_exit: bool,
        wait_for_all: bool,
    ) {
        let mut free_slot = None;

        for (slot, waiter) in self.waiters.iter_mut().enumerate() {
            if waiter.active {
                if waiter.task_idx == task_idx {
                    waiter.bits_to_wait = bits_to_wait;
                    waiter.clear_on_exit = clear_on_exit;
                    waiter.wait_for_all = wait_for_all;
                    return;
                }

                if unsafe { !task::task_is_blocked(waiter.task_idx) } {
                    waiter.active = false;
                }
            }

            if !waiter.active && free_slot.is_none() {
                free_slot = Some(slot);
            }
        }

        if let Some(slot) = free_slot {
            self.waiters[slot] = EventWaiter {
                task_idx,
                bits_to_wait,
                clear_on_exit,
                wait_for_all,
                active: true,
            };
        }
    }

    fn remove_waiter(&mut self, task_idx: usize) {
        for waiter in &mut self.waiters {
            if waiter.active && waiter.task_idx == task_idx {
                waiter.active = false;
            }
        }
    }
}

#[inline]
fn wait_condition_met(current_bits: u32, bits_to_wait: u32, wait_for_all: bool) -> bool {
    if wait_for_all {
        (current_bits & bits_to_wait) == bits_to_wait
    } else {
        (current_bits & bits_to_wait) != 0
    }
}

#[inline]
unsafe fn deadline_from_ticks(ticks_to_wait: u32) -> u64 {
    unsafe {
        if ticks_to_wait == 0xFFFF_FFFF {
            0
        } else {
            task::get_tick_count() + ticks_to_wait as u64
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn xEventGroupCreate() -> *mut c_void {
    unsafe {
        let raw = memory::zalloc(core::mem::size_of::<RawEventGroup>()) as *mut RawEventGroup;
        if raw.is_null() {
            return core::ptr::null_mut();
        }
        raw.write(RawEventGroup::new());
        raw as *mut c_void
    }
}

#[no_mangle]
pub unsafe extern "C" fn xEventGroupWaitBits(
    event_group: *mut c_void,
    bits_to_wait_for: u32,
    clear_on_exit: i32,
    wait_for_all_bits: i32,
    ticks_to_wait: u32,
) -> u32 {
    unsafe {
        if event_group.is_null() || bits_to_wait_for == 0 {
            return 0;
        }

        let group = &mut *(event_group as *mut RawEventGroup);
        let task_idx = task::current_task_index();
        let clear_on_exit = clear_on_exit != 0;
        let wait_for_all = wait_for_all_bits != 0;

        task::set_event_wait_result(task_idx, 0, false);

        task::vTaskSuspendAll();

        let current_bits = group.bits;
        if wait_condition_met(current_bits, bits_to_wait_for, wait_for_all) {
            if clear_on_exit {
                group.bits &= !bits_to_wait_for;
            }
            let _ = task::xTaskResumeAll();
            return current_bits;
        }

        if ticks_to_wait == 0 {
            let _ = task::xTaskResumeAll();
            return current_bits;
        }

        group.add_or_update_waiter(task_idx, bits_to_wait_for, clear_on_exit, wait_for_all);
        task::block_current_task_on_event_group(event_group, deadline_from_ticks(ticks_to_wait));

        let already_yielded = task::xTaskResumeAll() != 0;
        if !already_yielded {
            port::PendSV(1);
        }

        let (wake_bits, matched) = task::take_event_wait_result(task_idx);
        if matched {
            return wake_bits;
        }

        group.remove_waiter(task_idx);

        critical::vPortEnterCritical();
        let current_bits = group.bits;
        if wait_condition_met(current_bits, bits_to_wait_for, wait_for_all) && clear_on_exit {
            group.bits &= !bits_to_wait_for;
        }
        critical::vPortExitCritical();

        current_bits
    }
}

#[no_mangle]
pub unsafe extern "C" fn xEventGroupSetBits(event_group: *mut c_void, bits_to_set: u32) -> u32 {
    unsafe {
        if event_group.is_null() {
            return 0;
        }

        let group = &mut *(event_group as *mut RawEventGroup);
        let mut bits_to_clear = 0u32;

        task::vTaskSuspendAll();

        group.bits |= bits_to_set;
        let current_bits = group.bits;

        for waiter in &mut group.waiters {
            if !waiter.active {
                continue;
            }

            if !task::task_is_blocked(waiter.task_idx) {
                waiter.active = false;
                continue;
            }

            if wait_condition_met(current_bits, waiter.bits_to_wait, waiter.wait_for_all) {
                if waiter.clear_on_exit {
                    bits_to_clear |= waiter.bits_to_wait;
                }
                task::set_event_wait_result(waiter.task_idx, current_bits, true);
                waiter.active = false;
                let _ = task::wake_task(waiter.task_idx);
            }
        }

        group.bits &= !bits_to_clear;
        let result = group.bits;
        let _ = task::xTaskResumeAll();
        result
    }
}

#[no_mangle]
pub unsafe extern "C" fn xEventGroupClearBits(event_group: *mut c_void, bits_to_clear: u32) -> u32 {
    unsafe {
        if event_group.is_null() {
            return 0;
        }

        let group = &mut *(event_group as *mut RawEventGroup);
        critical::vPortEnterCritical();
        let previous_bits = group.bits;
        group.bits &= !bits_to_clear;
        critical::vPortExitCritical();
        previous_bits
    }
}

#[no_mangle]
pub unsafe extern "C" fn vEventGroupDelete(event_group: *mut c_void) {
    unsafe {
        if event_group.is_null() {
            return;
        }

        let group = &mut *(event_group as *mut RawEventGroup);
        task::vTaskSuspendAll();
        for waiter in &mut group.waiters {
            if waiter.active {
                task::set_event_wait_result(waiter.task_idx, 0, false);
                waiter.active = false;
                let _ = task::wake_task(waiter.task_idx);
            }
        }
        let _ = task::xTaskResumeAll();
        memory::free(event_group);
    }
}

#[no_mangle]
pub unsafe extern "C" fn xEventGroupSetBitsFromISR(
    event_group: *mut c_void,
    bits_to_set: u32,
    higher_priority_task_woken: *mut i32,
) -> i32 {
    unsafe {
        let before = task::current_task_index();
        let _ = xEventGroupSetBits(event_group, bits_to_set);
        if !higher_priority_task_woken.is_null() {
            *higher_priority_task_woken = if task::current_task_index() != before {
                1
            } else {
                0
            };
        }
        PD_FALSE
    }
}

pub(crate) unsafe fn remove_waiter_for_task(event_group: *mut c_void, task_idx: usize) {
    unsafe {
        if event_group.is_null() {
            return;
        }

        let group = &mut *(event_group as *mut RawEventGroup);
        group.remove_waiter(task_idx);
    }
}
