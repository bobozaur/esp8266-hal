//! Rust implementation of the OS adapter table consumed by the Wi-Fi blobs.
//!
//! Keep this layer deliberately mechanical: it translates SDK conventions
//! (blocking sentinel values and queue-position numbers) and delegates to the
//! compat scheduler primitives. Driver policy belongs above this ABI boundary.

use core::ffi::c_void;

use super::{port, queue, semphr, task, timer};

const OSI_TIME_BLOCKING: u32 = u32::MAX;
const OSI_SEND_FRONT: u32 = 0;
const OSI_SEND_BACK: u32 = 1;
const OSI_SEND_OVERWRITE: u32 = 2;

const QUEUE_SEND_TO_BACK: i32 = 0;
const QUEUE_SEND_TO_FRONT: i32 = 1;
const QUEUE_OVERWRITE: i32 = 2;

const MAX_PRIORITIES: u32 = 15;
const TICK_MILLIS: u32 = 10;

// SDK panic/WDT code expects its own XtExcFrame layout, while the HAL level-1
// dispatcher saves xtensa-lx-rt's Context layout. Convert only when an ISR
// asks for the current task frame.
#[repr(C, align(16))]
struct SdkExceptionFrame {
    words: [u32; 21],
}

static mut SDK_EXCEPTION_FRAME: SdkExceptionFrame = SdkExceptionFrame { words: [0; 21] };

#[inline(always)]
const fn timeout(ticks: u32) -> u32 {
    if ticks == OSI_TIME_BLOCKING {
        u32::MAX
    } else {
        ticks
    }
}

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

#[inline(always)]
fn stack_bytes(name: &[u8; 16], requested: u16) -> u16 {
    // The event task only dispatches a compact legacy event value in this
    // driver. The SDK default is sized for the full tcpip_adapter path.
    if name.starts_with(b"sys_evt") {
        requested.min(1_024)
    } else {
        requested
    }
}

#[inline(always)]
unsafe fn frame_word(frame: *const u8, offset: usize) -> u32 {
    unsafe { frame.add(offset).cast::<u32>().read_volatile() }
}

unsafe fn sdk_exception_frame(frame: *const u8) -> *mut c_void {
    const PC: usize = 0x00;
    const PS: usize = 0x04;
    const A0: usize = 0x08;
    const A1: usize = 0x0c;
    const A2: usize = 0x10;
    const SAR: usize = 0x48;
    const EXCCAUSE: usize = 0x4c;

    unsafe {
        let output = &raw mut SDK_EXCEPTION_FRAME;
        (*output).words[0] = 0; // XT_STK_EXIT
        (*output).words[1] = frame_word(frame, PC);
        (*output).words[2] = frame_word(frame, PS);
        (*output).words[3] = frame_word(frame, A0);
        (*output).words[4] = frame_word(frame, A1);
        for register in 0..14 {
            (*output).words[5 + register] = frame_word(frame, A2 + register * 4);
        }
        (*output).words[19] = frame_word(frame, SAR);
        (*output).words[20] = frame_word(frame, EXCCAUSE);
        (*output).words.as_mut_ptr().cast()
    }
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
    let stack_bytes = stack_bytes(&name_bytes, requested_stack_bytes as u16);
    let mut handle = core::ptr::null_mut();
    let result = unsafe {
        task::create_task(
            entry,
            name.cast(),
            stack_bytes,
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
pub unsafe extern "C" fn __wifi_task_delete(handle: *mut c_void) {
    unsafe { task::vTaskDelete(handle) }
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
pub unsafe extern "C" fn __wifi_task_top_sp() -> *mut c_void {
    unsafe {
        let current = task::pxCurrentTCB;
        if current.is_null() {
            return core::ptr::null_mut();
        }

        // The first field of every compat TCB is the saved stack pointer.
        let stack_top = current.cast::<*mut u8>().read();
        if stack_top.is_null() {
            return core::ptr::null_mut();
        }
        if port::xPortInIsrContext() != 0 {
            sdk_exception_frame(stack_top.cast_const())
        } else {
            stack_top.cast()
        }
    }
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
pub unsafe extern "C" fn __wifi_queue_delete(handle: *mut c_void) {
    unsafe { queue::vQueueDelete(handle) }
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
    (unsafe { queue::xQueueGenericSend(handle, item.cast_const(), timeout(wait_ticks), position) }
        == 1) as i32
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
    (unsafe { queue::xQueueReceive(handle, output, timeout(wait_ticks)) } == 1) as i32
}

#[no_mangle]
pub unsafe extern "C" fn __wifi_queue_msg_num(handle: *mut c_void) -> u32 {
    unsafe { queue::uxQueueMessagesWaiting(handle.cast_const()) }
}

#[no_mangle]
pub unsafe extern "C" fn __wifi_timer_create(
    name: *const u8,
    period_ticks: u32,
    auto_reload: bool,
    argument: *mut c_void,
    callback: unsafe extern "C" fn(*mut c_void),
) -> *mut c_void {
    unsafe {
        timer::xTimerCreate(
            name,
            period_ticks,
            i32::from(auto_reload),
            argument,
            callback,
        )
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wifi_timer_reset(handle: *mut c_void, wait_ticks: u32) -> i32 {
    unsafe { timer::xTimerReset(handle, wait_ticks) }
}

#[no_mangle]
pub unsafe extern "C" fn __wifi_timer_stop(handle: *mut c_void, wait_ticks: u32) -> i32 {
    unsafe { timer::xTimerStop(handle, wait_ticks) }
}

#[no_mangle]
pub unsafe extern "C" fn __wifi_semphr_create(maximum: u32, initial: u32) -> *mut c_void {
    unsafe { semphr::xSemaphoreCreateCounting(maximum, initial) }
}

#[no_mangle]
pub unsafe extern "C" fn __wifi_semphr_delete(handle: *mut c_void) {
    unsafe { semphr::vSemaphoreDelete(handle) }
}

#[no_mangle]
pub unsafe extern "C" fn __wifi_semphr_take(handle: *mut c_void, wait_ticks: u32) -> i32 {
    unsafe { semphr::xSemaphoreTake(handle, timeout(wait_ticks)) }
}

#[no_mangle]
pub unsafe extern "C" fn __wifi_semphr_give(handle: *mut c_void) -> i32 {
    unsafe { semphr::xSemaphoreGive(handle) }
}
