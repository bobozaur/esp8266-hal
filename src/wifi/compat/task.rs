//! Preemptive round-robin task scheduler.
//!
//! Provides the FreeRTOS task symbols expected by the WiFi blobs and compiled
//! C code. Tasks that block (on delay, queue, semaphore) enter the `Blocked`
//! state and are woken either by timeout (tick ISR) or by direct notification
//! from the producer (queue send / semaphore give).
//!
//! The scheduler is driven by the CCOMPARE tick timer interrupt which calls
//! `xPortSysTickHandle` → `xTaskIncrementTick`, and uses `pxCurrentTCB` for
//! context switch decisions.

use core::ffi::{c_char, c_void};

use crate::ram;

use super::critical;
use super::memory;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of concurrent tasks. WiFi blobs create ~2 internal tasks
/// (ppTask=13, sys_evt=10), idle + timer daemon take 2, main takes 1 —
/// 6 slots: main(0), IDLE(1), TmrSvc(2), sys_evt(3), ppTask(4), +1 spare.
const MAX_TASKS: usize = 6;

/// Maximum priority levels. Must match `configMAX_PRIORITIES` (15) that the
/// SDK blob expects. The blob creates tasks at priorities 8, 10, and 12;
/// clamping to fewer levels collapses their scheduling relationships and
/// breaks priority-inheritance behavior.
pub(crate) const MAX_PRIORITIES: u32 = 15;

/// FreeRTOS pdPASS / pdTRUE.
pub(crate) const PD_PASS: i32 = 1;
/// FreeRTOS pdFALSE.
pub(crate) const PD_FALSE: i32 = 0;
/// FreeRTOS errCOULD_NOT_ALLOCATE_REQUIRED_MEMORY.
const ERR_NO_MEM: i32 = -1;

/// Thread-local storage slot count (matches `configNUM_THREAD_LOCAL_STORAGE_POINTERS`).
const TLS_SLOTS: usize = 3;
const TSK_STACK_FILL_BYTE: u8 = 0xA5;
const STACK_CANARY_WORD: u32 = 0xA5A5_A5A5;
const STACK_CANARY_WORDS: usize = 4;

/// Scheduler state constants (for `xTaskGetSchedulerState`).
const TASK_NOT_STARTED: i32 = 1;
const TASK_RUNNING: i32 = 2;
const TASK_SUSPENDED: i32 = 0;
const PORT_MAX_DELAY: u32 = 0xFFFF_FFFF;

// ---------------------------------------------------------------------------
// Task state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum TaskState {
    Free = 0,
    Ready = 1,
    Running = 2,
    Blocked = 3,
    Suspended = 4,
    /// Marked for deletion — stack freed by idle task, not immediately.
    Deleted = 5,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum WaitObjectKind {
    None = 0,
    QueueRx = 1,
    QueueTx = 2,
    EventGroup = 3,
}

/// Task Control Block.
///
/// **Critical:** `stack_top` MUST be the first field. The assembly code
/// (`_xt_int_exit`, `_xt_enter_first_task`) dereferences `pxCurrentTCB`
/// to load/store the saved stack pointer from offset 0 of the TCB.
#[repr(C)]
struct TaskControlBlock {
    /// Saved stack pointer (top of interrupt frame). **Must be offset 0.**
    stack_top: *mut u8,
    /// Bottom of the heap-allocated stack buffer.
    stack_base: *mut u8,
    /// Stack size in bytes.
    stack_size: usize,
    /// Task priority (0 = lowest, MAX_PRIORITIES-1 = highest).
    priority: u32,
    /// Base (original) priority — for priority inheritance.
    base_priority: u32,
    /// Current state.
    state: TaskState,
    /// Tick count at which a timeout-based block expires (0 = no timeout).
    delay_until: u64,
    /// Task name (null-terminated).
    name: [u8; 16],
    /// Thread-local storage pointers.
    tls: [*mut c_void; TLS_SLOTS],
    /// Mutex nesting count — for recursive mutex support.
    pub(crate) mutex_held_count: u32,
    /// Task entry point used by the task trampoline.
    entry: Option<unsafe extern "C" fn(*mut c_void)>,
    /// Original task parameter used by the task trampoline.
    param: *mut c_void,
}

// ---------------------------------------------------------------------------
// Global scheduler state
// ---------------------------------------------------------------------------

/// The task table.
static mut TASKS: [TaskControlBlock; MAX_TASKS] = {
    const EMPTY: TaskControlBlock = TaskControlBlock {
        stack_top: core::ptr::null_mut(),
        stack_base: core::ptr::null_mut(),
        stack_size: 0,
        priority: 0,
        base_priority: 0,
        state: TaskState::Free,
        delay_until: 0,
        name: [0; 16],
        tls: [core::ptr::null_mut(); TLS_SLOTS],
        mutex_held_count: 0,
        entry: None,
        param: core::ptr::null_mut(),
    };
    [EMPTY; MAX_TASKS]
};

/// SDK-style task frames live on each task's own stack.
///
/// `_xt_int_enter` / `_xt_int_exit` save and restore directly from
/// `pxCurrentTCB->stack_top`, so keep that same contract in Rust rather than
/// copying frames through a side buffer.
static mut PENDING_READY: [bool; MAX_TASKS] = [false; MAX_TASKS];
static mut PENDING_READY_ORDER: [usize; MAX_TASKS] = [usize::MAX; MAX_TASKS];
static mut PENDING_READY_LEN: usize = 0;
static mut DELETED_TASK_QUEUE: [usize; MAX_TASKS] = [usize::MAX; MAX_TASKS];
static mut DELETED_TASK_QUEUE_LEN: usize = 0;
static mut DELETED_TASKS_WAITING_CLEANUP: usize = 0;
static mut TASK_EVENT_WAIT_BITS: [u32; MAX_TASKS] = [0; MAX_TASKS];
static mut TASK_EVENT_WAIT_MATCHED: [bool; MAX_TASKS] = [false; MAX_TASKS];
static mut TASK_WAIT_KIND: [WaitObjectKind; MAX_TASKS] = [WaitObjectKind::None; MAX_TASKS];
static mut TASK_WAIT_OBJECT: [*mut c_void; MAX_TASKS] = [core::ptr::null_mut(); MAX_TASKS];
static mut TASK_MARKERS: [u8; MAX_TASKS] = [b'?'; MAX_TASKS];

/// Pointer to the current task's TCB (as `*mut c_void`).
///
/// Assembly code dereferences this to get `(*pxCurrentTCB).stack_top`.
#[no_mangle]
pub static mut pxCurrentTCB: *mut c_void = core::ptr::null_mut();

/// Index of the currently running task in `TASKS`.
static mut CURRENT_TASK: usize = 0;

/// Global tick counter (64-bit avoids overflow for ~5.8 billion years at 100Hz).
static mut TICK_COUNT: u64 = 0;

/// Scheduler suspended counter (vTaskSuspendAll / xTaskResumeAll nesting).
static mut SCHEDULER_SUSPENDED: u32 = 0;

/// Ticks that elapsed while the scheduler was suspended.
///
/// FreeRTOS tracks pended ticks and replays them in xTaskResumeAll() so task
/// delays and timer deadlines do not silently stop advancing during a suspend
/// window.
static mut PENDED_TICKS: u32 = 0;

/// Yield pending flag (set when scheduler is suspended but a switch is needed).
static mut YIELD_PENDING: bool = false;

/// Whether the scheduler has been started.
static mut SCHEDULER_STARTED: bool = false;
static mut STACK_CANARY_REPORTED: [bool; MAX_TASKS] = [false; MAX_TASKS];
#[no_mangle]
pub static mut g_ui_zero_save_pc: u32 = 0;
#[no_mangle]
pub static mut g_first_zero_save_task: u32 = 0;
#[no_mangle]
pub static mut g_first_zero_save_pc: u32 = 0;
#[no_mangle]
pub static mut g_first_zero_save_a0: u32 = 0;
#[no_mangle]
pub static mut g_first_zero_restore_task: u32 = 0;
#[no_mangle]
pub static mut g_first_zero_restore_a0: u32 = 0;
#[no_mangle]
pub static mut g_first_save_owner: u32 = 0;
#[no_mangle]
pub static mut g_first_save_current: u32 = 0;
#[no_mangle]
pub static mut g_first_save_mismatch_pc: u32 = 0;
#[no_mangle]
pub static mut g_first_save_mismatch_a0: u32 = 0;
#[no_mangle]
pub static mut g_wait_stage: u32 = 0;
#[no_mangle]
pub static mut g_wait_task: u32 = 0;
#[no_mangle]
pub static mut g_tick_stage: u32 = 0;
#[no_mangle]
pub static mut g_tick_scan_task: u32 = 0;
#[no_mangle]
pub static mut g_cancel_wait_task: u32 = 0;
#[no_mangle]
pub static mut g_cancel_wait_kind: u32 = 0;
#[no_mangle]
pub static mut g_cancel_wait_object: u32 = 0;
#[no_mangle]
pub static mut g_idle_stage: u32 = 0;
#[no_mangle]
pub static mut g_switch_stage: u32 = 0;
#[no_mangle]
pub static mut g_switch_from_task: u32 = 0;
#[no_mangle]
pub static mut g_switch_to_task: u32 = 0;
#[no_mangle]
pub static mut g_last_saved_task: u32 = 0;
#[no_mangle]
pub static mut g_last_saved_pc: u32 = 0;
#[no_mangle]
pub static mut g_last_saved_a0: u32 = 0;
#[no_mangle]
pub static mut g_last_saved_a1: u32 = 0;
#[no_mangle]
pub static mut g_last_saved_a14: u32 = 0;
#[no_mangle]
pub static mut g_last_saved_a15: u32 = 0;
#[no_mangle]
pub static mut g_last_saved_frame_ptr: u32 = 0;
#[no_mangle]
pub static mut g_last_saved_ret_slot: u32 = 0;
#[no_mangle]
pub static mut g_last_saved_ret_slot_4: u32 = 0;
#[no_mangle]
pub static mut g_last_saved_ret_slot_212: u32 = 0;
#[no_mangle]
pub static mut g_last_saved_ret_slot_284: u32 = 0;
#[no_mangle]
pub static mut g_last_restored_task: u32 = 0;
#[no_mangle]
pub static mut g_last_restored_pc: u32 = 0;
#[no_mangle]
pub static mut g_last_restored_a0: u32 = 0;
#[no_mangle]
pub static mut g_last_restored_a1: u32 = 0;
#[no_mangle]
pub static mut g_last_restored_a14: u32 = 0;
#[no_mangle]
pub static mut g_last_restored_a15: u32 = 0;
#[no_mangle]
pub static mut g_last_restored_frame_ptr: u32 = 0;
#[no_mangle]
pub static mut g_last_restored_ret_slot: u32 = 0;
#[no_mangle]
pub static mut g_last_restored_ret_slot_aux1: u32 = 0;
#[no_mangle]
pub static mut g_last_restored_ret_slot_aux2: u32 = 0;
#[no_mangle]
pub static mut g_last_restored_ret_slot_aux3: u32 = 0;
#[no_mangle]
pub static mut g_last_restored_ret_slot_4: u32 = 0;
#[no_mangle]
pub static mut g_last_restored_ret_slot_212: u32 = 0;
#[no_mangle]
pub static mut g_last_restored_ret_slot_284: u32 = 0;
#[no_mangle]
pub static mut g_last_idle_saved_pc: u32 = 0;
#[no_mangle]
pub static mut g_last_idle_saved_a0: u32 = 0;
#[no_mangle]
pub static mut g_pending_ready_len_dbg: u32 = 0;
#[no_mangle]
pub static mut g_deleted_task_queue_len_dbg: u32 = 0;
static mut FIRST_DELETE_TRACE_DONE: bool = false;
static mut FIRST_CLEANUP_TRACE_DONE: bool = false;
static mut FIRST_UI_TICK_DECISION_TRACE_DONE: bool = false;
static mut FIRST_ZERO_PC_RESTORE_TRACE_DONE: bool = false;
static mut FIRST_PENDING_READY_LEN_CLAMP_TRACE_DONE: bool = false;
static mut FIRST_DELETED_QUEUE_LEN_CLAMP_TRACE_DONE: bool = false;
static mut FIRST_IDLE_ZERO_A0_SAVE_TRACE_DONE: bool = false;
static mut FIRST_UI_RESTORE_TRACE_DONE: bool = false;

// ---------------------------------------------------------------------------
// Stack initialisation
// ---------------------------------------------------------------------------

/// Stack frame constants matching xtensa-lx-rt's ESP8266 exception frame.
pub(crate) const XT_STK_PC: usize = 0x00;
pub(crate) const XT_STK_PS: usize = 0x04;
pub(crate) const XT_STK_A0: usize = 0x08;
pub(crate) const XT_STK_A1: usize = 0x0C;
pub(crate) const XT_STK_A2: usize = 0x10;
pub(crate) const XT_STK_A14: usize = 0x40;
pub(crate) const XT_STK_A15: usize = 0x44;
pub(crate) const XT_STK_FRMSZ: usize = 0x100;
/// ESP8266 RTOS SDK call0 interrupt/task frame size from
/// `freertos/xtensa_context.h` with the ESP8266 config headers.
const SDK_XT_STK_FRMSZ: usize = 0x50;
/// Extra bytes each compat-created task needs so an SDK stack-depth request
/// preserves the same usable headroom under the larger xtensa-lx-rt frame.
const COMPAT_STACK_FRAME_DELTA: usize = XT_STK_FRMSZ - SDK_XT_STK_FRMSZ;
pub(crate) const PS_UM: u32 = 0x20;
pub(crate) const PS_EXCM: u32 = 0x10;

#[inline(always)]
fn is_resume_all_and_maybe_yield_pc(pc: u32) -> bool {
    let resume_all_pc = super::queue::resume_all_and_maybe_yield as usize as u32;
    pc.wrapping_sub(resume_all_pc) < 0x100
}

#[inline(always)]
fn is_x_task_resume_all_pc(pc: u32) -> bool {
    let task_resume_all_pc = xTaskResumeAll as usize as u32;
    pc.wrapping_sub(task_resume_all_pc) < 0x180
}

#[inline(always)]
fn special_return_probe_offsets(pc: u32) -> Option<(usize, usize, usize, usize)> {
    if is_resume_all_and_maybe_yield_pc(pc) {
        // Current-image disassembly:
        // - `resume_all_and_maybe_yield()` uses a 32-byte frame and saves A0 at `[A1 + 16]`
        // - its caller `queue_receive_impl()` uses a 224-byte frame and saves A0 at `+212`
        // - `__wifi_queue_recv()` calls `queue_receive_impl()` directly in the active image
        //   and uses a 32-byte frame with A0 saved at `+16`
        // - `ppTask()` uses a 64-byte frame and saves A0 at `+60`
        // Relative to the current `resume_all_and_maybe_yield()` frame base:
        // - immediate caller slot      = +16
        // - `queue_receive_impl` slot  = +32 + 212 = +244
        // - `__wifi_queue_recv` slot   = +32 + 224 + 16 = +272
        // - `ppTask` slot              = +32 + 224 + 32 + 60 = +348
        Some((16, 244, 272, 348))
    } else if is_x_task_resume_all_pc(pc) {
        // Current-image disassembly:
        // - `xTaskResumeAll()` uses a 32-byte frame and saves A0 at `[A1 + 24]`
        // - its caller `resume_all_and_maybe_yield()` uses a 32-byte frame and saves A0 at `+16`
        // - `queue_receive_impl()` uses a 224-byte frame and saves A0 at `+212`
        // - `__wifi_queue_recv()` calls `queue_receive_impl()` directly in the active image
        //   and uses a 32-byte frame with A0 saved at `+16`
        // - `ppTask()` uses a 64-byte frame and saves A0 at `+60`
        // Relative to the current `xTaskResumeAll()` frame base:
        // - immediate caller slot            = +24
        // - caller `resume_all` slot         = +32 + 16 = +48
        // - `queue_receive_impl` slot        = +32 + 32 + 212 = +276
        // - `__wifi_queue_recv` caller slot  = +32 + 32 + 224 + 16 = +304
        Some((24, 48, 276, 304))
    } else {
        None
    }
}

#[inline(always)]
unsafe fn read_probed_return_slot(stack_base: usize, stack_end: usize, pc: u32, a1: u32) -> u32 {
    unsafe {
        if stack_base == 0 || (a1 as usize) < stack_base || (a1 as usize) >= stack_end {
            return 0;
        }

        let Some((slot_offset, _, _, _)) = special_return_probe_offsets(pc) else {
            return 0;
        };

        let slot_addr = (a1 as usize).saturating_add(slot_offset);
        if slot_addr.saturating_add(core::mem::size_of::<u32>()) > stack_end {
            0
        } else {
            (slot_addr as *const u32).read_volatile()
        }
    }
}

#[inline(always)]
unsafe fn read_stack_slot(stack_base: usize, stack_end: usize, a1: u32, slot_offset: usize) -> u32 {
    unsafe {
        if stack_base == 0 || (a1 as usize) < stack_base || (a1 as usize) >= stack_end {
            return 0;
        }

        let slot_addr = (a1 as usize).saturating_add(slot_offset);
        if slot_addr.saturating_add(core::mem::size_of::<u32>()) > stack_end {
            0
        } else {
            (slot_addr as *const u32).read_volatile()
        }
    }
}

unsafe fn init_stack_canary(stack_base: *mut u8, stack_size: usize) {
    unsafe {
        if stack_base.is_null() || stack_size < STACK_CANARY_WORDS * core::mem::size_of::<u32>() {
            return;
        }
        let p = stack_base as *mut u32;
        for i in 0..STACK_CANARY_WORDS {
            p.add(i).write_volatile(STACK_CANARY_WORD);
        }
    }
}

unsafe fn init_stack_fill(stack_base: *mut u8, stack_size: usize) {
    unsafe {
        if stack_base.is_null() || stack_size == 0 {
            return;
        }

        core::ptr::write_bytes(stack_base, TSK_STACK_FILL_BYTE, stack_size);
    }
}

#[ram]
unsafe fn stack_canary_corrupted(idx: usize) -> bool {
    unsafe {
        if idx >= MAX_TASKS || TASKS[idx].stack_base.is_null() {
            return false;
        }
        let p = TASKS[idx].stack_base as *const u32;
        for i in 0..STACK_CANARY_WORDS {
            if p.add(i).read_volatile() != STACK_CANARY_WORD {
                return true;
            }
        }
        false
    }
}

unsafe fn stack_high_water_bytes(idx: usize) -> u32 {
    unsafe {
        if idx >= MAX_TASKS || TASKS[idx].stack_base.is_null() || TASKS[idx].stack_size == 0 {
            return 0;
        }

        let mut used_fill = 0usize;
        while used_fill < TASKS[idx].stack_size
            && TASKS[idx].stack_base.add(used_fill).read_volatile() == TSK_STACK_FILL_BYTE
        {
            used_fill += 1;
        }

        used_fill as u32
    }
}

#[inline(never)]
#[ram]
unsafe fn report_stack_canary_once(idx: usize) {
    unsafe {
        if idx >= MAX_TASKS || STACK_CANARY_REPORTED[idx] || !stack_canary_corrupted(idx) {
            return;
        }
        STACK_CANARY_REPORTED[idx] = true;

        let marker = match TASK_MARKERS[idx] {
            b'U' => b'w',
            b'S' => b'y',
            b'P' => b'z',
            b'I' => b'i',
            _ => b'x',
        };
        crate::logger::debug_byte(marker);

        let task = TASKS.as_mut_ptr().add(idx);
        super::port::vApplicationStackOverflowHook(task.cast::<c_void>(), (*task).name.as_ptr());
    }
}

fn marker_from_task_name(name: &[u8; 16]) -> u8 {
    match &name[..] {
        [b'u', b'i', b'T', ..] => b'U',
        [b's', b'y', b's', ..] => b'S',
        [b'p', b'p', b'T', ..] => b'P',
        [b'w', b'p', b's', b'T', ..] => b'W',
        [b'T', b'm', b'r', ..] => b'M',
        [b'I', b'D', b'L', b'E', ..] => b'I',
        [b'm', b'a', b'i', b'n', ..] => b'B',
        _ => b'?',
    }
}

fn task_name_marker(idx: usize) -> u8 {
    unsafe {
        if idx < MAX_TASKS {
            TASK_MARKERS[idx]
        } else {
            b'?'
        }
    }
}

fn task_state_marker(state: TaskState) -> u8 {
    match state {
        TaskState::Free => b'F',
        TaskState::Ready => b'R',
        TaskState::Running => b'N',
        TaskState::Blocked => b'B',
        TaskState::Suspended => b'S',
        TaskState::Deleted => b'D',
    }
}

pub(crate) unsafe fn record_wait_stage(stage: u8, task_idx: usize) {
    unsafe {
        g_wait_stage = stage as u32;
        g_wait_task = if task_idx < MAX_TASKS {
            task_name_marker(task_idx) as u32
        } else {
            b'?' as u32
        };
    }
}

/// Return a compact marker for the task whose stack owns `sp`.
pub fn debug_task_marker_for_sp(sp: u32) -> u8 {
    unsafe {
        let sp = sp as usize;
        for idx in 0..MAX_TASKS {
            let base = TASKS[idx].stack_base as usize;
            let size = TASKS[idx].stack_size;
            if base == 0 || size == 0 {
                continue;
            }
            if sp >= base && sp < base + size {
                return task_name_marker(idx);
            }
        }
        b'?'
    }
}

/// Return a compact marker for the scheduler's current task slot.
pub fn debug_current_task_marker() -> u8 {
    unsafe {
        if CURRENT_TASK < MAX_TASKS {
            task_name_marker(CURRENT_TASK)
        } else {
            b'?'
        }
    }
}

/// Return a SDK-style stack high-water mark for the given task marker.
///
/// The value is returned in `StackType_t` items; on ESP8266 that means bytes.
pub fn debug_task_stack_high_water_bytes(marker: u8) -> u32 {
    unsafe {
        for idx in 0..MAX_TASKS {
            if task_name_marker(idx) == marker {
                return stack_high_water_bytes(idx);
            }
        }

        0
    }
}

pub fn debug_task_stack_size_bytes(marker: u8) -> u32 {
    unsafe {
        for idx in 0..MAX_TASKS {
            if task_name_marker(idx) == marker {
                return TASKS[idx].stack_size as u32;
            }
        }

        0
    }
}

pub fn debug_task_stack_bounds(marker: u8) -> (u32, u32) {
    unsafe {
        for idx in 0..MAX_TASKS {
            if task_name_marker(idx) == marker {
                let base = TASKS[idx].stack_base as usize;
                let size = TASKS[idx].stack_size;
                if base != 0 && size != 0 {
                    let end = base.saturating_add(size);
                    return (base as u32, end as u32);
                }
            }
        }

        (0, 0)
    }
}

pub fn debug_current_task_stack_bounds() -> (u32, u32) {
    debug_task_stack_bounds(debug_current_task_marker())
}

pub fn debug_task_stack_canary_corrupted(marker: u8) -> bool {
    unsafe {
        for idx in 0..MAX_TASKS {
            if task_name_marker(idx) == marker {
                return stack_canary_corrupted(idx);
            }
        }

        false
    }
}

#[inline(never)]
fn ready_or_running_count_at_priority(priority: u32) -> usize {
    unsafe {
        let mut count = 0usize;
        let mut idx = 0usize;
        while idx < MAX_TASKS {
            if TASKS[idx].priority != priority {
                idx += 1;
                continue;
            }

            if TASKS[idx].state == TaskState::Ready
                || (idx == CURRENT_TASK && TASKS[idx].state == TaskState::Running)
            {
                count += 1;
            }

            idx += 1;
        }

        count
    }
}

#[inline(never)]
fn top_ready_or_running_priority() -> u32 {
    unsafe {
        let mut top = 0u32;
        let mut idx = 0usize;
        while idx < MAX_TASKS {
            if TASKS[idx].state == TaskState::Ready
                || (idx == CURRENT_TASK && TASKS[idx].state == TaskState::Running)
            {
                top = top.max(TASKS[idx].priority);
            }

            idx += 1;
        }

        top
    }
}

#[inline(always)]
fn another_idle_priority_task_ready() -> bool {
    unsafe {
        let mut idx = 0usize;
        while idx < MAX_TASKS {
            if idx != CURRENT_TASK
                && TASKS[idx].priority == 0
                && TASKS[idx].state == TaskState::Ready
            {
                return true;
            }

            idx += 1;
        }

        false
    }
}

#[inline(never)]
fn find_idle_task_index() -> Option<usize> {
    unsafe {
        let mut idx = 0usize;
        while idx < MAX_TASKS {
            if TASK_MARKERS[idx] == b'I'
                && TASKS[idx].state != TaskState::Free
                && TASKS[idx].state != TaskState::Deleted
            {
                return Some(idx);
            }

            idx += 1;
        }

        None
    }
}

#[inline(always)]
unsafe fn consider_ready_candidate(idx: usize, best_idx: &mut usize, best_prio: &mut u32) {
    unsafe {
        if TASKS[idx].state == TaskState::Ready
            && (*best_idx == usize::MAX || TASKS[idx].priority > *best_prio)
        {
            *best_idx = idx;
            *best_prio = TASKS[idx].priority;
        }
    }
}

fn init_stack_common(stack_top: *mut u8, pc: u32, a0: u32, a2: u32) -> *mut u8 {
    unsafe {
        let frame_top = stack_top.add(4) as usize;
        let sp = ((frame_top - XT_STK_FRMSZ) & !0xF) as *mut u32;

        core::ptr::write_bytes(sp as *mut u8, 0, XT_STK_FRMSZ);

        let set = |offset: usize, val: u32| {
            sp.add(offset >> 2).write(val);
        };

        set(XT_STK_PC, pc);
        set(XT_STK_A0, a0);
        set(XT_STK_A1, sp as u32 + XT_STK_FRMSZ as u32);
        set(XT_STK_A2, a2);
        set(XT_STK_PS, PS_UM | PS_EXCM);

        sp as *mut u8
    }
}

/// Build an initial xtensa-lx-rt exception frame for a direct entrypoint.
pub(crate) unsafe fn init_stack(
    stack_top: *mut u8,
    entry: unsafe extern "C" fn(*mut c_void),
    param: *mut c_void,
) -> *mut u8 {
    init_stack_common(stack_top, entry as usize as u32, 0, param as u32)
}

/// Build an initial frame for a compat-created task using the SDK's direct
/// entry contract: first restore returns straight into the task entrypoint with
/// `A2 = param` and `A0 = 0`.
pub(crate) unsafe fn init_task_stack(
    stack_top: *mut u8,
    entry: unsafe extern "C" fn(*mut c_void),
    param: *mut c_void,
) -> *mut u8 {
    unsafe { init_stack(stack_top, entry, param) }
}

/// Copy the ISR frame into the current task's per-task save buffer.
#[inline(never)]
#[ram]
pub(crate) unsafe fn save_current_frame(frame: *mut u8) {
    unsafe {
        let idx = CURRENT_TASK;
        if idx >= MAX_TASKS {
            return;
        }
        super::port::g_ctx_stage = b'v' as u32;

        let tcb = TASKS.as_mut_ptr().add(idx);
        let current_marker = *TASK_MARKERS.as_ptr().add(idx);
        let stack_base = (*tcb).stack_base as usize;
        let stack_end = stack_base.saturating_add((*tcb).stack_size);

        (*tcb).stack_top = frame;
        let live_pc = (frame.add(XT_STK_PC) as *const u32).read_volatile();
        let a0 = (frame.add(XT_STK_A0) as *const u32).read_volatile();
        let a1 = (frame.add(XT_STK_A1) as *const u32).read_volatile();
        let probed_ret = read_probed_return_slot(stack_base, stack_end, live_pc, a1);
        let ret_slot_4 = read_stack_slot(stack_base, stack_end, a1, 4);
        let ret_slot_212 = read_stack_slot(stack_base, stack_end, a1, 212);
        let ret_slot_284 = read_stack_slot(stack_base, stack_end, a1, 284);
        super::port::g_ctx_stage = b'w' as u32;

        g_last_saved_task = current_marker as u32;
        g_last_saved_pc = live_pc;
        g_last_saved_a0 = a0;
        g_last_saved_a1 = a1;
        g_last_saved_frame_ptr = frame as u32;
        g_last_saved_ret_slot = probed_ret;
        g_last_saved_ret_slot_4 = ret_slot_4;
        g_last_saved_ret_slot_212 = ret_slot_212;
        g_last_saved_ret_slot_284 = ret_slot_284;
        if current_marker == b'I' {
            g_last_idle_saved_pc = live_pc;
            g_last_idle_saved_a0 = a0;
        }

        super::port::g_ctx_stage = b'x' as u32;
    }
}

/// Copy the selected task's saved frame into the current live interrupt frame
/// so the outer `xtensa-lx-rt` restore returns into the chosen task.
#[inline(never)]
#[ram]
pub(crate) unsafe fn restore_current_frame_into(frame: *mut u8) {
    unsafe {
        if frame.is_null() || CURRENT_TASK >= MAX_TASKS {
            return;
        }

        let saved_frame = TASKS[CURRENT_TASK].stack_top;
        if saved_frame.is_null() {
            return;
        }

        core::ptr::copy_nonoverlapping(saved_frame, frame, XT_STK_FRMSZ);
    }
}

/// Mirror the SDK `_xt_int_enter` contract for ordinary interrupts:
/// `pxCurrentTCB->stack_top` tracks the live interrupt frame base even when
/// no compat task switch is requested in that interrupt.
#[inline(always)]
pub(crate) unsafe fn note_interrupt_frame(frame: *mut u8) {
    unsafe {
        let idx = CURRENT_TASK;
        if idx >= MAX_TASKS {
            return;
        }

        let tcb = TASKS.as_mut_ptr().add(idx);
        (*tcb).stack_top = frame;
    }
}

/// Commit `pxCurrentTCB->stack_top` to the post-restore SP of the frame the
/// level-1 exit path has already chosen to restore.
#[no_mangle]
#[ram]
pub unsafe extern "C" fn commit_current_task_post_restore_stack_top(frame: *mut u8) -> *mut u8 {
    unsafe {
        let idx = CURRENT_TASK;
        if idx >= MAX_TASKS || frame.is_null() {
            return frame;
        }

        let restored_a1 = (frame.add(XT_STK_A1) as *const u32).read_volatile();
        let tcb = TASKS.as_mut_ptr().add(idx);
        (*tcb).stack_top = restored_a1 as *mut u8;
        frame
    }
}

/// Prepare the selected task's own saved frame for restore and return its
/// frame pointer for the level-1 exit path.
#[inline(never)]
#[ram]
pub(crate) unsafe fn prepare_current_frame_for_restore() -> *mut u8 {
    unsafe {
        let idx = CURRENT_TASK;
        if idx >= MAX_TASKS {
            return core::ptr::null_mut();
        }

        let tcb = TASKS.as_mut_ptr().add(idx);
        let stack_base = (*tcb).stack_base as usize;
        let stack_end = stack_base.saturating_add((*tcb).stack_size);
        let frame_ptr = (*tcb).stack_top;
        if frame_ptr.is_null() {
            return core::ptr::null_mut();
        }
        let restored_pc = (frame_ptr.add(XT_STK_PC) as *const u32).read_volatile();
        let restored_a0 = (frame_ptr.add(XT_STK_A0) as *const u32).read_volatile();
        let restored_a1 = (frame_ptr.add(XT_STK_A1) as *const u32).read_volatile();
        let probed_ret = read_probed_return_slot(stack_base, stack_end, restored_pc, restored_a1);
        let ret_slot_4 = read_stack_slot(stack_base, stack_end, restored_a1, 4);
        let ret_slot_212 = read_stack_slot(stack_base, stack_end, restored_a1, 212);
        let ret_slot_284 = read_stack_slot(stack_base, stack_end, restored_a1, 284);
        let current_marker = *TASK_MARKERS.as_ptr().add(idx);

        g_last_restored_task = current_marker as u32;
        g_last_restored_pc = restored_pc;
        g_last_restored_a0 = restored_a0;
        g_last_restored_a1 = restored_a1;
        g_last_restored_frame_ptr = frame_ptr as u32;
        g_last_restored_ret_slot = probed_ret;
        g_last_restored_ret_slot_4 = ret_slot_4;
        g_last_restored_ret_slot_212 = ret_slot_212;
        g_last_restored_ret_slot_284 = ret_slot_284;

        if restored_pc == 0 && g_first_zero_restore_task == 0 {
            g_first_zero_restore_task = current_marker as u32;
            g_first_zero_restore_a0 = restored_a0;
        }

        frame_ptr
    }
}

// ---------------------------------------------------------------------------
// Idle task
// ---------------------------------------------------------------------------

/// Idle task — runs at priority 0 and follows the SDK hook split:
/// cleanup locally, then call the idle hooks from `port.rs`.
unsafe extern "C" fn idle_task(_param: *mut c_void) {
    loop {
        unsafe {
            g_idle_stage = b'c' as u32;
            cleanup_deleted_tasks();
        }

        // Match SDK `prvIdleTask()` with `configIDLE_SHOULD_YIELD == 1`:
        // if another idle-priority task is ready, yield before the timeslice
        // ends instead of spinning in this task.
        unsafe {
            g_idle_stage = b'y' as u32;
        }
        if another_idle_priority_task_ready() {
            unsafe {
                super::port::PendSV(1);
            }
        }

        unsafe {
            g_idle_stage = b'h' as u32;
            super::port::vApplicationIdleHook();
        }

        unsafe {
            g_idle_stage = b'p' as u32;
            super::port::esp_internal_idle_hook();
        }
    }
}

/// Free stacks of tasks in the `Deleted` state.
unsafe fn cleanup_deleted_tasks() {
    unsafe {
        while DELETED_TASKS_WAITING_CLEANUP > 0 {
            critical::vPortEnterCritical();
            let idx = pop_deleted_task_locked();
            critical::vPortExitCritical();

            if let Some(idx) = idx {
                if !FIRST_CLEANUP_TRACE_DONE {
                    FIRST_CLEANUP_TRACE_DONE = true;
                    crate::logger::debug_byte(b'Z');
                    crate::logger::debug_byte(debug_current_task_marker());
                    crate::logger::debug_byte(task_name_marker(idx));
                    crate::logger::debug_byte(task_state_marker(TASKS[idx].state));
                }
                destroy_task_slot(idx);
            } else {
                break;
            }
        }
    }
}

/// Register the current CPU context as the "main task" in the scheduler.
///
/// This is the key to making `esp_wifi_init` work: by making the caller's
/// context a schedulable task, `vTaskDelay` / `xSemaphoreTake` inside the
/// WiFi blob can properly yield (PendSV saves the main context and switches
/// to blob-created tasks, then switches back when they yield).
///
/// Matches the `allocate_main_task()` pattern from ESP32's `esp-rtos`.
///
/// # Safety
///
/// Must be called exactly once, before any task creation or scheduler start.
/// The caller's stack must remain valid for the lifetime of the scheduler.
pub(crate) unsafe fn allocate_main_task() {
    unsafe {
        // Use slot 0 for the main task.
        let tcb = &mut TASKS[0];
        tcb.stack_top = core::ptr::null_mut(); // filled by first context switch
        tcb.stack_base = core::ptr::null_mut(); // not heap-allocated
        tcb.stack_size = 0; // unknown (it's the real CPU stack)
        tcb.priority = 1; // just above idle (0), below WiFi tasks (8-12)
        tcb.base_priority = 1;
        tcb.state = TaskState::Running; // we ARE running right now
        tcb.delay_until = 0;
        tcb.name = [0; 16];
        tcb.name[..4].copy_from_slice(b"main");
        tcb.tls = [core::ptr::null_mut(); TLS_SLOTS];
        tcb.mutex_held_count = 0;
        tcb.entry = None;
        tcb.param = core::ptr::null_mut();
        TASK_MARKERS[0] = marker_from_task_name(&tcb.name);
        TASK_WAIT_KIND[0] = WaitObjectKind::None;
        TASK_WAIT_OBJECT[0] = core::ptr::null_mut();

        CURRENT_TASK = 0;
        PENDING_READY[0] = false;
        pxCurrentTCB = &mut TASKS[0] as *mut TaskControlBlock as *mut c_void;
    }
}

/// Retire the current compat-only external task registration.
///
/// This is used for the temporary boot-thread slot created by
/// `allocate_main_task()`: it has no owned heap stack and exists only so the
/// compat scheduler can switch away from the caller once `uiT` has been
/// created. Unlike a normal FreeRTOS task, there is nothing for the idle task
/// to reclaim later.
unsafe fn retire_current_unowned_task_inner(request_switch: bool) {
    unsafe {
        critical::vPortEnterCritical();

        let idx = CURRENT_TASK;
        if idx < MAX_TASKS && TASKS[idx].stack_base.is_null() && TASKS[idx].stack_size == 0 {
            clear_pending_ready_locked(idx);
            TASKS[idx].state = TaskState::Free;
            TASKS[idx].delay_until = 0;
            TASKS[idx].priority = 0;
            TASKS[idx].base_priority = 0;
            TASKS[idx].stack_top = core::ptr::null_mut();
            TASKS[idx].name = [0; 16];
            TASKS[idx].tls = [core::ptr::null_mut(); TLS_SLOTS];
            TASKS[idx].mutex_held_count = 0;
            TASKS[idx].entry = None;
            TASKS[idx].param = core::ptr::null_mut();
            TASK_EVENT_WAIT_BITS[idx] = 0;
            TASK_EVENT_WAIT_MATCHED[idx] = false;
            PENDING_READY[idx] = false;
            TASK_MARKERS[idx] = b'?';
            STACK_CANARY_REPORTED[idx] = false;
        }

        critical::vPortExitCritical();
        if request_switch {
            super::port::PendSV(1);
        }
    }
}

pub(crate) unsafe fn retire_current_unowned_task() {
    unsafe { retire_current_unowned_task_inner(true) }
}

pub(crate) unsafe fn retire_current_unowned_task_for_first_switch() {
    unsafe { retire_current_unowned_task_inner(false) }
}

/// Create the idle task. Called once from `xPortStartScheduler`.
pub(crate) unsafe fn create_idle_task() {
    unsafe {
        xTaskCreate(
            idle_task,
            b"IDLE\0".as_ptr() as *const _,
            512,
            core::ptr::null_mut(),
            0, // lowest priority
            core::ptr::null_mut(),
        );
    }
}

// ---------------------------------------------------------------------------
// FreeRTOS API: task creation
// ---------------------------------------------------------------------------

/// `xTaskCreate` — create a new task and add it to the ready list.
#[no_mangle]
pub unsafe extern "C" fn xTaskCreate(
    task_code: unsafe extern "C" fn(*mut c_void),
    name: *const c_char,
    stack_depth: u16,
    parameters: *mut c_void,
    priority: u32,
    created_task: *mut *mut c_void,
) -> i32 {
    unsafe {
        let mut should_yield = false;
        critical::vPortEnterCritical();

        let slot = match find_free_slot() {
            Some(s) => s,
            None => {
                critical::vPortExitCritical();
                return ERR_NO_MEM;
            }
        };

        // On the ESP8266 RTOS SDK port `StackType_t = uint8_t`, so task stack
        // depths are already byte-counted. Preserve SDK-equivalent usable
        // stack by allocating the requested bytes plus the larger live
        // xtensa-lx-rt frame delta.
        let requested_stack_bytes = stack_depth as usize;
        let stack_bytes = requested_stack_bytes + COMPAT_STACK_FRAME_DELTA;
        let stack_base = memory::malloc(stack_bytes) as *mut u8;
        if stack_base.is_null() {
            critical::vPortExitCritical();
            return ERR_NO_MEM;
        }
        init_stack_fill(stack_base, stack_bytes);
        init_stack_canary(stack_base, stack_bytes);

        let prio = priority.min(MAX_PRIORITIES - 1);
        let tcb = &mut TASKS[slot];
        let stack_top = stack_base.add(stack_bytes - 4);
        tcb.entry = Some(task_code);
        tcb.param = parameters;
        let sp = init_task_stack(stack_top, task_code, parameters);

        if slot == 3 {
            super::memory::monitor_task3_stack(stack_base, stack_bytes, sp);
        }

        tcb.stack_top = sp;

        tcb.stack_base = stack_base;
        tcb.stack_size = stack_bytes;
        tcb.priority = prio;
        tcb.base_priority = prio;
        tcb.state = TaskState::Ready;
        tcb.delay_until = 0;
        PENDING_READY[slot] = false;
        TASK_WAIT_KIND[slot] = WaitObjectKind::None;
        TASK_WAIT_OBJECT[slot] = core::ptr::null_mut();
        tcb.tls = [core::ptr::null_mut(); TLS_SLOTS];
        tcb.mutex_held_count = 0;

        // Copy name
        tcb.name = [0; 16];
        if !name.is_null() {
            let mut i = 0;
            while i < 15 {
                let c = *name.add(i);
                if c == 0 {
                    break;
                }
                tcb.name[i] = c as u8;
                i += 1;
            }
        }
        TASK_MARKERS[slot] = marker_from_task_name(&tcb.name);

        // Debug: log task creation
        crate::wifi_debugln!(
            "xTaskCreate: [{}] prio={} name={}",
            slot,
            prio,
            core::str::from_utf8(&tcb.name)
                .unwrap_or("?")
                .trim_end_matches('\0')
        );

        if !created_task.is_null() {
            *created_task = tcb as *mut TaskControlBlock as *mut c_void;
        }

        if SCHEDULER_STARTED && CURRENT_TASK < MAX_TASKS && prio > TASKS[CURRENT_TASK].priority {
            if SCHEDULER_SUSPENDED > 0 {
                YIELD_PENDING = true;
            } else {
                should_yield = true;
            }
        }

        critical::vPortExitCritical();

        if should_yield {
            if TASK_MARKERS[slot] == b'S' {
                crate::logger::debug_byte(b'(');
            }
            super::port::PendSV(1);
            if TASK_MARKERS[slot] == b'S' {
                crate::logger::debug_byte(b')');
            }
        }

        PD_PASS
    }
}

/// `xTaskGenericCreate` — alias used by some SDK code.
#[no_mangle]
pub unsafe extern "C" fn xTaskGenericCreate(
    task_code: unsafe extern "C" fn(*mut c_void),
    name: *const c_char,
    stack_depth: u16,
    parameters: *mut c_void,
    priority: u32,
    created_task: *mut *mut c_void,
    _stack_buffer: *mut u8,
    _regions: *const c_void,
) -> i32 {
    unsafe {
        xTaskCreate(
            task_code,
            name,
            stack_depth,
            parameters,
            priority,
            created_task,
        )
    }
}

// ---------------------------------------------------------------------------
// FreeRTOS API: task deletion
// ---------------------------------------------------------------------------

/// `vTaskDelete` — delete a task (or the current task if handle is NULL).
///
/// Stack deallocation is deferred to the idle task to avoid freeing the stack
/// we're currently running on.
#[no_mangle]
pub unsafe extern "C" fn vTaskDelete(handle: *mut c_void) {
    unsafe {
        critical::vPortEnterCritical();

        let idx = if handle.is_null() {
            CURRENT_TASK
        } else {
            handle_to_index(handle)
        };

        if idx < MAX_TASKS {
            if !FIRST_DELETE_TRACE_DONE {
                FIRST_DELETE_TRACE_DONE = true;
                crate::logger::debug_byte(b'D');
                crate::logger::debug_byte(debug_current_task_marker());
                crate::logger::debug_byte(if idx == CURRENT_TASK { b'S' } else { b'O' });
                crate::logger::debug_byte(task_name_marker(idx));
                crate::logger::debug_byte(task_state_marker(TASKS[idx].state));
            }
            cancel_task_wait_locked(idx);
            clear_pending_ready_locked(idx);
            if idx == CURRENT_TASK {
                // Match SDK tasks.c: only self-delete is deferred to the idle
                // task's termination path.
                TASKS[idx].state = TaskState::Deleted;
                enqueue_deleted_task_locked(idx);
            } else {
                destroy_task_slot(idx);
            }
        }

        let was_current = idx == CURRENT_TASK;
        critical::vPortExitCritical();

        if was_current {
            super::port::PendSV(1);
        }
    }
}

// ---------------------------------------------------------------------------
// FreeRTOS API: delay
// ---------------------------------------------------------------------------

/// `vTaskDelay` — block the current task for `ticks` tick periods.
#[no_mangle]
pub unsafe extern "C" fn vTaskDelay(ticks: u32) {
    unsafe {
        if ticks == 0 {
            super::port::PendSV(1);
            return;
        }

        vTaskSuspendAll();

        critical::vPortEnterCritical();
        record_wait_stage(b'd', CURRENT_TASK);
        clear_task_wait_locked(CURRENT_TASK);
        TASKS[CURRENT_TASK].state = TaskState::Blocked;
        TASKS[CURRENT_TASK].delay_until = TICK_COUNT + ticks as u64;
        critical::vPortExitCritical();

        let already_yielded = xTaskResumeAll() != 0;
        if !already_yielded {
            super::port::PendSV(1);
        }
        record_wait_stage(b'D', CURRENT_TASK);
    }
}

// ---------------------------------------------------------------------------
// FreeRTOS API: suspend / resume
// ---------------------------------------------------------------------------

/// `vTaskSuspendAll` — prevent context switches.
#[no_mangle]
pub unsafe extern "C" fn vTaskSuspendAll() {
    unsafe {
        SCHEDULER_SUSPENDED += 1;
    }
}

/// `xTaskResumeAll` — resume the scheduler after `vTaskSuspendAll`.
#[no_mangle]
pub unsafe extern "C" fn xTaskResumeAll() -> i32 {
    unsafe {
        critical::vPortEnterCritical();

        if SCHEDULER_SUSPENDED > 0 {
            SCHEDULER_SUSPENDED -= 1;
        }

        if SCHEDULER_SUSPENDED == 0 {
            drain_pending_ready_locked();

            let pended = PENDED_TICKS;
            PENDED_TICKS = 0;

            for _ in 0..pended {
                if xTaskIncrementTick() != 0 {
                    YIELD_PENDING = true;
                }
            }
        }

        let switched = if SCHEDULER_SUSPENDED == 0 && YIELD_PENDING {
            // Match ESP8266 SDK `tasks.c` + `portmacro.h`:
            // `taskYIELD_IF_USING_PREEMPTION()` inside xTaskResumeAll()
            // expands to `portYIELD_WITHIN_API()` -> `portYIELD()` ->
            // `PendSV(1)`, even though we are already inside the outer
            // critical section.
            super::port::PendSV(1);
            PD_PASS
        } else {
            PD_FALSE
        };

        critical::vPortExitCritical();
        switched
    }
}

/// `vTaskSuspend` — suspend a specific task (or current if NULL).
#[no_mangle]
pub unsafe extern "C" fn vTaskSuspend(handle: *mut c_void) {
    unsafe {
        critical::vPortEnterCritical();

        let idx = if handle.is_null() {
            CURRENT_TASK
        } else {
            handle_to_index(handle)
        };

        if idx < MAX_TASKS {
            cancel_task_wait_locked(idx);
            TASKS[idx].state = TaskState::Suspended;
        }

        let was_current = idx == CURRENT_TASK;
        critical::vPortExitCritical();

        if was_current {
            super::port::PendSV(1);
        }
    }
}

/// `vTaskResume` — resume a suspended task.
#[no_mangle]
pub unsafe extern "C" fn vTaskResume(handle: *mut c_void) {
    unsafe {
        if handle.is_null() {
            return;
        }
        critical::vPortEnterCritical();

        let idx = handle_to_index(handle);
        if idx < MAX_TASKS
            && TASKS[idx].state == TaskState::Suspended
            && TASK_WAIT_KIND[idx] == WaitObjectKind::None
        {
            TASKS[idx].state = TaskState::Ready;
        }

        critical::vPortExitCritical();
    }
}

// ---------------------------------------------------------------------------
// FreeRTOS API: priority
// ---------------------------------------------------------------------------

/// `uxTaskPriorityGet` — return a task's current priority.
#[no_mangle]
pub unsafe extern "C" fn uxTaskPriorityGet(handle: *const c_void) -> u32 {
    unsafe {
        let idx = if handle.is_null() {
            CURRENT_TASK
        } else {
            handle_to_index(handle as *mut c_void)
        };
        if idx < MAX_TASKS {
            TASKS[idx].priority
        } else {
            0
        }
    }
}

/// `vTaskPrioritySet` — change a task's priority.
#[no_mangle]
pub unsafe extern "C" fn vTaskPrioritySet(handle: *mut c_void, new_priority: u32) {
    unsafe {
        critical::vPortEnterCritical();

        let idx = if handle.is_null() {
            CURRENT_TASK
        } else {
            handle_to_index(handle)
        };

        if idx < MAX_TASKS {
            let prio = new_priority.min(MAX_PRIORITIES - 1);
            TASKS[idx].priority = prio;
            TASKS[idx].base_priority = prio;
        }

        critical::vPortExitCritical();
    }
}

// ---------------------------------------------------------------------------
// FreeRTOS API: queries
// ---------------------------------------------------------------------------

/// `xTaskGetCurrentTaskHandle` — return handle of the running task.
#[no_mangle]
pub unsafe extern "C" fn xTaskGetCurrentTaskHandle() -> *mut c_void {
    unsafe {
        if CURRENT_TASK < MAX_TASKS {
            &mut TASKS[CURRENT_TASK] as *mut TaskControlBlock as *mut c_void
        } else {
            core::ptr::null_mut()
        }
    }
}

/// `uxTaskGetStackHighWaterMark` — return the minimum ever free stack.
///
/// FreeRTOS reports this in `StackType_t` items; on ESP8266 that means bytes.
#[no_mangle]
pub unsafe extern "C" fn uxTaskGetStackHighWaterMark(handle: *mut c_void) -> u32 {
    unsafe {
        let idx = if handle.is_null() {
            CURRENT_TASK
        } else {
            handle_to_index(handle)
        };

        stack_high_water_bytes(idx)
    }
}

/// `xTaskGetSchedulerState` — return scheduler state.
#[no_mangle]
pub unsafe extern "C" fn xTaskGetSchedulerState() -> i32 {
    unsafe {
        if !SCHEDULER_STARTED {
            TASK_NOT_STARTED
        } else if SCHEDULER_SUSPENDED > 0 {
            TASK_SUSPENDED
        } else {
            TASK_RUNNING
        }
    }
}

/// `pcTaskGetName` — return a task's name.
#[no_mangle]
pub unsafe extern "C" fn pcTaskGetName(handle: *mut c_void) -> *const c_char {
    unsafe {
        let idx = if handle.is_null() {
            CURRENT_TASK
        } else {
            handle_to_index(handle)
        };
        if idx < MAX_TASKS {
            TASKS[idx].name.as_ptr() as *const c_char
        } else {
            b"\0".as_ptr() as *const c_char
        }
    }
}

// ---------------------------------------------------------------------------
// FreeRTOS API: thread-local storage
// ---------------------------------------------------------------------------

/// `pvTaskGetThreadLocalStorageBufferPointer` — get TLS pointer.
#[no_mangle]
pub unsafe extern "C" fn pvTaskGetThreadLocalStorageBufferPointer(
    handle: *mut c_void,
    index: i32,
) -> *mut c_void {
    unsafe {
        let idx = if handle.is_null() {
            CURRENT_TASK
        } else {
            handle_to_index(handle)
        };
        if idx < MAX_TASKS && (index as usize) < TLS_SLOTS {
            TASKS[idx].tls[index as usize]
        } else {
            core::ptr::null_mut()
        }
    }
}

/// `vTaskSetThreadLocalStoragePointer` — set TLS pointer.
#[no_mangle]
pub unsafe extern "C" fn vTaskSetThreadLocalStoragePointer(
    handle: *mut c_void,
    index: i32,
    value: *mut c_void,
) {
    unsafe {
        let idx = if handle.is_null() {
            CURRENT_TASK
        } else {
            handle_to_index(handle)
        };
        if idx < MAX_TASKS && (index as usize) < TLS_SLOTS {
            TASKS[idx].tls[index as usize] = value;
        }
    }
}

// ---------------------------------------------------------------------------
// FreeRTOS API: tick management
// ---------------------------------------------------------------------------

/// `xTaskIncrementTick` — called from the tick ISR.
///
/// Increments the tick counter, wakes tasks whose delay has expired,
/// processes timers, polls embassy alarms, and returns `pdTRUE` if a
/// context switch is needed.
#[inline(never)]
#[no_mangle]
pub unsafe extern "C" fn xTaskIncrementTick() -> i32 {
    unsafe { xTaskIncrementTick_impl() }
}

#[inline(never)]
unsafe fn xTaskIncrementTick_impl() -> i32 {
    unsafe {
        g_tick_stage = b'i' as u32;
        if SCHEDULER_SUSPENDED > 0 {
            PENDED_TICKS = PENDED_TICKS.saturating_add(1);
            return PD_FALSE;
        }

        TICK_COUNT += 1;

        let current_marker = debug_current_task_marker();
        let current_prio = if CURRENT_TASK < MAX_TASKS {
            TASKS[CURRENT_TASK].priority
        } else {
            0
        };
        if current_marker == b'U' && !FIRST_UI_TICK_DECISION_TRACE_DONE {
            crate::logger::debug_byte(b'1');
        }

        g_tick_stage = b'w' as u32;
        let wake_switch = wake_timed_out_tasks_locked(current_prio);
        let mut switch_needed = wake_switch;
        if current_marker == b'U' && !FIRST_UI_TICK_DECISION_TRACE_DONE {
            crate::logger::debug_byte(if wake_switch { b'3' } else { b'2' });
        }

        // Process software timers
        g_tick_stage = b't' as u32;
        super::timer::process_timers(TICK_COUNT);
        if current_marker == b'U' && !FIRST_UI_TICK_DECISION_TRACE_DONE {
            crate::logger::debug_byte(b'4');
        }

        // SDK `xTaskIncrementTick()` only does RTOS tick bookkeeping. Keep
        // embassy alarm polling on real tick-ISR calls, but skip it when
        // `xTaskResumeAll()` unwinds pended ticks on an arbitrary task stack.
        if super::port::xPortInIsrContext() != 0 {
            g_tick_stage = b'a' as u32;
            crate::wifi::timer_driver::poll_alarms();
            if current_marker == b'U' && !FIRST_UI_TICK_DECISION_TRACE_DONE {
                crate::logger::debug_byte(b'5');
                FIRST_UI_TICK_DECISION_TRACE_DONE = true;
            }
        }

        if !switch_needed
            && CURRENT_TASK < MAX_TASKS
            && ready_or_running_count_at_priority(current_prio) > 1
        {
            switch_needed = true;
        }

        g_tick_stage = b'r' as u32;
        if switch_needed {
            PD_PASS
        } else {
            PD_FALSE
        }
    }
}

#[inline(never)]
unsafe fn wake_timed_out_tasks_locked(current_prio: u32) -> bool {
    unsafe {
        let mut switch_needed = false;
        let mut idx = 0usize;
        g_tick_stage = b'b' as u32;

        while idx < MAX_TASKS {
            g_tick_scan_task = task_name_marker(idx) as u32;
            if TASKS[idx].state == TaskState::Blocked {
                let deadline = TASKS[idx].delay_until;
                if deadline != 0 && TICK_COUNT >= deadline {
                    cancel_task_wait_locked(idx);
                    record_wait_stage(b'x', idx);
                    TASKS[idx].state = TaskState::Ready;
                    TASKS[idx].delay_until = 0;
                    if TASKS[idx].priority >= current_prio {
                        switch_needed = true;
                    }
                }
            }

            idx += 1;
        }

        switch_needed
    }
}

/// `vTaskStepTick` — advance the tick counter by multiple ticks at once.
#[no_mangle]
pub unsafe extern "C" fn vTaskStepTick(ticks: u32) {
    unsafe {
        TICK_COUNT += ticks as u64;
        super::port::esp_increase_tick_cnt(ticks);
    }
}

/// `xTaskGetTickCount` — return the current tick count (truncated to u32).
///
/// The internal counter is u64 but the FreeRTOS API returns u32, so this
/// wraps every ~49.7 days at 1 kHz tick rate. Callers must handle wrap.
#[no_mangle]
pub unsafe extern "C" fn xTaskGetTickCount() -> u32 {
    unsafe { TICK_COUNT as u32 }
}

// ---------------------------------------------------------------------------
// Context switch
// ---------------------------------------------------------------------------

/// `vTaskSwitchContext` — select the highest-priority ready task.
///
/// Called from `_xt_isr_handler` (assembly) after an ISR sets
/// `s_switch_ctx_flag`, and from `xPortStartScheduler`.
#[no_mangle]
#[ram]
pub unsafe extern "C" fn vTaskSwitchContext() {
    unsafe {
        g_switch_stage = b'e' as u32;
        g_switch_from_task = if CURRENT_TASK < MAX_TASKS {
            task_name_marker(CURRENT_TASK) as u32
        } else {
            b'?' as u32
        };
        g_switch_to_task = 0;

        if SCHEDULER_SUSPENDED > 0 {
            g_switch_stage = b's' as u32;
            YIELD_PENDING = true;
            return;
        }

        // Match SDK tasks.c: once the scheduler is actively selecting the next
        // task, the pending-yield latch is consumed.
        YIELD_PENDING = false;

        // Match SDK `taskCHECK_FOR_STACK_OVERFLOW()` placement in
        // `vTaskSwitchContext()`: check the currently running task before
        // selecting the next task.
        g_switch_stage = b'k' as u32;
        if CURRENT_TASK < MAX_TASKS {
            report_stack_canary_once(CURRENT_TASK);
        }

        // Match the SDK invariant: only the current task may be considered
        // running. Any stale non-current Running tasks belong back in the
        // ready set.
        g_switch_stage = b'n' as u32;
        let current_idx = CURRENT_TASK;
        let mut i = 0usize;
        while i < MAX_TASKS {
            if i != CURRENT_TASK && TASKS[i].state == TaskState::Running {
                TASKS[i].state = TaskState::Ready;
            }

            i += 1;
        }

        // Mark current task as Ready (if it's still Running)
        if current_idx < MAX_TASKS && TASKS[current_idx].state == TaskState::Running {
            TASKS[current_idx].state = TaskState::Ready;
        }

        // Find highest-priority ready task (round-robin within same priority).
        g_switch_stage = b'f' as u32;
        let mut best_idx = usize::MAX;
        let mut best_prio: u32 = 0;

        if current_idx < MAX_TASKS {
            i = current_idx + 1;
            while i < MAX_TASKS {
                consider_ready_candidate(i, &mut best_idx, &mut best_prio);
                i += 1;
            }

            i = 0;
            while i <= current_idx {
                consider_ready_candidate(i, &mut best_idx, &mut best_prio);
                i += 1;
            }
        } else {
            i = 0;
            while i < MAX_TASKS {
                consider_ready_candidate(i, &mut best_idx, &mut best_prio);
                i += 1;
            }
        }

        let selected_idx = if best_idx < MAX_TASKS {
            Some(best_idx)
        } else {
            find_idle_task_index()
        };

        if let Some(idx) = selected_idx {
            g_switch_to_task = task_name_marker(idx) as u32;
            g_switch_stage = b'p' as u32;
            CURRENT_TASK = idx;
            TASKS[idx].state = TaskState::Running;
            pxCurrentTCB = &mut TASKS[idx] as *mut TaskControlBlock as *mut c_void;
        }
        g_switch_stage = b'z' as u32;
        // The SDK scheduler always has the idle task available as a fallback.
    }
}

// ---------------------------------------------------------------------------
// Blocking support (used by queue.rs)
// ---------------------------------------------------------------------------

/// Block the current task until `deadline` tick (0 = no timeout, i.e.
/// infinite). The task is set to Blocked; the caller must trigger a
/// context switch after calling this.
pub(crate) unsafe fn block_current_task(deadline: u64) {
    unsafe {
        record_wait_stage(b'b', CURRENT_TASK);
        clear_pending_ready_locked(CURRENT_TASK);
        TASKS[CURRENT_TASK].state = TaskState::Blocked;
        TASKS[CURRENT_TASK].delay_until = deadline;
    }
}

unsafe fn block_current_task_on_wait_object(
    kind: WaitObjectKind,
    object: *mut c_void,
    deadline: u64,
) {
    unsafe {
        record_wait_stage(b'b', CURRENT_TASK);
        clear_pending_ready_locked(CURRENT_TASK);
        set_task_wait_locked(CURRENT_TASK, kind, object);
        TASKS[CURRENT_TASK].state = if deadline == 0 {
            TaskState::Suspended
        } else {
            TaskState::Blocked
        };
        TASKS[CURRENT_TASK].delay_until = deadline;
    }
}

pub(crate) unsafe fn block_current_task_on_queue_rx(queue: *mut c_void, deadline: u64) {
    unsafe {
        block_current_task_on_wait_object(WaitObjectKind::QueueRx, queue, deadline);
    }
}

pub(crate) unsafe fn block_current_task_on_queue_tx(queue: *mut c_void, deadline: u64) {
    unsafe {
        block_current_task_on_wait_object(WaitObjectKind::QueueTx, queue, deadline);
    }
}

pub(crate) unsafe fn block_current_task_on_event_group(event_group: *mut c_void, deadline: u64) {
    unsafe {
        block_current_task_on_wait_object(WaitObjectKind::EventGroup, event_group, deadline);
    }
}

/// Wake a blocked task by index, making it Ready. Returns true if the
/// woken task has higher priority than the currently running task.
pub(crate) unsafe fn wake_task(idx: usize) -> bool {
    unsafe {
        if idx < MAX_TASKS
            && (TASKS[idx].state == TaskState::Blocked
                || (TASKS[idx].state == TaskState::Suspended
                    && TASK_WAIT_KIND[idx] != WaitObjectKind::None))
        {
            record_wait_stage(b'w', idx);
            TASKS[idx].delay_until = 0;
            clear_task_wait_locked(idx);
            let higher_priority = TASKS[idx].priority > TASKS[CURRENT_TASK].priority;
            if SCHEDULER_SUSPENDED > 0 {
                enqueue_pending_ready_locked(idx);
                if higher_priority {
                    YIELD_PENDING = true;
                }
                return higher_priority;
            }

            TASKS[idx].state = TaskState::Ready;
            PENDING_READY[idx] = false;
            if higher_priority {
                return true;
            }
        }
        false
    }
}

pub(crate) unsafe fn missed_yield() {
    unsafe {
        YIELD_PENDING = true;
    }
}

pub(crate) unsafe fn scheduler_is_suspended() -> bool {
    unsafe { SCHEDULER_SUSPENDED > 0 }
}

pub(crate) unsafe fn debug_scheduler_suspended_count() -> u32 {
    unsafe { SCHEDULER_SUSPENDED }
}

/// Get the current task index.
pub(crate) unsafe fn current_task_index() -> usize {
    unsafe { CURRENT_TASK }
}

pub unsafe fn debug_current_task_stack_top() -> u32 {
    unsafe {
        if CURRENT_TASK < MAX_TASKS {
            TASKS[CURRENT_TASK].stack_top as u32
        } else {
            0
        }
    }
}

/// Store the event-group wake result for a blocked task.
pub(crate) unsafe fn set_event_wait_result(idx: usize, bits: u32, matched: bool) {
    unsafe {
        if idx < MAX_TASKS {
            TASK_EVENT_WAIT_BITS[idx] = bits;
            TASK_EVENT_WAIT_MATCHED[idx] = matched;
        }
    }
}

/// Take and clear the latched event-group wake result for a task.
pub(crate) unsafe fn take_event_wait_result(idx: usize) -> (u32, bool) {
    unsafe {
        if idx < MAX_TASKS {
            let bits = TASK_EVENT_WAIT_BITS[idx];
            let matched = TASK_EVENT_WAIT_MATCHED[idx];
            TASK_EVENT_WAIT_BITS[idx] = 0;
            TASK_EVENT_WAIT_MATCHED[idx] = false;
            (bits, matched)
        } else {
            (0, false)
        }
    }
}

/// Get a task's priority by index.
pub(crate) unsafe fn task_priority(idx: usize) -> u32 {
    unsafe {
        if idx < MAX_TASKS {
            TASKS[idx].priority
        } else {
            0
        }
    }
}

/// Whether the task is still in the compat Blocked state.
pub(crate) unsafe fn task_is_blocked(idx: usize) -> bool {
    unsafe {
        idx < MAX_TASKS
            && (TASKS[idx].state == TaskState::Blocked
                || (TASKS[idx].state == TaskState::Suspended
                    && TASK_WAIT_KIND[idx] != WaitObjectKind::None))
    }
}

/// Temporarily raise current task's priority (for priority inheritance).
pub(crate) unsafe fn raise_priority(idx: usize, new_prio: u32) {
    unsafe {
        if idx < MAX_TASKS && new_prio > TASKS[idx].priority {
            TASKS[idx].priority = new_prio;
        }
    }
}

/// Restore a task's base priority (after mutex release).
pub(crate) unsafe fn restore_priority(idx: usize) {
    unsafe {
        if idx < MAX_TASKS {
            TASKS[idx].priority = TASKS[idx].base_priority;
        }
    }
}

#[inline(never)]
unsafe fn drain_pending_ready_locked() {
    unsafe {
        g_pending_ready_len_dbg = PENDING_READY_LEN as u32;
        let len = PENDING_READY_LEN.min(MAX_TASKS);
        let current_prio = if CURRENT_TASK < MAX_TASKS {
            TASKS[CURRENT_TASK].priority
        } else {
            0
        };

        let mut slot = 0usize;
        while slot < len {
            let idx = PENDING_READY_ORDER[slot];
            PENDING_READY_ORDER[slot] = usize::MAX;
            slot += 1;

            if idx >= MAX_TASKS || !PENDING_READY[idx] {
                continue;
            }

            PENDING_READY[idx] = false;
            if TASKS[idx].state != TaskState::Blocked && TASKS[idx].state != TaskState::Suspended {
                continue;
            }

            TASKS[idx].state = TaskState::Ready;
            TASKS[idx].delay_until = 0;
            if TASKS[idx].priority >= current_prio {
                YIELD_PENDING = true;
            }
        }

        while slot < MAX_TASKS {
            PENDING_READY_ORDER[slot] = usize::MAX;
            slot += 1;
        }
        PENDING_READY_LEN = 0;
        g_pending_ready_len_dbg = 0;
    }
}

unsafe fn enqueue_pending_ready_locked(idx: usize) {
    unsafe {
        g_pending_ready_len_dbg = PENDING_READY_LEN as u32;
        if idx >= MAX_TASKS || PENDING_READY[idx] {
            return;
        }

        if PENDING_READY_LEN >= MAX_TASKS {
            compact_pending_ready_locked();
            if PENDING_READY_LEN >= MAX_TASKS {
                return;
            }
        }

        PENDING_READY[idx] = true;
        PENDING_READY_ORDER[PENDING_READY_LEN] = idx;
        PENDING_READY_LEN += 1;
        g_pending_ready_len_dbg = PENDING_READY_LEN as u32;
    }
}

unsafe fn clear_pending_ready_locked(idx: usize) {
    unsafe {
        if idx < MAX_TASKS {
            PENDING_READY[idx] = false;
        }
        g_pending_ready_len_dbg = PENDING_READY_LEN as u32;
    }
}

unsafe fn compact_pending_ready_locked() {
    unsafe {
        g_pending_ready_len_dbg = PENDING_READY_LEN as u32;
        let len = PENDING_READY_LEN.min(MAX_TASKS);
        if PENDING_READY_LEN > MAX_TASKS && !FIRST_PENDING_READY_LEN_CLAMP_TRACE_DONE {
            FIRST_PENDING_READY_LEN_CLAMP_TRACE_DONE = true;
            crate::logger::debug_byte(b'j');
            crate::logger::debug_byte(debug_current_task_marker());
        }
        let mut seen = [false; MAX_TASKS];
        let mut write = 0usize;

        for read in 0..len {
            let idx = PENDING_READY_ORDER[read];
            if idx < MAX_TASKS && PENDING_READY[idx] && !seen[idx] {
                PENDING_READY_ORDER[write] = idx;
                seen[idx] = true;
                write += 1;
            }
        }

        for slot in write..MAX_TASKS {
            PENDING_READY_ORDER[slot] = usize::MAX;
        }
        PENDING_READY_LEN = write;
        g_pending_ready_len_dbg = PENDING_READY_LEN as u32;
    }
}

unsafe fn enqueue_deleted_task_locked(idx: usize) {
    unsafe {
        g_deleted_task_queue_len_dbg = DELETED_TASK_QUEUE_LEN as u32;
        if idx >= MAX_TASKS {
            return;
        }

        let len = DELETED_TASK_QUEUE_LEN.min(MAX_TASKS);
        if DELETED_TASK_QUEUE_LEN > MAX_TASKS && !FIRST_DELETED_QUEUE_LEN_CLAMP_TRACE_DONE {
            FIRST_DELETED_QUEUE_LEN_CLAMP_TRACE_DONE = true;
            crate::logger::debug_byte(b'k');
            crate::logger::debug_byte(debug_current_task_marker());
        }

        for slot in 0..len {
            if DELETED_TASK_QUEUE[slot] == idx {
                return;
            }
        }

        if len >= MAX_TASKS {
            return;
        }

        DELETED_TASK_QUEUE[len] = idx;
        DELETED_TASK_QUEUE_LEN = len + 1;
        DELETED_TASKS_WAITING_CLEANUP += 1;
        g_deleted_task_queue_len_dbg = DELETED_TASK_QUEUE_LEN as u32;
    }
}

unsafe fn pop_deleted_task_locked() -> Option<usize> {
    unsafe {
        g_deleted_task_queue_len_dbg = DELETED_TASK_QUEUE_LEN as u32;
        let len = DELETED_TASK_QUEUE_LEN.min(MAX_TASKS);
        if DELETED_TASK_QUEUE_LEN > MAX_TASKS && !FIRST_DELETED_QUEUE_LEN_CLAMP_TRACE_DONE {
            FIRST_DELETED_QUEUE_LEN_CLAMP_TRACE_DONE = true;
            crate::logger::debug_byte(b'k');
            crate::logger::debug_byte(debug_current_task_marker());
        }

        if DELETED_TASKS_WAITING_CLEANUP == 0 || len == 0 {
            return None;
        }

        let idx = DELETED_TASK_QUEUE[0];
        for slot in 1..len {
            DELETED_TASK_QUEUE[slot - 1] = DELETED_TASK_QUEUE[slot];
        }
        DELETED_TASK_QUEUE_LEN = len - 1;
        DELETED_TASK_QUEUE[DELETED_TASK_QUEUE_LEN] = usize::MAX;
        DELETED_TASKS_WAITING_CLEANUP -= 1;
        g_deleted_task_queue_len_dbg = DELETED_TASK_QUEUE_LEN as u32;

        if idx < MAX_TASKS {
            Some(idx)
        } else {
            None
        }
    }
}

unsafe fn destroy_task_slot(idx: usize) {
    unsafe {
        if idx >= MAX_TASKS {
            return;
        }

        if !TASKS[idx].stack_base.is_null() {
            memory::free(TASKS[idx].stack_base as *mut c_void);
        }

        TASKS[idx].stack_top = core::ptr::null_mut();
        TASKS[idx].stack_base = core::ptr::null_mut();
        TASKS[idx].stack_size = 0;
        TASKS[idx].priority = 0;
        TASKS[idx].base_priority = 0;
        TASKS[idx].state = TaskState::Free;
        TASKS[idx].delay_until = 0;
        TASKS[idx].name = [0; 16];
        TASKS[idx].tls = [core::ptr::null_mut(); TLS_SLOTS];
        TASKS[idx].mutex_held_count = 0;
        TASKS[idx].entry = None;
        TASKS[idx].param = core::ptr::null_mut();
        TASK_EVENT_WAIT_BITS[idx] = 0;
        TASK_EVENT_WAIT_MATCHED[idx] = false;
        TASK_WAIT_KIND[idx] = WaitObjectKind::None;
        TASK_WAIT_OBJECT[idx] = core::ptr::null_mut();
        PENDING_READY[idx] = false;
        TASK_MARKERS[idx] = b'?';
        STACK_CANARY_REPORTED[idx] = false;
    }
}

/// Get the current tick count (internal).
pub(crate) unsafe fn get_tick_count() -> u64 {
    unsafe { TICK_COUNT }
}

unsafe fn next_task_unblock_time() -> u64 {
    unsafe {
        let mut next = u64::MAX;

        for i in 0..MAX_TASKS {
            if TASKS[i].state == TaskState::Blocked {
                let deadline = TASKS[i].delay_until;
                if deadline != 0 && deadline < next {
                    next = deadline;
                }
            }
        }

        next
    }
}

/// SDK-compatible `prvGetExpectedIdleTime()` backing function.
///
/// Mirrors the decompiled SDK `tasks.c` logic used by `esp_sleep.c` through
/// `esp_internal_idle_hook()`:
/// - return 0 if the current task is above idle priority
/// - return 0 if the idle ready list has more than one item
/// - return 0 if the top ready priority is above idle
/// - otherwise return ticks until the next timed unblock
///
/// When no timed unblock is pending, FreeRTOS keeps `xNextTaskUnblockTime` at
/// `portMAX_DELAY`, so return `u32::MAX` here as the compat analogue.
#[no_mangle]
pub unsafe extern "C" fn compat_prvGetExpectedIdleTime() -> u32 {
    unsafe {
        const IDLE_PRIORITY: u32 = 0;

        if CURRENT_TASK >= MAX_TASKS || TASKS[CURRENT_TASK].priority > IDLE_PRIORITY {
            return 0;
        }

        if ready_or_running_count_at_priority(IDLE_PRIORITY) > 1 {
            return 0;
        }

        if top_ready_or_running_priority() != IDLE_PRIORITY {
            return 0;
        }

        let next = next_task_unblock_time();
        if next == u64::MAX {
            u32::MAX
        } else {
            next.saturating_sub(TICK_COUNT).min(u32::MAX as u64) as u32
        }
    }
}

/// Minimal timeout state used by the compat queue blocking paths.
///
/// SDK FreeRTOS stores both tick and overflow count. The compat scheduler
/// uses a monotonic `u64` tick counter, so only the entry tick is required.
#[derive(Clone, Copy)]
pub(crate) struct TimeOutState {
    time_on_entering: u64,
}

pub(crate) const fn new_timeout_state() -> TimeOutState {
    TimeOutState {
        time_on_entering: 0,
    }
}

pub(crate) unsafe fn set_timeout_state(state: &mut TimeOutState) {
    unsafe {
        state.time_on_entering = TICK_COUNT;
    }
}

pub(crate) unsafe fn check_for_timeout(state: &mut TimeOutState, ticks_to_wait: &mut u32) -> bool {
    unsafe {
        if *ticks_to_wait == PORT_MAX_DELAY {
            return false;
        }

        critical::vPortEnterCritical();
        let now = TICK_COUNT;
        let elapsed = now.saturating_sub(state.time_on_entering);
        let timed_out = if elapsed < *ticks_to_wait as u64 {
            *ticks_to_wait -= elapsed as u32;
            state.time_on_entering = now;
            false
        } else {
            *ticks_to_wait = 0;
            true
        };
        critical::vPortExitCritical();

        timed_out
    }
}

/// Mark that the scheduler has started.
pub(crate) unsafe fn mark_scheduler_started() {
    unsafe {
        SCHEDULER_STARTED = true;
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Find a free task slot.
unsafe fn find_free_slot() -> Option<usize> {
    unsafe {
        for i in 0..MAX_TASKS {
            if TASKS[i].state == TaskState::Free {
                return Some(i);
            }
        }
        None
    }
}

unsafe fn set_task_wait_locked(idx: usize, kind: WaitObjectKind, object: *mut c_void) {
    unsafe {
        if idx < MAX_TASKS {
            TASK_WAIT_KIND[idx] = kind;
            TASK_WAIT_OBJECT[idx] = object;
        }
    }
}

unsafe fn clear_task_wait_locked(idx: usize) {
    unsafe {
        if idx < MAX_TASKS {
            TASK_WAIT_KIND[idx] = WaitObjectKind::None;
            TASK_WAIT_OBJECT[idx] = core::ptr::null_mut();
        }
    }
}

unsafe fn cancel_task_wait_locked(idx: usize) {
    unsafe {
        if idx >= MAX_TASKS {
            return;
        }

        let kind = TASK_WAIT_KIND[idx];
        let object = TASK_WAIT_OBJECT[idx];
        clear_task_wait_locked(idx);
        g_cancel_wait_task = task_name_marker(idx) as u32;
        g_cancel_wait_kind = kind as u32;
        g_cancel_wait_object = object as usize as u32;

        match kind {
            WaitObjectKind::None => {}
            WaitObjectKind::QueueRx => super::queue::remove_rx_waiter_for_task(object, idx),
            WaitObjectKind::QueueTx => super::queue::remove_tx_waiter_for_task(object, idx),
            WaitObjectKind::EventGroup => super::event_group::remove_waiter_for_task(object, idx),
        }
    }
}

/// Convert a task handle (pointer to TCB) to an index in the TASKS array.
fn handle_to_index(handle: *mut c_void) -> usize {
    // SAFETY: We only take the address of the static, no aliasing concern.
    let base = unsafe { &raw const TASKS } as *const TaskControlBlock as usize;
    let addr = handle as usize;
    let tcb_size = core::mem::size_of::<TaskControlBlock>();
    if addr >= base && addr < base + MAX_TASKS * tcb_size {
        let offset = addr - base;
        if offset % tcb_size == 0 {
            offset / tcb_size
        } else {
            MAX_TASKS
        }
    } else {
        MAX_TASKS // invalid sentinel
    }
}
