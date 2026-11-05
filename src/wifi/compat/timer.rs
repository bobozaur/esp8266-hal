//! FreeRTOS-compatible software timer implementation.
//!
//! Timers are stored in a bounded array. A command queue wakes the timer
//! daemon, which dispatches expired callbacks in task context.

use core::ffi::c_void;

use super::critical;
use super::queue;
use super::task;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_TIMERS: usize = 12;
const TIMER_QUEUE_LENGTH: u32 = 10;
const PD_PASS: i32 = 1;
const PD_FALSE: i32 = 0;
// The compatibility daemon has its own stack budget; create_task adds the
// larger HAL exception-frame allowance. Callback depth also consumes this stack.
const TIMER_DAEMON_STACK_BYTES: u16 = 768;
// Match SDK `configTIMER_TASK_PRIORITY = tskIDLE_PRIORITY + 2`.
const TIMER_DAEMON_PRIORITY: u32 = 2;

/// Timer command IDs (matching FreeRTOS tmrCOMMAND_*).
const TMR_COMMAND_START_DONT_TRACE: i32 = 0;
const TMR_COMMAND_START: i32 = 1;
const TMR_COMMAND_RESET: i32 = 2;
const TMR_COMMAND_STOP: i32 = 3;
const TMR_COMMAND_CHANGE_PERIOD: i32 = 4;
const TMR_COMMAND_DELETE: i32 = 5;
const TMR_COMMAND_START_FROM_ISR: i32 = 6;
const TMR_COMMAND_RESET_FROM_ISR: i32 = 7;
const TMR_COMMAND_STOP_FROM_ISR: i32 = 8;
const TMR_COMMAND_CHANGE_PERIOD_FROM_ISR: i32 = 9;

// ---------------------------------------------------------------------------
// Timer data structure
// ---------------------------------------------------------------------------

type TimerCallback = unsafe extern "C" fn(*mut c_void);

struct SoftwareTimer {
    period_ticks: u32,
    auto_reload: bool,
    callback: TimerCallback,
    /// Callback argument (`pvTimerGetTimerID` returns this).
    arg: *mut c_void,
    /// Absolute tick when the timer expires.
    expiry_tick: u64,
    active: bool,
    in_use: bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct DaemonTaskMessage {
    command_id: i32,
    command_value: u32,
    timer: *mut c_void,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static mut TIMERS: [SoftwareTimer; MAX_TIMERS] = {
    const EMPTY: SoftwareTimer = SoftwareTimer {
        period_ticks: 0,
        auto_reload: false,
        callback: empty_callback,
        arg: core::ptr::null_mut(),
        expiry_tick: 0,
        active: false,
        in_use: false,
    };
    [EMPTY; MAX_TIMERS]
};

/// Handle for the timer daemon task (NULL if not started).
static mut TIMER_DAEMON_HANDLE: *mut c_void = core::ptr::null_mut();
static mut TIMER_QUEUE_HANDLE: *mut c_void = core::ptr::null_mut();

unsafe extern "C" fn empty_callback(_: *mut c_void) {}

// ---------------------------------------------------------------------------
// Timer daemon task
// ---------------------------------------------------------------------------

/// Timer daemon task — processes queued timer commands and expirations.
///
/// Runs in task context like SDK `prvTimerTask(...)`, so timer callbacks can
/// safely call blocking APIs.
unsafe extern "C" fn timer_daemon_task(_param: *mut c_void) {
    loop {
        unsafe {
            let now = task::get_tick_count();
            if process_expired_timers(now) {
                continue;
            }

            let wait_ticks = next_timer_wait(now);
            let mut message = DaemonTaskMessage {
                command_id: 0,
                command_value: 0,
                timer: core::ptr::null_mut(),
            };

            let recv = queue::xQueueReceive(
                TIMER_QUEUE_HANDLE,
                &mut message as *mut DaemonTaskMessage as *mut c_void,
                wait_ticks,
            );
            if recv == PD_PASS {
                process_timer_command(message);
                drain_timer_commands();
            }
        }
    }
}

/// Create the timer daemon and its command queue before scheduler activation.
pub(crate) unsafe fn create_timer_daemon() -> i32 {
    unsafe { xTimerCreateTimerTask() }
}

/// Release the command queue after failed preparation, before tasks can run.
pub(crate) unsafe fn rollback_startup() {
    unsafe {
        if !TIMER_QUEUE_HANDLE.is_null() {
            queue::vQueueDelete(TIMER_QUEUE_HANDLE);
            TIMER_QUEUE_HANDLE = core::ptr::null_mut();
        }
        TIMER_DAEMON_HANDLE = core::ptr::null_mut();
    }
}

/// `xTimerCreateTimerTask` — ensure the timer command queue exists, then
/// create the `Tmr Svc` daemon task.
#[no_mangle]
pub unsafe extern "C" fn xTimerCreateTimerTask() -> i32 {
    unsafe {
        ensure_timer_queue();
        if TIMER_QUEUE_HANDLE.is_null() {
            return PD_FALSE;
        }
        if !TIMER_DAEMON_HANDLE.is_null() {
            return PD_PASS;
        }

        task::xTaskCreate(
            timer_daemon_task,
            b"Tmr Svc\0".as_ptr() as *const _,
            TIMER_DAEMON_STACK_BYTES,
            core::ptr::null_mut(),
            TIMER_DAEMON_PRIORITY,
            &raw mut TIMER_DAEMON_HANDLE,
        )
    }
}

// ---------------------------------------------------------------------------
// FreeRTOS API
// ---------------------------------------------------------------------------

/// `xTimerCreate` — create a software timer.
#[no_mangle]
pub unsafe extern "C" fn xTimerCreate(
    _name: *const u8,
    period_ticks: u32,
    auto_reload: i32,
    arg: *mut c_void,
    callback: TimerCallback,
) -> *mut c_void {
    unsafe {
        ensure_timer_queue();

        critical::vPortEnterCritical();

        let Some(idx) = find_free_slot() else {
            critical::vPortExitCritical();
            return core::ptr::null_mut();
        };

        let t = &mut TIMERS[idx];
        t.period_ticks = period_ticks;
        t.auto_reload = auto_reload != 0;
        t.callback = callback;
        t.arg = arg;
        t.expiry_tick = 0;
        t.active = false;
        t.in_use = true;

        critical::vPortExitCritical();

        &mut TIMERS[idx] as *mut SoftwareTimer as *mut c_void
    }
}

/// `xTimerGenericCommand` — the real timer API that FreeRTOS macros expand to.
///
/// `xTimerStart`, `xTimerStop`, `xTimerReset`, `xTimerChangePeriod`,
/// `xTimerDelete` all go through this function.
#[no_mangle]
pub unsafe extern "C" fn xTimerGenericCommand(
    timer: *mut c_void,
    command: i32,
    command_value: u32, // new period for CHANGE_PERIOD, or unused
    _higher_priority_task_woken: *mut i32,
    _ticks_to_wait: u32,
) -> i32 {
    unsafe {
        if timer.is_null() {
            return PD_FALSE;
        }
        ensure_timer_queue();
        if TIMER_QUEUE_HANDLE.is_null() {
            return PD_FALSE;
        }

        let message = DaemonTaskMessage {
            command_id: command,
            command_value,
            timer,
        };

        if command < TMR_COMMAND_START_FROM_ISR {
            let ticks_to_wait = if task::xTaskGetSchedulerState() == 2 {
                _ticks_to_wait
            } else {
                0
            };

            queue::xQueueGenericSend(
                TIMER_QUEUE_HANDLE,
                &message as *const DaemonTaskMessage as *const c_void,
                ticks_to_wait,
                0,
            )
        } else {
            queue::xQueueSendFromISR(
                TIMER_QUEUE_HANDLE,
                &message as *const DaemonTaskMessage as *const c_void,
                _higher_priority_task_woken,
            )
        }
    }
}

/// `xTimerStart` — start a timer.
#[no_mangle]
pub unsafe extern "C" fn xTimerStart(timer: *mut c_void, ticks_to_wait: u32) -> i32 {
    unsafe {
        xTimerGenericCommand(
            timer,
            TMR_COMMAND_START,
            task::xTaskGetTickCount(),
            core::ptr::null_mut(),
            ticks_to_wait,
        )
    }
}

/// `xTimerReset` — (re)start a timer.
#[no_mangle]
pub unsafe extern "C" fn xTimerReset(timer: *mut c_void, ticks_to_wait: u32) -> i32 {
    unsafe {
        xTimerGenericCommand(
            timer,
            TMR_COMMAND_RESET,
            task::xTaskGetTickCount(),
            core::ptr::null_mut(),
            ticks_to_wait,
        )
    }
}

/// `xTimerStop` — stop a timer.
#[no_mangle]
pub unsafe extern "C" fn xTimerStop(timer: *mut c_void, ticks_to_wait: u32) -> i32 {
    unsafe {
        xTimerGenericCommand(
            timer,
            TMR_COMMAND_STOP,
            0,
            core::ptr::null_mut(),
            ticks_to_wait,
        )
    }
}

/// `xTimerDelete` — delete a timer.
#[no_mangle]
pub unsafe extern "C" fn xTimerDelete(timer: *mut c_void, ticks_to_wait: u32) -> i32 {
    unsafe {
        xTimerGenericCommand(
            timer,
            TMR_COMMAND_DELETE,
            0,
            core::ptr::null_mut(),
            ticks_to_wait,
        )
    }
}

/// `pvTimerGetTimerID` — return the timer's user argument.
#[no_mangle]
pub unsafe extern "C" fn pvTimerGetTimerID(timer: *mut c_void) -> *mut c_void {
    unsafe {
        if timer.is_null() {
            return core::ptr::null_mut();
        }
        let t = &*(timer as *const SoftwareTimer);
        t.arg
    }
}

/// `xTimerGetTimerDaemonTaskHandle` — return the daemon task handle.
#[no_mangle]
pub unsafe extern "C" fn xTimerGetTimerDaemonTaskHandle() -> *mut c_void {
    unsafe { TIMER_DAEMON_HANDLE }
}

// ---------------------------------------------------------------------------
// Internal
// ---------------------------------------------------------------------------

unsafe fn find_free_slot() -> Option<usize> {
    unsafe {
        for i in 0..MAX_TIMERS {
            if !TIMERS[i].in_use {
                return Some(i);
            }
        }
        None
    }
}

unsafe fn ensure_timer_queue() {
    unsafe {
        if TIMER_QUEUE_HANDLE.is_null() {
            TIMER_QUEUE_HANDLE = queue::xQueueCreate(
                TIMER_QUEUE_LENGTH,
                core::mem::size_of::<DaemonTaskMessage>() as u32,
            );
        }
    }
}

unsafe fn next_timer_wait(now: u64) -> u32 {
    unsafe {
        let mut next_expiry: Option<u64> = None;

        critical::vPortEnterCritical();
        for i in 0..MAX_TIMERS {
            let t = &TIMERS[i];
            if !t.in_use || !t.active {
                continue;
            }
            next_expiry = Some(match next_expiry {
                Some(current) => current.min(t.expiry_tick),
                None => t.expiry_tick,
            });
        }
        critical::vPortExitCritical();

        match next_expiry {
            Some(expiry) if expiry <= now => 0,
            Some(expiry) => (expiry - now).min(u32::MAX as u64) as u32,
            None => 0xFFFF_FFFF,
        }
    }
}

unsafe fn process_expired_timers(now: u64) -> bool {
    unsafe {
        let mut processed = false;

        loop {
            let mut callback: Option<TimerCallback> = None;
            let mut handle = core::ptr::null_mut();

            critical::vPortEnterCritical();
            for i in 0..MAX_TIMERS {
                let t = &mut TIMERS[i];
                if !t.in_use || !t.active || t.expiry_tick > now {
                    continue;
                }

                if t.auto_reload {
                    t.expiry_tick = t.expiry_tick.saturating_add(t.period_ticks as u64);
                } else {
                    t.active = false;
                }

                callback = Some(t.callback);
                handle = t as *mut SoftwareTimer as *mut c_void;
                processed = true;
                break;
            }
            critical::vPortExitCritical();

            let Some(callback) = callback else {
                break;
            };

            callback(handle);
        }

        processed
    }
}

unsafe fn drain_timer_commands() {
    unsafe {
        loop {
            let mut message = DaemonTaskMessage {
                command_id: 0,
                command_value: 0,
                timer: core::ptr::null_mut(),
            };
            let recv = queue::xQueueReceive(
                TIMER_QUEUE_HANDLE,
                &mut message as *mut DaemonTaskMessage as *mut c_void,
                0,
            );
            if recv != PD_PASS {
                break;
            }
            process_timer_command(message);
        }
    }
}

unsafe fn process_timer_command(message: DaemonTaskMessage) {
    unsafe {
        if message.timer.is_null() {
            return;
        }

        let timer = message.timer as *mut SoftwareTimer;
        if !(*timer).in_use && message.command_id != TMR_COMMAND_DELETE {
            return;
        }

        match message.command_id {
            TMR_COMMAND_START_DONT_TRACE
            | TMR_COMMAND_START
            | TMR_COMMAND_START_FROM_ISR
            | TMR_COMMAND_RESET
            | TMR_COMMAND_RESET_FROM_ISR => {
                start_or_reset_timer(timer, message.command_value as u64)
            }
            TMR_COMMAND_STOP | TMR_COMMAND_STOP_FROM_ISR => {
                critical::vPortEnterCritical();
                (*timer).active = false;
                critical::vPortExitCritical();
            }
            TMR_COMMAND_CHANGE_PERIOD | TMR_COMMAND_CHANGE_PERIOD_FROM_ISR => {
                let now = task::get_tick_count();
                critical::vPortEnterCritical();
                (*timer).period_ticks = message.command_value;
                (*timer).expiry_tick = now + message.command_value as u64;
                (*timer).active = true;
                critical::vPortExitCritical();
            }
            TMR_COMMAND_DELETE => {
                critical::vPortEnterCritical();
                (*timer).active = false;
                (*timer).in_use = false;
                critical::vPortExitCritical();
            }
            _ => {}
        }
    }
}

unsafe fn start_or_reset_timer(timer: *mut SoftwareTimer, command_time: u64) {
    unsafe {
        let mut callback: Option<TimerCallback> = None;
        let mut handle = core::ptr::null_mut();

        critical::vPortEnterCritical();
        let t = &mut *timer;
        let period = t.period_ticks as u64;
        let next_expiry = command_time.saturating_add(period);
        let now = task::get_tick_count();

        if period != 0 && next_expiry <= now && now.saturating_sub(command_time) >= period {
            if t.auto_reload {
                t.expiry_tick = next_expiry.saturating_add(period);
                t.active = true;
            } else {
                t.active = false;
            }
            callback = Some(t.callback);
            handle = timer as *mut c_void;
        } else {
            t.expiry_tick = next_expiry;
            t.active = true;
        }
        critical::vPortExitCritical();

        if let Some(callback) = callback {
            callback(handle);
        }
    }
}
