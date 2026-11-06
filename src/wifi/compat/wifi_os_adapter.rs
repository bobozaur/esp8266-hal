//! Rust implementation of the OS adapter table consumed by the Wi-Fi blobs.
//!
//! This layer translates SDK queue-position numbers and delegates to the
//! compat scheduler primitives. Driver policy belongs above this ABI boundary.

use core::ffi::c_void;

use super::{port, queue, task};

const OSI_SEND_FRONT: u32 = 0;
const OSI_SEND_BACK: u32 = 1;
const OSI_SEND_OVERWRITE: u32 = 2;

const QUEUE_SEND_TO_BACK: i32 = 0;
const QUEUE_SEND_TO_FRONT: i32 = 1;
const QUEUE_OVERWRITE: i32 = 2;

const MAX_PRIORITIES: u32 = 15;
const TICK_MILLIS: u32 = 10;

#[inline(always)]
fn task_name(name: *const u8) -> [u8; 16] {
    let mut output = [0; 16];
    if name.is_null() {
        return output;
    }

    let mut index = 0;
    while index < output.len() - 1 {
        let byte = unsafe { name.add(index).read() };
        if byte == 0 {
            break;
        }
        output[index] = byte;
        index += 1;
    }
    output
}

#[no_mangle]
pub unsafe extern "C" fn __wifi_task_create(
    entry: unsafe extern "C" fn(*mut c_void),
    name: *const u8,
    requested_stack_bytes: u32,
    argument: *mut c_void,
    priority: u32,
) -> *mut c_void {
    let name_bytes = task_name(name);
    let mut handle = core::ptr::null_mut();
    let result = unsafe {
        task::create_task(
            entry,
            requested_stack_bytes as u16,
            argument,
            priority,
            &mut handle,
            name_bytes.starts_with(b"ppT\0")
                && requested_stack_bytes == task::PP_TASK_STACK_BYTES as u32,
        )
    };
    if result == 1 {
        handle
    } else {
        core::ptr::null_mut()
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wifi_task_yield_from_isr() {
    unsafe { port::portYIELD_FROM_ISR() }
}

#[no_mangle]
pub unsafe extern "C" fn __wifi_task_delay(ticks: u32) {
    unsafe { task::vTaskDelay(ticks) }
}

#[no_mangle]
pub extern "C" fn __wifi_task_get_max_priority() -> u32 {
    MAX_PRIORITIES
}

#[no_mangle]
pub extern "C" fn __wifi_task_ms_to_ticks(milliseconds: u32) -> u32 {
    milliseconds / TICK_MILLIS
}

#[no_mangle]
pub unsafe extern "C" fn __wifi_task_suspend_all() {
    unsafe { task::vTaskSuspendAll() }
}

#[no_mangle]
pub unsafe extern "C" fn __wifi_task_resume_all() {
    let _ = unsafe { task::xTaskResumeAll() };
}

#[no_mangle]
pub unsafe extern "C" fn __wifi_queue_create(length: u32, item_size: u32) -> *mut c_void {
    unsafe {
        if length == 75 && item_size == 8 {
            queue::create_pp_queue()
        } else {
            queue::xQueueCreate(length, item_size)
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wifi_queue_send(
    handle: *mut c_void,
    item: *mut c_void,
    wait_ticks: u32,
    position: u32,
) -> i32 {
    let position = match position {
        OSI_SEND_FRONT => QUEUE_SEND_TO_FRONT,
        OSI_SEND_BACK => QUEUE_SEND_TO_BACK,
        OSI_SEND_OVERWRITE => QUEUE_OVERWRITE,
        _ => QUEUE_SEND_TO_BACK,
    };
    (unsafe { queue::xQueueGenericSend(handle, item.cast_const(), wait_ticks, position) } == 1)
        as i32
}

#[no_mangle]
pub unsafe extern "C" fn __wifi_queue_send_from_isr(
    handle: *mut c_void,
    item: *mut c_void,
    higher_priority_task_woken: *mut i32,
) -> i32 {
    (unsafe { queue::xQueueSendFromISR(handle, item.cast_const(), higher_priority_task_woken) }
        == 1) as i32
}

#[no_mangle]
pub unsafe extern "C" fn __wifi_queue_recv(
    handle: *mut c_void,
    output: *mut c_void,
    wait_ticks: u32,
) -> i32 {
    (unsafe { queue::xQueueReceive(handle, output, wait_ticks) } == 1) as i32
}

#[no_mangle]
pub unsafe extern "C" fn __wifi_queue_msg_num(handle: *mut c_void) -> u32 {
    unsafe { queue::uxQueueMessagesWaiting(handle.cast_const()) }
}
