//! WiFi OS adapter shim functions (`__wifi_*`).
//!
//! Direct Rust port of `esp_wifi_os_adapter.c`. Each `__wifi_*` function
//! is called by the WiFi blob libraries and delegates to our compat
//! implementations of the FreeRTOS primitives.

use core::ffi::c_void;
use core::ptr;
use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use super::{port, queue, semphr, task, timer};

// ---------------------------------------------------------------------------
// Constants (from esp_wifi_osi.h)
// ---------------------------------------------------------------------------

/// Infinite blocking time.
const OSI_FUNCS_TIME_BLOCKING: u32 = 0xFFFF_FFFF;
/// Queue send positions.
const OSI_QUEUE_SEND_BACK: u32 = 1;
const OSI_QUEUE_SEND_FRONT: u32 = 0;
const OSI_QUEUE_SEND_OVERWRITE: u32 = 2;

/// FreeRTOS queue positions.
const QUEUE_SEND_TO_BACK: i32 = 0;
const QUEUE_SEND_TO_FRONT: i32 = 1;
const QUEUE_OVERWRITE: i32 = 2;

/// `configMAX_PRIORITIES`.
const MAX_PRIORITIES: u32 = 15;

/// `portMAX_DELAY`.
const PORT_MAX_DELAY: u32 = 0xFFFF_FFFF;

/// `portTICK_RATE_MS` = 1000/100 = 10.
const PORT_TICK_RATE_MS: u32 = 10;

// `ppTask` uses a queue created as `__wifi_queue_create(0x4b, 8)`.
static PP_QUEUE_PTR: AtomicUsize = AtomicUsize::new(0);
static PP_SIG_TRACE_REMAINING: AtomicU8 = AtomicU8::new(0);
static WIFI_TASK_TOP_SP_TRACE_REMAINING: AtomicU8 = AtomicU8::new(4);

// SDK panic/WDT code expects an `XtExcFrame` with the ESP8266 RTOS SDK
// layout (`XT_STK_EXIT` at word 0, then PC/PS/A0/A1/...).
#[repr(C, align(16))]
struct SdkExcFrameShadow {
    words: [u32; 21],
}

static mut WIFI_TASK_TOP_SP_SHADOW: SdkExcFrameShadow = SdkExcFrameShadow { words: [0; 21] };

#[no_mangle]
pub static mut g_queue_recv_stage: u32 = 0;
#[no_mangle]
pub static mut g_pp_last_sig: u32 = 0;
#[no_mangle]
pub static mut g_pp_last_arg: u32 = 0;
#[no_mangle]
pub static mut g_wifi_sched_stage: u32 = 0;
#[no_mangle]
pub static mut g_wifi_sched_before: u32 = 0;
#[no_mangle]
pub static mut g_wifi_sched_after: u32 = 0;
#[no_mangle]
pub static mut g_wifi_sched_ret: u32 = 0;
#[no_mangle]
pub static mut g_wifi_sched_suspend_calls: u32 = 0;
#[no_mangle]
pub static mut g_wifi_sched_resume_calls: u32 = 0;

extern "C" {
    static mut pTxRx: *mut u8;
    static mut g_ic: u32;
}

#[inline]
fn hex_nibble(n: u8) -> u8 {
    match n & 0x0f {
        0..=9 => b'0' + (n & 0x0f),
        _ => b'a' + ((n & 0x0f) - 10),
    }
}

#[inline]
fn trace_hex_u32(value: u32) {
    crate::logger::trace_byte(hex_nibble((value >> 28) as u8));
    crate::logger::trace_byte(hex_nibble((value >> 24) as u8));
    crate::logger::trace_byte(hex_nibble((value >> 20) as u8));
    crate::logger::trace_byte(hex_nibble((value >> 16) as u8));
    crate::logger::trace_byte(hex_nibble((value >> 12) as u8));
    crate::logger::trace_byte(hex_nibble((value >> 8) as u8));
    crate::logger::trace_byte(hex_nibble((value >> 4) as u8));
    crate::logger::trace_byte(hex_nibble(value as u8));
}

#[inline]
unsafe fn read_xtensa_rt_word(frame: *const u8, offset: usize) -> u32 {
    unsafe { (frame.add(offset) as *const u32).read_volatile() }
}

#[inline]
unsafe fn trace_wifi_task_top_sp(frame: *const u8) {
    const XT_RT_PC: usize = 0x00;
    const XT_RT_A1: usize = 0x0c;
    const XT_RT_EXCCAUSE: usize = 0x4c;

    let remaining = WIFI_TASK_TOP_SP_TRACE_REMAINING.load(Ordering::Relaxed);
    if remaining == 0 {
        return;
    }
    WIFI_TASK_TOP_SP_TRACE_REMAINING.store(remaining - 1, Ordering::Relaxed);

    let frame_u32 = frame as u32;
    let pc = unsafe { read_xtensa_rt_word(frame, XT_RT_PC) };
    let a1 = unsafe { read_xtensa_rt_word(frame, XT_RT_A1) };
    let exccause = unsafe { read_xtensa_rt_word(frame, XT_RT_EXCCAUSE) };

    crate::logger::trace_byte(b'W');
    crate::logger::trace_byte(b'F');
    trace_hex_u32(frame_u32);
    crate::logger::trace_byte(b'P');
    trace_hex_u32(pc);
    crate::logger::trace_byte(b'S');
    trace_hex_u32(a1);
    crate::logger::trace_byte(b'E');
    trace_hex_u32(exccause);
    crate::logger::trace_byte(b';');
}

unsafe fn sdk_panic_frame_from_xtensa_rt(frame: *const u8) -> *mut c_void {
    const XT_RT_PC: usize = 0x00;
    const XT_RT_PS: usize = 0x04;
    const XT_RT_A0: usize = 0x08;
    const XT_RT_A1: usize = 0x0c;
    const XT_RT_A2: usize = 0x10;
    const XT_RT_SAR: usize = 0x48;
    const XT_RT_EXCCAUSE: usize = 0x4c;

    unsafe {
        let shadow = &raw mut WIFI_TASK_TOP_SP_SHADOW;
        (*shadow).words[0] = 0;
        (*shadow).words[1] = read_xtensa_rt_word(frame, XT_RT_PC);
        (*shadow).words[2] = read_xtensa_rt_word(frame, XT_RT_PS);
        (*shadow).words[3] = read_xtensa_rt_word(frame, XT_RT_A0);
        (*shadow).words[4] = read_xtensa_rt_word(frame, XT_RT_A1);

        let mut i = 0usize;
        while i < 14 {
            (*shadow).words[5 + i] = read_xtensa_rt_word(frame, XT_RT_A2 + i * 4);
            i += 1;
        }

        (*shadow).words[19] = read_xtensa_rt_word(frame, XT_RT_SAR);
        (*shadow).words[20] = read_xtensa_rt_word(frame, XT_RT_EXCCAUSE);

        (*shadow).words.as_mut_ptr() as *mut c_void
    }
}

unsafe fn trace_pp_rx_head_summary() {
    unsafe {
        if pTxRx.is_null() {
            crate::logger::write_byte(b'x');
            return;
        }

        let rx_head = ptr::read(pTxRx.add(0xb0) as *const *mut u8);
        if rx_head.is_null() {
            crate::logger::write_byte(b'0');
            return;
        }

        let rx_mode_ptr = ptr::read(rx_head.add(0x28) as *const *const u32);
        let rx_mode = if rx_mode_ptr.is_null() {
            0
        } else {
            ptr::read(rx_mode_ptr)
        };
        crate::logger::write_byte(if (rx_mode & 0x10) != 0 { b'M' } else { b'm' });

        let hdr = ptr::read(rx_head.add(0x0c) as *const *const u8);
        if hdr.is_null() {
            crate::logger::write_byte(b'h');
            return;
        }

        let fc0 = ptr::read(hdr);
        let fc1 = ptr::read(hdr.add(1));
        let seq_frag = ptr::read_unaligned(hdr.add(0x16) as *const u16);

        crate::logger::write_byte(hex_nibble(fc0 >> 4));
        crate::logger::write_byte(hex_nibble(fc0));

        let reassembly_case =
            (fc0 & 0x04) == 0 && (((fc1 & 0x04) != 0) || ((seq_frag & 0x000f) != 0));
        let path_marker = if (fc0 & 0x04) != 0 {
            b'H'
        } else if reassembly_case {
            b'F'
        } else {
            b'N'
        };
        crate::logger::write_byte(path_marker);

        let rx_desc = ptr::read(rx_head.add(0x10) as *const *const u32);
        if rx_desc.is_null() {
            crate::logger::write_byte(b'x');
            return;
        }

        let desc0 = ptr::read(rx_desc);
        let desc2 = ptr::read(rx_desc.add(2));
        let desc2_byte = (desc2 & 0xff) as u8;

        crate::logger::write_byte(hex_nibble((desc0 >> 28) as u8));
        crate::logger::write_byte(hex_nibble(desc2_byte >> 4));
        crate::logger::write_byte(hex_nibble(desc2_byte));

        let g_ic_base = &raw const g_ic as *const u32;
        let scan_cb_base = ptr::read(g_ic_base.add(0x51)) as *const u32;
        let wps_cb_base = ptr::read(g_ic_base.add(0x52)) as *const u32;
        let scan_cb = if scan_cb_base.is_null() {
            0
        } else {
            ptr::read(scan_cb_base.add(0x12))
        };
        let wps_cb = if wps_cb_base.is_null() {
            0
        } else {
            ptr::read(wps_cb_base)
        };
        let scan_mask = ptr::read(g_ic_base.add(0x6a));
        let scan_disable = ptr::read((&raw const g_ic).cast::<u8>().add(0x141));

        let scan_state = if scan_cb_base.is_null() {
            0
        } else if scan_cb == 0 {
            1
        } else {
            2
        };
        let wps_state = if wps_cb_base.is_null() {
            0
        } else if wps_cb == 0 {
            1
        } else {
            2
        };
        let scan_mask_state =
            ((scan_mask & (1 << 5)) != 0) as u8 | ((((scan_mask & (1 << 8)) != 0) as u8) << 1);

        crate::logger::write_byte(b'k');
        crate::logger::write_byte(hex_nibble(scan_state));
        crate::logger::write_byte(hex_nibble(wps_state));
        crate::logger::write_byte(hex_nibble(scan_mask_state));
        crate::logger::write_byte(hex_nibble(scan_disable));
    }
}

unsafe fn trace_pp_tx_head_summary() {
    unsafe {
        if pTxRx.is_null() {
            crate::logger::write_byte(b'x');
            return;
        }

        let tx_head = ptr::read(pTxRx.add(0xa8) as *const *mut u8);
        if tx_head.is_null() {
            crate::logger::write_byte(b'0');
            return;
        }

        let hdr = ptr::read(tx_head.add(0x0c) as *const *const u8);
        if hdr.is_null() {
            crate::logger::write_byte(b'h');
            return;
        }

        let fc0 = ptr::read(hdr);
        crate::logger::write_byte(if fc0 == 0x40 { b'P' } else { b'p' });
        crate::logger::write_byte(hex_nibble(fc0 >> 4));
        crate::logger::write_byte(hex_nibble(fc0));

        let addr1 = hdr.add(4);
        let addr2 = hdr.add(10);
        let addr3 = hdr.add(16);
        let addr1_bcast = (0..6).all(|i| ptr::read(addr1.add(i)) == 0xff);
        let addr3_bcast = (0..6).all(|i| ptr::read(addr3.add(i)) == 0xff);
        crate::logger::write_byte(if addr1_bcast { b'b' } else { b'u' });
        crate::logger::write_byte(if addr3_bcast { b'b' } else { b'u' });
        crate::logger::write_byte(b'a');
        crate::logger::write_byte(hex_nibble(ptr::read(addr2) >> 4));
        crate::logger::write_byte(hex_nibble(ptr::read(addr2)));
        crate::logger::write_byte(hex_nibble(ptr::read(addr2.add(1)) >> 4));
        crate::logger::write_byte(hex_nibble(ptr::read(addr2.add(1))));

        let ie = hdr.add(24);
        let tag = ptr::read(ie);
        let len = ptr::read(ie.add(1));
        crate::logger::write_byte(if tag == 0 { b's' } else { b'i' });
        crate::logger::write_byte(hex_nibble(len >> 4));
        crate::logger::write_byte(hex_nibble(len));
    }
}

#[inline(always)]
unsafe fn set_queue_recv_stage(marker: u8) {
    unsafe {
        core::ptr::write_volatile(&raw mut g_queue_recv_stage, marker as u32);
    }
}

#[inline(always)]
unsafe fn should_trace_sig10_sched() -> bool {
    unsafe { g_pp_last_sig == 0x10 && task::debug_current_task_marker() == b'P' }
}

#[inline(always)]
unsafe fn reset_sig10_sched_trace() {
    unsafe {
        g_wifi_sched_stage = 0;
        g_wifi_sched_before = 0;
        g_wifi_sched_after = 0;
        g_wifi_sched_ret = 0;
        g_wifi_sched_suspend_calls = 0;
        g_wifi_sched_resume_calls = 0;
        super::pp_stage::reset_sig10_scan_trace();
        super::pp_stage::reset_sig10_timer_trace();
    }
}

// ---------------------------------------------------------------------------
// Task shims
// ---------------------------------------------------------------------------

/// Create a FreeRTOS task on behalf of the WiFi blobs.
///
/// Returns the task handle or NULL on failure.
#[no_mangle]
pub unsafe extern "C" fn __wifi_task_create(
    task_func: unsafe extern "C" fn(*mut c_void),
    name: *const u8,
    stack_depth: u32,
    param: *mut c_void,
    prio: u32,
) -> *mut c_void {
    unsafe {
        // The ESP8266 RTOS SDK port defines `StackType_t = uint8_t`, so the
        // blob-provided `stack_depth` already matches the SDK `xTaskCreate`
        // unit: byte-counted stack items.
        let stack_depth = stack_depth as u16;
        let mut handle: *mut c_void = core::ptr::null_mut();
        let ret = task::xTaskCreate(
            task_func,
            name as *const _,
            stack_depth,
            param,
            prio,
            &mut handle,
        );
        if ret == 1 {
            handle
        } else {
            let mut name_buf = [0u8; 16];
            if !name.is_null() {
                let mut i = 0usize;
                while i < name_buf.len() - 1 {
                    let c = *name.add(i);
                    if c == 0 {
                        break;
                    }
                    name_buf[i] = c;
                    i += 1;
                }
            }
            let task_name = core::str::from_utf8(&name_buf)
                .unwrap_or("?")
                .trim_end_matches('\0');
            let free = unsafe { super::memory::esp_get_free_heap_size() };
            crate::println!(
                "wifi_task_create FAILED: name={} depth={} prio={} heap_free=0x{:X}",
                task_name,
                stack_depth,
                prio,
                free,
            );
            core::ptr::null_mut()
        }
    }
}

/// Delete a task.
#[no_mangle]
pub unsafe extern "C" fn __wifi_task_delete(task_handle: *mut c_void) {
    unsafe { task::vTaskDelete(task_handle) }
}

/// Yield from ISR context.
#[no_mangle]
pub unsafe extern "C" fn __wifi_task_yield_from_isr() {
    unsafe { port::portYIELD_FROM_ISR() }
}

/// Delay the current task.
#[no_mangle]
pub unsafe extern "C" fn __wifi_task_delay(tick: u32) {
    unsafe { task::vTaskDelay(tick) }
}

/// Return `configMAX_PRIORITIES`.
#[no_mangle]
pub unsafe extern "C" fn __wifi_task_get_max_priority() -> u32 {
    MAX_PRIORITIES
}

/// Convert milliseconds to tick count.
#[no_mangle]
pub unsafe extern "C" fn __wifi_task_ms_to_ticks(ms: u32) -> u32 {
    ms / PORT_TICK_RATE_MS
}

/// Suspend the scheduler.
#[no_mangle]
pub unsafe extern "C" fn __wifi_task_suspend_all() {
    unsafe {
        let trace = should_trace_sig10_sched();
        if trace {
            g_wifi_sched_stage = b's' as u32;
            g_wifi_sched_before = task::debug_scheduler_suspended_count();
            g_wifi_sched_ret = 0;
        }
        super::pp_stage::pm_bcn_mark_helper(1 << 5, b'S');
        task::vTaskSuspendAll();
        super::pp_stage::pm_bcn_mark_helper(1 << 21, b'S');
        if trace {
            g_wifi_sched_after = task::debug_scheduler_suspended_count();
            g_wifi_sched_suspend_calls = g_wifi_sched_suspend_calls.wrapping_add(1);
            g_wifi_sched_stage = b'S' as u32;
        }
    }
}

/// Resume the scheduler.
#[no_mangle]
pub unsafe extern "C" fn __wifi_task_resume_all() {
    unsafe {
        let trace = should_trace_sig10_sched();
        if trace {
            g_wifi_sched_stage = b'r' as u32;
            g_wifi_sched_before = task::debug_scheduler_suspended_count();
        }
        super::pp_stage::pm_bcn_mark_helper(1 << 6, b'T');
        let ret = task::xTaskResumeAll();
        super::pp_stage::pm_bcn_mark_helper(1 << 22, b'T');
        if trace {
            g_wifi_sched_after = task::debug_scheduler_suspended_count();
            g_wifi_sched_ret = ret as u32;
            g_wifi_sched_resume_calls = g_wifi_sched_resume_calls.wrapping_add(1);
            g_wifi_sched_stage = b'R' as u32;
        }
    }
}

/// Return the current task's saved stack pointer (top of stack).
#[no_mangle]
pub unsafe extern "C" fn __wifi_task_top_sp() -> *mut c_void {
    unsafe {
        if task::pxCurrentTCB.is_null() {
            return core::ptr::null_mut();
        }
        // pxCurrentTCB points to a TCB whose first field is stack_top
        let stack_top_ptr = task::pxCurrentTCB as *const *mut u8;
        let stack_top = *stack_top_ptr;
        if stack_top.is_null() {
            return core::ptr::null_mut();
        }

        if port::xPortInIsrContext() != 0 {
            trace_wifi_task_top_sp(stack_top as *const u8);
            return sdk_panic_frame_from_xtensa_rt(stack_top as *const u8);
        }

        stack_top as *mut c_void
    }
}

// ---------------------------------------------------------------------------
// Queue shims
// ---------------------------------------------------------------------------

/// Create a queue.
#[no_mangle]
pub unsafe extern "C" fn __wifi_queue_create(queue_len: u32, item_size: u32) -> *mut c_void {
    unsafe {
        let q = queue::xQueueCreate(queue_len, item_size);
        if !q.is_null() && queue_len == 0x4b && item_size == 8 {
            PP_QUEUE_PTR.store(q as usize, Ordering::Release);
        }
        q
    }
}

/// Delete a queue.
#[no_mangle]
pub unsafe extern "C" fn __wifi_queue_delete(q: *mut c_void) {
    unsafe { queue::vQueueDelete(q) }
}

/// Send an item to a queue.
///
/// `pos`: 0 = front, 1 = back, 2 = overwrite (OSI convention — note the
/// mapping is different from FreeRTOS internal convention).
#[no_mangle]
pub unsafe extern "C" fn __wifi_queue_send(
    q: *mut c_void,
    item: *mut c_void,
    block_time_tick: u32,
    pos: u32,
) -> i32 {
    unsafe {
        let os_pos = match pos {
            OSI_QUEUE_SEND_BACK => QUEUE_SEND_TO_BACK,
            OSI_QUEUE_SEND_FRONT => QUEUE_SEND_TO_FRONT,
            OSI_QUEUE_SEND_OVERWRITE => QUEUE_OVERWRITE,
            _ => QUEUE_SEND_TO_BACK,
        };

        let timeout = if block_time_tick == OSI_FUNCS_TIME_BLOCKING {
            PORT_MAX_DELAY
        } else {
            block_time_tick
        };

        let ret = queue::xQueueGenericSend(q, item as *const c_void, timeout, os_pos);
        if ret == 1 {
            1
        } else {
            0
        }
    }
}

/// Send from ISR context (never blocks).
#[no_mangle]
pub unsafe extern "C" fn __wifi_queue_send_from_isr(
    q: *mut c_void,
    item: *mut c_void,
    hptw: *mut i32,
) -> i32 {
    unsafe {
        let ret = queue::xQueueSendFromISR(q, item as *const c_void, hptw);
        if ret == 1 {
            1
        } else {
            0
        }
    }
}

/// Receive an item from a queue.
#[no_mangle]
pub unsafe extern "C" fn __wifi_queue_recv(
    q: *mut c_void,
    item: *mut c_void,
    block_time_tick: u32,
) -> i32 {
    unsafe {
        let timeout = if block_time_tick == OSI_FUNCS_TIME_BLOCKING {
            PORT_MAX_DELAY
        } else {
            block_time_tick
        };
        let is_pp_queue = PP_QUEUE_PTR.load(Ordering::Acquire) == q as usize;
        if is_pp_queue {
            set_queue_recv_stage(b'e');
        }

        let ret = queue::xQueueReceive(q, item, timeout);
        if is_pp_queue {
            set_queue_recv_stage(if ret == 1 { b'r' } else { b'f' });
            if ret == 1 && !item.is_null() {
                let sig = *(item as *const u32);
                super::pp_stage::reset_pp_stage_trace();
                g_pp_last_sig = sig;
                g_pp_last_arg = *((item as *const u32).add(1));
                if sig == 0x10 {
                    reset_sig10_sched_trace();
                }
            }
        }
        if ret == 1
            && PP_SIG_TRACE_REMAINING.load(Ordering::Acquire) != 0
            && is_pp_queue
            && !item.is_null()
        {
            set_queue_recv_stage(b's');
            let remaining = PP_SIG_TRACE_REMAINING.load(Ordering::Acquire);
            let sig = *(item as *const u32);
            match sig {
                4 => {
                    crate::logger::write_byte(b'4');
                    trace_pp_tx_head_summary();
                }
                5 => crate::logger::write_byte(b'5'),
                8 => crate::logger::write_byte(b'8'),
                9 => crate::logger::write_byte(b'9'),
                0x0b => crate::logger::write_byte(b'B'),
                0x0c => crate::logger::write_byte(b'C'),
                0x10 => crate::logger::write_byte(b'T'),
                _ => {
                    crate::logger::write_byte(b'X');
                    crate::logger::write_byte(hex_nibble((sig >> 4) as u8));
                    crate::logger::write_byte(hex_nibble(sig as u8));
                }
            }
            if sig == 5 {
                trace_pp_rx_head_summary();
            }
            PP_SIG_TRACE_REMAINING.store(remaining - 1, Ordering::Release);
            set_queue_recv_stage(b't');
        }
        if ret == 1 {
            if is_pp_queue {
                set_queue_recv_stage(b'z');
            }
            1
        } else {
            0
        }
    }
}

pub fn arm_pp_sig_trace(limit: u8) {
    PP_SIG_TRACE_REMAINING.store(limit, Ordering::Release);
    unsafe {
        g_queue_recv_stage = 0;
        super::pp_stage::g_pp_stage = 0;
        super::pp_stage::g_pp_stage_hits = 0;
    }
}

/// Return number of messages waiting in a queue.
#[no_mangle]
pub unsafe extern "C" fn __wifi_queue_msg_num(q: *mut c_void) -> u32 {
    unsafe { queue::uxQueueMessagesWaiting(q as *const c_void) }
}

// ---------------------------------------------------------------------------
// Timer shims
// ---------------------------------------------------------------------------

/// Create a software timer.
#[no_mangle]
pub unsafe extern "C" fn __wifi_timer_create(
    name: *const u8,
    period_ticks: u32,
    auto_load: bool,
    arg: *mut c_void,
    cb: unsafe extern "C" fn(*mut c_void),
) -> *mut c_void {
    unsafe { timer::xTimerCreate(name, period_ticks, if auto_load { 1 } else { 0 }, arg, cb) }
}

/// Reset (restart) a timer.
#[no_mangle]
pub unsafe extern "C" fn __wifi_timer_reset(tmr: *mut c_void, ticks: u32) -> i32 {
    unsafe { timer::xTimerReset(tmr, ticks) }
}

/// Stop a timer.
#[no_mangle]
pub unsafe extern "C" fn __wifi_timer_stop(tmr: *mut c_void, ticks: u32) -> i32 {
    unsafe { timer::xTimerStop(tmr, ticks) }
}

// ---------------------------------------------------------------------------
// Semaphore shims
// ---------------------------------------------------------------------------

/// Create a counting semaphore.
#[no_mangle]
pub unsafe extern "C" fn __wifi_semphr_create(max: u32, init: u32) -> *mut c_void {
    unsafe { semphr::xSemaphoreCreateCounting(max, init) }
}

/// Delete a semaphore.
#[no_mangle]
pub unsafe extern "C" fn __wifi_semphr_delete(sem: *mut c_void) {
    unsafe { semphr::vSemaphoreDelete(sem) }
}

/// Take (decrement) a semaphore.
#[no_mangle]
pub unsafe extern "C" fn __wifi_semphr_take(sem: *mut c_void, block_time_tick: u32) -> i32 {
    unsafe {
        let timeout = if block_time_tick == OSI_FUNCS_TIME_BLOCKING {
            PORT_MAX_DELAY
        } else {
            block_time_tick
        };
        semphr::xSemaphoreTake(sem, timeout)
    }
}

/// Give (increment) a semaphore.
#[no_mangle]
pub unsafe extern "C" fn __wifi_semphr_give(sem: *mut c_void) -> i32 {
    unsafe { semphr::xSemaphoreGive(sem) }
}
