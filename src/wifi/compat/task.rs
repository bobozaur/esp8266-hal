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

/// Maximum number of concurrent tasks. The supported worst case needs the
/// application, idle, timer daemon, sys_evt and ppTask, plus optional WPA2 and
/// WPS workers. Spare slots allow feature workers awaiting idle cleanup
/// to coexist with replacements during rapid stop/start cycles.
pub(super) const MAX_TASKS: usize = 10;
pub(crate) const PP_TASK_STACK_BYTES: u16 = 2816;

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
}

/// Task Control Block.
///
/// **Critical:** `stack_top` MUST be the first field. The interrupt/context
/// entry code dereferences `pxCurrentTCB` to load/store the saved stack pointer
/// from offset 0 of the TCB. Native callers otherwise treat task handles as
/// opaque. Saved frames stay on each task's own stack.
#[repr(C)]
struct TaskControlBlock {
    /// Saved stack pointer (top of interrupt frame). **Must be offset 0.**
    stack_top: *mut u8,
    /// Bottom of the heap-allocated stack buffer.
    stack_base: *mut u8,
    /// Task priority (0 = lowest, MAX_PRIORITIES-1 = highest).
    priority: u32,
    /// Base (original) priority — for priority inheritance.
    base_priority: u32,
    /// Tick count at which a timeout-based block expires (0 = no timeout).
    delay_until: u64,
    /// Current state. Kept last to avoid padding before the deadline.
    state: TaskState,
}

const _: () = {
    assert!(core::mem::offset_of!(TaskControlBlock, stack_top) == 0);
    assert!(core::mem::size_of::<TaskControlBlock>() == 32);
};

// ---------------------------------------------------------------------------
// Global scheduler state
// ---------------------------------------------------------------------------

const EMPTY: TaskControlBlock = TaskControlBlock {
    stack_top: core::ptr::null_mut(),
    stack_base: core::ptr::null_mut(),
    priority: 0,
    base_priority: 0,
    delay_until: 0,
    state: TaskState::Free,
};

/// The task table.
static mut TASKS: [TaskControlBlock; MAX_TASKS] = [EMPTY; MAX_TASKS];

/// Wakeups deferred while scheduling is suspended. Resume drains the complete
/// set before task selection, which uses priority and cyclic task-table order.
static mut PENDING_READY: [bool; MAX_TASKS] = [false; MAX_TASKS];
static mut DELETED_TASK_QUEUE: [usize; MAX_TASKS] = [usize::MAX; MAX_TASKS];
static mut DELETED_TASK_QUEUE_LEN: usize = 0;
static mut TASK_WAIT_KIND: [WaitObjectKind; MAX_TASKS] = [WaitObjectKind::None; MAX_TASKS];
static mut TASK_WAIT_OBJECT: [*mut c_void; MAX_TASKS] = [core::ptr::null_mut(); MAX_TASKS];
static mut IDLE_TASK: usize = MAX_TASKS;

/// Pointer to the current task's TCB (as `*mut c_void`).
///
/// Interrupt/context entry code dereferences this to get
/// `(*pxCurrentTCB).stack_top`.
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
// ---------------------------------------------------------------------------
// Stack initialisation
// ---------------------------------------------------------------------------

/// Stack frame constants matching xtensa-lx-rt's ESP8266 exception frame.
pub(crate) const XT_STK_PC: usize = 0x00;
pub(crate) const XT_STK_PS: usize = 0x04;
pub(crate) const XT_STK_A0: usize = 0x08;
pub(crate) const XT_STK_A1: usize = 0x0C;
pub(crate) const XT_STK_A2: usize = 0x10;
pub(crate) const XT_STK_FRMSZ: usize = 0x100;
/// ESP8266 RTOS SDK call0 interrupt/task frame size from
/// `freertos/xtensa_context.h` with the ESP8266 config headers.
const SDK_XT_STK_FRMSZ: usize = 0x50;
/// Extra bytes each compat-created task needs so an SDK stack-depth request
/// preserves the same usable headroom under the larger xtensa-lx-rt frame.
const COMPAT_STACK_FRAME_DELTA: usize = XT_STK_FRMSZ - SDK_XT_STK_FRMSZ;
pub(crate) const PS_UM: u32 = 0x20;
pub(crate) const PS_EXCM: u32 = 0x10;

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

#[ram]
unsafe fn check_stack_canary(idx: usize) {
    unsafe {
        if !stack_canary_corrupted(idx) {
            return;
        }
        let task = TASKS.as_mut_ptr().add(idx);
        super::port::vApplicationStackOverflowHook(task.cast::<c_void>(), core::ptr::null());
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
        let idx = IDLE_TASK;
        if idx < MAX_TASKS
            && TASKS[idx].state != TaskState::Free
            && TASKS[idx].state != TaskState::Deleted
        {
            Some(idx)
        } else {
            None
        }
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

/// Build the HAL interrupt frame used when a compat-created task first runs.
pub(crate) unsafe fn init_task_stack(
    stack_top: *mut u8,
    entry: unsafe extern "C" fn(*mut c_void),
    param: *mut c_void,
) -> *mut u8 {
    unsafe {
        let frame_top = stack_top.add(4) as usize;
        let sp = ((frame_top - XT_STK_FRMSZ) & !0xF) as *mut u32;
        core::ptr::write_bytes(sp as *mut u8, 0, XT_STK_FRMSZ);
        let set = |offset: usize, val: u32| sp.add(offset >> 2).write(val);
        set(XT_STK_PC, entry as usize as u32);
        set(XT_STK_A0, 0);
        set(XT_STK_A1, sp as u32 + XT_STK_FRMSZ as u32);
        set(XT_STK_A2, param as u32);
        set(XT_STK_PS, PS_UM | PS_EXCM);
        sp.cast()
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
#[ram]
pub(crate) unsafe fn prepare_current_frame_for_restore() -> *mut u8 {
    unsafe {
        let idx = CURRENT_TASK;
        if idx >= MAX_TASKS {
            return core::ptr::null_mut();
        }

        TASKS[idx].stack_top
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
            cleanup_deleted_tasks();
        }

        // Match SDK `prvIdleTask()` with `configIDLE_SHOULD_YIELD == 1`:
        // if another idle-priority task is ready, yield before the timeslice
        // ends instead of spinning in this task.
        if another_idle_priority_task_ready() {
            unsafe {
                super::port::PendSV(1);
            }
        }

        unsafe {
            super::port::esp_internal_idle_hook();
        }
    }
}

/// Free stacks of tasks in the `Deleted` state.
unsafe fn cleanup_deleted_tasks() {
    unsafe {
        while DELETED_TASK_QUEUE_LEN > 0 {
            critical::vPortEnterCritical();
            let idx = pop_deleted_task_locked();
            critical::vPortExitCritical();

            if let Some(idx) = idx {
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
        tcb.priority = 1; // just above idle (0), below WiFi tasks (8-12)
        tcb.base_priority = 1;
        tcb.state = TaskState::Running; // we ARE running right now
        tcb.delay_until = 0;
        TASK_WAIT_KIND[0] = WaitObjectKind::None;
        TASK_WAIT_OBJECT[0] = core::ptr::null_mut();

        CURRENT_TASK = 0;
        PENDING_READY[0] = false;
        pxCurrentTCB = &mut TASKS[0] as *mut TaskControlBlock as *mut c_void;
    }
}

/// Create the idle task during explicit runtime preparation.
pub(crate) unsafe fn create_idle_task() -> i32 {
    unsafe {
        let mut handle = core::ptr::null_mut();
        let result = create_task(
            idle_task,
            // Retain the qualified idle budget, plus the HAL interrupt-frame
            // allowance added by create_task.
            1024,
            core::ptr::null_mut(),
            0, // lowest priority
            &mut handle,
            false,
        );
        if result == PD_PASS {
            IDLE_TASK = handle_to_index(handle);
        }
        result
    }
}

/// Undo startup allocations before the tick or software ISR is activated.
pub(crate) unsafe fn rollback_startup() {
    unsafe {
        for index in 0..MAX_TASKS {
            destroy_task_slot(index);
        }
        pxCurrentTCB = core::ptr::null_mut();
        CURRENT_TASK = 0;
    }
}

// ---------------------------------------------------------------------------
// FreeRTOS API: task creation
// ---------------------------------------------------------------------------

/// `xTaskCreate` — create a new task and add it to the ready list.
#[no_mangle]
pub unsafe extern "C" fn xTaskCreate(
    task_code: unsafe extern "C" fn(*mut c_void),
    _name: *const c_char,
    stack_depth: u16,
    parameters: *mut c_void,
    priority: u32,
    created_task: *mut *mut c_void,
) -> i32 {
    unsafe {
        create_task(
            task_code,
            stack_depth,
            parameters,
            priority,
            created_task,
            false,
        )
    }
}

pub(crate) const fn stack_allocation_bytes(requested: u16) -> usize {
    requested as usize + COMPAT_STACK_FRAME_DELTA
}

// Only the audited native PP task consumes its pre-reserved stack.
pub(crate) unsafe fn create_task(
    task_code: unsafe extern "C" fn(*mut c_void),
    stack_depth: u16,
    parameters: *mut c_void,
    priority: u32,
    created_task: *mut *mut c_void,
    pp_task: bool,
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
        let stack_bytes = stack_allocation_bytes(stack_depth);
        let reserved = if pp_task {
            memory::take_init_allocation(memory::InitAllocation::PpStack, stack_bytes)
        } else {
            None
        };
        let stack_base = reserved
            .unwrap_or_else(|| memory::malloc(stack_bytes))
            .cast::<u8>();
        if stack_base.is_null() {
            critical::vPortExitCritical();
            return ERR_NO_MEM;
        }
        init_stack_canary(stack_base, stack_bytes);

        let prio = priority.min(MAX_PRIORITIES - 1);
        let tcb = &mut TASKS[slot];
        let stack_top = stack_base.add(stack_bytes - 4);
        let sp = init_task_stack(stack_top, task_code, parameters);

        tcb.stack_top = sp;

        tcb.stack_base = stack_base;
        tcb.priority = prio;
        tcb.base_priority = prio;
        tcb.state = TaskState::Ready;
        tcb.delay_until = 0;
        PENDING_READY[slot] = false;
        TASK_WAIT_KIND[slot] = WaitObjectKind::None;
        TASK_WAIT_OBJECT[slot] = core::ptr::null_mut();

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
            super::port::PendSV(1);
        }

        PD_PASS
    }
}

pub(crate) fn has_free_task_slot() -> bool {
    unsafe {
        critical::vPortEnterCritical();
        let available = find_free_slot().is_some();
        critical::vPortExitCritical();
        available
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
        clear_task_wait_locked(CURRENT_TASK);
        TASKS[CURRENT_TASK].state = TaskState::Blocked;
        TASKS[CURRENT_TASK].delay_until = TICK_COUNT + ticks as u64;
        critical::vPortExitCritical();

        let already_yielded = xTaskResumeAll() != 0;
        if !already_yielded {
            super::port::PendSV(1);
        }
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

// ---------------------------------------------------------------------------
// FreeRTOS API: priority
// ---------------------------------------------------------------------------

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
    unsafe { x_task_increment_tick_impl() }
}

#[inline(never)]
unsafe fn x_task_increment_tick_impl() -> i32 {
    unsafe {
        if SCHEDULER_SUSPENDED > 0 {
            PENDED_TICKS = PENDED_TICKS.saturating_add(1);
            return PD_FALSE;
        }

        TICK_COUNT += 1;

        let current_prio = if CURRENT_TASK < MAX_TASKS {
            TASKS[CURRENT_TASK].priority
        } else {
            0
        };
        let wake_switch = wake_timed_out_tasks_locked(current_prio);
        let mut switch_needed = wake_switch;

        if !switch_needed
            && CURRENT_TASK < MAX_TASKS
            && ready_or_running_count_at_priority(current_prio) > 1
        {
            switch_needed = true;
        }

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

        while idx < MAX_TASKS {
            if TASKS[idx].state == TaskState::Blocked {
                let deadline = TASKS[idx].delay_until;
                if deadline != 0 && TICK_COUNT >= deadline {
                    cancel_task_wait_locked(idx);
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
    }
}

// ---------------------------------------------------------------------------
// Context switch
// ---------------------------------------------------------------------------

/// `vTaskSwitchContext` — select the highest-priority ready task.
///
/// Called after an ISR requests a context switch.
#[no_mangle]
#[ram]
pub unsafe extern "C" fn vTaskSwitchContext() {
    unsafe {
        if SCHEDULER_SUSPENDED > 0 {
            YIELD_PENDING = true;
            return;
        }

        // Match SDK tasks.c: once the scheduler is actively selecting the next
        // task, the pending-yield latch is consumed.
        YIELD_PENDING = false;

        // Match SDK `taskCHECK_FOR_STACK_OVERFLOW()` placement in
        // `vTaskSwitchContext()`: check the currently running task before
        // selecting the next task.
        if CURRENT_TASK < MAX_TASKS {
            check_stack_canary(CURRENT_TASK);
        }

        // Match the SDK invariant: only the current task may be considered
        // running. Any stale non-current Running tasks belong back in the
        // ready set.
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
            CURRENT_TASK = idx;
            TASKS[idx].state = TaskState::Running;
            pxCurrentTCB = &mut TASKS[idx] as *mut TaskControlBlock as *mut c_void;
        }
        // The SDK scheduler always has the idle task available as a fallback.
    }
}

// ---------------------------------------------------------------------------
// Blocking support (used by queue.rs)
// ---------------------------------------------------------------------------

/// Block the current task until `deadline` tick (0 = no timeout, i.e.
/// infinite). The task is set to Blocked; the caller must trigger a
/// context switch after calling this.
#[cfg(feature = "embassy")]
pub(crate) unsafe fn block_current_task(deadline: u64) {
    unsafe {
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

/// Wake a blocked task by index, making it Ready. Returns true if the
/// woken task has higher priority than the currently running task.
pub(crate) unsafe fn wake_task(idx: usize) -> bool {
    unsafe {
        if idx < MAX_TASKS
            && (TASKS[idx].state == TaskState::Blocked
                || (TASKS[idx].state == TaskState::Suspended
                    && TASK_WAIT_KIND[idx] != WaitObjectKind::None))
        {
            TASKS[idx].delay_until = 0;
            clear_task_wait_locked(idx);
            let higher_priority = TASKS[idx].priority > TASKS[CURRENT_TASK].priority;
            if SCHEDULER_SUSPENDED > 0 {
                PENDING_READY[idx] = true;
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

/// Get the current task index.
pub(crate) unsafe fn current_task_index() -> usize {
    unsafe { CURRENT_TASK }
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
        let current_prio = if CURRENT_TASK < MAX_TASKS {
            TASKS[CURRENT_TASK].priority
        } else {
            0
        };

        for idx in 0..MAX_TASKS {
            if !PENDING_READY[idx] {
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
    }
}

unsafe fn clear_pending_ready_locked(idx: usize) {
    unsafe {
        if idx < MAX_TASKS {
            PENDING_READY[idx] = false;
        }
    }
}

unsafe fn enqueue_deleted_task_locked(idx: usize) {
    unsafe {
        if idx >= MAX_TASKS {
            return;
        }

        let len = DELETED_TASK_QUEUE_LEN.min(MAX_TASKS);

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
    }
}

unsafe fn pop_deleted_task_locked() -> Option<usize> {
    unsafe {
        let len = DELETED_TASK_QUEUE_LEN.min(MAX_TASKS);

        if len == 0 {
            return None;
        }

        let idx = DELETED_TASK_QUEUE[0];
        for slot in 1..len {
            DELETED_TASK_QUEUE[slot - 1] = DELETED_TASK_QUEUE[slot];
        }
        DELETED_TASK_QUEUE_LEN = len - 1;
        DELETED_TASK_QUEUE[DELETED_TASK_QUEUE_LEN] = usize::MAX;

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

        // Idle cleanup has no outer exclusion. Detach the old allocation and
        // reset every slot field before another task can reuse the Free slot.
        critical::vPortEnterCritical();
        let stack_base = TASKS[idx].stack_base;
        TASKS[idx] = EMPTY;
        TASK_WAIT_KIND[idx] = WaitObjectKind::None;
        TASK_WAIT_OBJECT[idx] = core::ptr::null_mut();
        PENDING_READY[idx] = false;
        if IDLE_TASK == idx {
            IDLE_TASK = MAX_TASKS;
        }
        critical::vPortExitCritical();

        // The slot may already belong to a new task here. Touch only the
        // detached allocation; caller-owned main stacks are represented by null.
        if !stack_base.is_null() {
            memory::free(stack_base.cast());
        }
    }
}

/// Get the current tick count (internal).
pub(crate) unsafe fn get_tick_count() -> u64 {
    unsafe {
        critical::vPortEnterCritical();
        let ticks = TICK_COUNT;
        critical::vPortExitCritical();
        ticks
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

        match kind {
            WaitObjectKind::None => {}
            WaitObjectKind::QueueRx => super::queue::remove_rx_waiter_for_task(object, idx),
            WaitObjectKind::QueueTx => super::queue::remove_tx_waiter_for_task(object, idx),
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
