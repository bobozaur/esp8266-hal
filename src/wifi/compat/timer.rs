//! FreeRTOS-compatible software timer implementation.
//!
//! Timers are stored in a flat array. The tick ISR marks expired timers, and
//! their callbacks are executed from a dedicated timer service task (not from
//! ISR context). This matches FreeRTOS behavior where timer callbacks run in
//! the timer daemon task and can safely call blocking APIs.

use core::ffi::c_void;
use core::sync::atomic::{AtomicU8, Ordering};

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
// Match SDK `configTIMER_TASK_STACK_DEPTH = CONFIG_FREERTOS_TIMER_STACKSIZE`
// with `CONFIG_FREERTOS_TIMER_STACKSIZE = 2048` bytes on this branch.
const TIMER_DAEMON_STACK_BYTES: u16 = 2048;
// Match the generated `sdkconfig.h` on this branch:
// `CONFIG_ESP_TIMER_TASK_STACK_SIZE = 2048`.
// `components/esp_common/include/esp_task.h` defines
// `ESP_TASK_TIMER_STACK = CONFIG_ESP_TIMER_TASK_STACK_SIZE + TASK_EXTRA_STACK_SIZE`.
const ETS_TIMER_TASK_STACK_BYTES: u16 = 2048;
// Match SDK `ESP_TASK_TIMER_PRIO = ESP_TASK_PRIO_MAX - 3`.
const ETS_TIMER_TASK_PRIORITY: u32 = task::MAX_PRIORITIES - 3;
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
static mut ETS_TIMER_TASK_HANDLE: *mut c_void = core::ptr::null_mut();
static ETS_TIMER_CONNECT_TRACE: AtomicU8 = AtomicU8::new(0);

unsafe extern "C" fn empty_callback(_: *mut c_void) {}

// ---------------------------------------------------------------------------
// Timer daemon task
// ---------------------------------------------------------------------------

extern "C" {
    /// ROM-based ETS timer processing.
    fn ets_timer_proc();
    /// WiFi blob NMI handler — processes received frames.
    fn wDev_ProcessFiq();
}

/// Dedicated task for processing ROM ETS timers.
/// Runs at priority 2 (above idle, below ppTask at 13) so timer callbacks
/// can properly interact with higher-priority blob tasks via yielding.
/// Diagnostic counter for ets_timer_task iterations.
#[no_mangle]
pub static mut g_ets_timer_task_count: u32 = 0;

unsafe extern "C" fn ets_timer_task(_param: *mut c_void) {
    loop {
        unsafe {
            g_ets_timer_task_count += 1;
            let trace_armed = ETS_TIMER_CONNECT_TRACE.load(Ordering::Acquire) == 1;
            if trace_armed {
                ETS_TIMER_CONNECT_TRACE.store(2, Ordering::Release);
                crate::logger::write_byte(b'u');
            }
            // SDK ets_sys.h documents os_timer callbacks with scheduling
            // disabled. Mirror that around the task-dispatched timer pass.
            task::vTaskSuspendAll();
            ets_timer_proc();
            task::xTaskResumeAll();
            if trace_armed {
                crate::logger::write_byte(b'U');
                ETS_TIMER_CONNECT_TRACE.store(3, Ordering::Release);
            }
            task::vTaskDelay(1); // 1 tick = 10ms at 100Hz
        }
    }
}

/// Timer daemon task — processes queued timer commands and expirations.
///
/// Runs in task context like SDK `prvTimerTask(...)`, so timer callbacks can
/// safely call blocking APIs.
unsafe extern "C" fn timer_daemon_task(_param: *mut c_void) {
    crate::println!("Tmr entry");
    loop {
        unsafe {
            crate::println!("Tmr 0");
            let now = task::get_tick_count();
            crate::println!("Tmr 1");
            if process_expired_timers(now) {
                crate::println!("Tmr exp");
                continue;
            }

            crate::println!("Tmr 2");
            let wait_ticks = next_timer_wait(now);
            let mut message = DaemonTaskMessage {
                command_id: 0,
                command_value: 0,
                timer: core::ptr::null_mut(),
            };

            crate::println!("Tmr wait");
            let recv = queue::xQueueReceive(
                TIMER_QUEUE_HANDLE,
                &mut message as *mut DaemonTaskMessage as *mut c_void,
                wait_ticks,
            );
            crate::println!("Tmr recv {}", recv);

            if recv == PD_PASS {
                process_timer_command(message);
                drain_timer_commands();
            }
        }
    }
}

/// Create the timer daemon task. Called from `xPortStartScheduler`.
pub(crate) unsafe fn create_timer_daemon() {
    unsafe {
        xTimerCreateTimerTask();
    }
}

/// Create the ETS timer processing task. Must be called AFTER esp_wifi_init
/// (which runs ets_timer_init to set up the ROM timer list).
pub(crate) unsafe fn start_ets_timer_task() -> i32 {
    unsafe {
        if !ETS_TIMER_TASK_HANDLE.is_null() {
            return PD_PASS;
        }

        task::xTaskCreate(
            ets_timer_task,
            b"ETStmr\0".as_ptr() as *const _,
            ETS_TIMER_TASK_STACK_BYTES,
            core::ptr::null_mut(),
            ETS_TIMER_TASK_PRIORITY,
            &raw mut ETS_TIMER_TASK_HANDLE,
        )
    }
}

/// Emit one-shot `u/U` markers around the next ETS timer processing pass.
pub fn arm_ets_timer_trace_once() {
    ETS_TIMER_CONNECT_TRACE.store(1, Ordering::Release);
}

// ---------------------------------------------------------------------------
// Tick processing (called from xTaskIncrementTick in ISR context)
// ---------------------------------------------------------------------------

/// Timer expiry is handled in task context by `Tmr Svc`, mirroring SDK
/// `prvTimerTask(...)` rather than firing callbacks from the tick ISR.
pub(crate) unsafe fn process_timers(_tick: u64) {}

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
