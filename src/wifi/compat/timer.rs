//! One-shot timers used by the native PMKSA cache.
//!
//! Timer state belongs to this runtime; a coalescing wake queue only notifies
//! its daemon that a deadline changed. Stop/delete take effect before returning,
//! and an expired timer is disarmed before its callback can rearm or delete it.
//!
//! The retained native caller is `pmksa_cache.c`. Its callback may free cache
//! entries, post a nonblocking deauthentication event, and rearm this timer. It
//! must not block. Dispatch suspends task switching until the callback returns:
//! native Wi-Fi stop frees PMKSA entries before deleting their timer, sometimes
//! with interrupts already masked, so waiting during delete cannot protect that
//! lifetime. Hardware interrupts and NMI remain enabled during dispatch.
//!
//! The SDK's separate ETS timers still run through FRC2 and the Wi-Fi task.
//! The station's global `gWpaSm` owns the only PMKSA cache; AP clients use ETS
//! timers, so simultaneous station/AP operation does not require another slot.

use core::ffi::c_void;

const TICK_MICROS: u64 = 10_000;
type TimerCallback = unsafe extern "C" fn(*mut c_void);

struct OneShotTimer {
    callback: Option<TimerCallback>,
    arg: *mut c_void,
    expiry: Option<u64>,
}

impl OneShotTimer {
    const fn new() -> Self {
        Self {
            callback: None,
            arg: core::ptr::null_mut(),
            expiry: None,
        }
    }

    fn create(&mut self, callback: TimerCallback, arg: *mut c_void) -> bool {
        if self.callback.is_some() {
            return false;
        }
        *self = Self {
            callback: Some(callback),
            arg,
            expiry: None,
        };
        true
    }

    fn start(&mut self, now: u64, ticks: u32) -> bool {
        if self.callback.is_none() {
            return false;
        }
        self.expiry = Some(now.saturating_add(u64::from(ticks)));
        true
    }

    fn stop(&mut self) -> bool {
        if self.callback.is_none() {
            return false;
        }
        self.expiry = None;
        true
    }

    fn delete(&mut self) -> bool {
        if self.callback.is_none() {
            return false;
        }
        *self = Self::new();
        true
    }

    fn take_expired(&mut self, now: u64) -> Option<(TimerCallback, *mut c_void)> {
        if self.expiry? > now {
            return None;
        }
        self.expiry = None;
        // Copy the callback and argument: no timer borrow survives dispatch.
        self.callback.map(|callback| (callback, self.arg))
    }

    fn wait_ticks(&self, now: u64) -> u32 {
        self.expiry
            // u32::MAX means an indefinite queue wait, not a finite deadline.
            .map(|expiry| expiry.saturating_sub(now).min(u64::from(u32::MAX - 1)) as u32)
            .unwrap_or(u32::MAX)
    }
}

fn timeout_ticks(micros: u64) -> Option<u32> {
    if micros % TICK_MICROS != 0 {
        return None;
    }
    u32::try_from(micros / TICK_MICROS)
        .ok()
        .filter(|ticks| *ticks != 0)
}

#[cfg(target_arch = "xtensa")]
mod target {
    use core::ffi::c_char;

    use super::*;
    use crate::wifi::compat::{critical, queue, task};
    use esp8266_wifi_sys::{ESP_ERR_INVALID_ARG, ESP_ERR_NO_MEM, ESP_OK};

    // Preserve the qualified daemon stack and SDK timer-task priority.
    const DAEMON_STACK_BYTES: u16 = 768;
    const DAEMON_PRIORITY: u32 = 2;

    static mut TIMER: OneShotTimer = OneShotTimer::new();
    static mut WAKE_QUEUE: *mut c_void = core::ptr::null_mut();

    #[repr(C)]
    pub struct CreateArgs {
        callback: Option<TimerCallback>,
        arg: *mut c_void,
        dispatch_method: u32,
        name: *const c_char,
    }

    const _: () = {
        assert!(core::mem::size_of::<CreateArgs>() == 16);
        assert!(core::mem::align_of::<CreateArgs>() == 4);
        assert!(core::mem::offset_of!(CreateArgs, callback) == 0);
        assert!(core::mem::offset_of!(CreateArgs, arg) == 4);
        assert!(core::mem::offset_of!(CreateArgs, dispatch_method) == 8);
        assert!(core::mem::offset_of!(CreateArgs, name) == 12);
    };

    /// The closure must not invoke callbacks, block, or retain timer references.
    unsafe fn with_timer<R>(operation: impl FnOnce(&mut OneShotTimer) -> R) -> R {
        unsafe {
            critical::vPortEnterCritical();
            let result = operation(&mut *core::ptr::addr_of_mut!(TIMER));
            critical::vPortExitCritical();
            result
        }
    }

    unsafe fn notify_daemon() {
        // A full queue already carries the required wakeup. No state or timer
        // pointers are queued, so delete/recreate cannot leave stale commands.
        let _ = unsafe { queue::xQueueGenericSend(WAKE_QUEUE, core::ptr::null(), 0, 0) };
    }

    unsafe extern "C" fn timer_daemon_task(_: *mut c_void) {
        loop {
            unsafe {
                task::vTaskSuspendAll();
                let now = task::get_tick_count();
                let callback = with_timer(|timer| timer.take_expired(now));
                if let Some((callback, arg)) = callback {
                    callback(arg);
                    let _ = task::xTaskResumeAll();
                    continue;
                }
                let wait = with_timer(|timer| timer.wait_ticks(now));
                let _ = task::xTaskResumeAll();
                let _ = queue::xQueueReceive(WAKE_QUEUE, core::ptr::null_mut(), wait);
            }
        }
    }

    /// Allocate mandatory runtime infrastructure before scheduler activation.
    pub(crate) unsafe fn create_timer_daemon() -> i32 {
        unsafe {
            WAKE_QUEUE = queue::xQueueCreate(1, 0);
            if WAKE_QUEUE.is_null() {
                return task::PD_FALSE;
            }
            task::xTaskCreate(
                timer_daemon_task,
                core::ptr::null(),
                DAEMON_STACK_BYTES,
                core::ptr::null_mut(),
                DAEMON_PRIORITY,
                core::ptr::null_mut(),
            )
        }
    }

    /// Task startup rollback releases the daemon stack; release its wake queue.
    pub(crate) unsafe fn rollback_startup() {
        unsafe {
            if !WAKE_QUEUE.is_null() {
                queue::vQueueDelete(WAKE_QUEUE);
                WAKE_QUEUE = core::ptr::null_mut();
            }
        }
    }

    /// Native PMKSA ABI; task context and a nonblocking callback are required.
    #[no_mangle]
    pub unsafe extern "C" fn esp_timer_create(
        args: *const CreateArgs,
        out_handle: *mut *mut c_void,
    ) -> i32 {
        if args.is_null() || out_handle.is_null() {
            return ESP_ERR_INVALID_ARG;
        }
        let args = unsafe { &*args };
        let Some(callback) = args.callback else {
            return ESP_ERR_INVALID_ARG;
        };
        if args.dispatch_method != 0 {
            return ESP_ERR_INVALID_ARG;
        }
        unsafe {
            with_timer(|timer| {
                if timer.create(callback, args.arg) {
                    out_handle.write((timer as *mut OneShotTimer).cast());
                    ESP_OK
                } else {
                    ESP_ERR_NO_MEM
                }
            })
        }
    }

    /// Start or replace the deadline, including from the timer's own callback.
    #[no_mangle]
    pub unsafe extern "C" fn esp_timer_start_once(handle: *mut c_void, micros: u64) -> i32 {
        let Some(ticks) = timeout_ticks(micros) else {
            return ESP_ERR_INVALID_ARG;
        };
        unsafe {
            let valid = with_timer(|timer| {
                handle == (timer as *mut OneShotTimer).cast()
                    && timer.start(task::get_tick_count(), ticks)
            });
            if !valid {
                return ESP_ERR_INVALID_ARG;
            }
            notify_daemon();
        }
        ESP_OK
    }

    #[no_mangle]
    pub unsafe extern "C" fn esp_timer_stop(handle: *mut c_void) -> i32 {
        let valid = unsafe {
            with_timer(|timer| handle == (timer as *mut OneShotTimer).cast() && timer.stop())
        };
        if valid {
            ESP_OK
        } else {
            ESP_ERR_INVALID_ARG
        }
    }

    #[no_mangle]
    pub unsafe extern "C" fn esp_timer_delete(handle: *mut c_void) -> i32 {
        let valid = unsafe {
            with_timer(|timer| handle == (timer as *mut OneShotTimer).cast() && timer.delete())
        };
        if valid {
            ESP_OK
        } else {
            ESP_ERR_INVALID_ARG
        }
    }
}

#[cfg(target_arch = "xtensa")]
pub(crate) use target::{create_timer_daemon, rollback_startup};

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" fn callback(_: *mut c_void) {}

    #[test]
    fn expiration_allows_rearm_without_a_trailing_stop() {
        let mut timer = OneShotTimer::new();
        assert!(timer.create(callback, core::ptr::null_mut()));
        assert!(timer.start(0, 2));
        assert!(timer.take_expired(1).is_none());
        assert!(timer.take_expired(2).is_some());
        assert!(timer.take_expired(2).is_none());
        // The callback's rearm survives returning from the previous dispatch.
        assert!(timer.start(2, 3));
        assert_eq!(timer.wait_ticks(2), 3);
        assert!(timer.take_expired(4).is_none());
        assert!(timer.take_expired(5).is_some());
    }

    #[test]
    fn coalesced_wakeup_observes_latest_start_stop_and_delete() {
        let mut timer = OneShotTimer::new();
        assert!(timer.create(callback, core::ptr::null_mut()));
        assert!(timer.start(0, 1));
        assert!(timer.stop());
        assert!(timer.start(0, 3));
        assert!(timer.take_expired(1).is_none());
        assert!(timer.start(1, 5));
        assert!(timer.take_expired(3).is_none());
        assert!(timer.delete());
        assert!(!timer.delete());
        assert!(timer.create(callback, core::ptr::null_mut()));
        // A wake already queued for the old timer cannot act on this new slot.
        assert!(timer.take_expired(6).is_none());
        assert_eq!(timer.wait_ticks(6), u32::MAX);
        assert!(timer.stop());
    }

    #[test]
    fn single_owner_can_release_and_reuse_timer_during_callback() {
        let mut timer = OneShotTimer::new();
        assert!(timer.create(callback, core::ptr::null_mut()));
        assert!(!timer.create(callback, core::ptr::null_mut()));
        assert!(timer.start(0, 1));
        assert!(timer.take_expired(1).is_some());
        assert!(timer.delete());
        assert!(timer.create(callback, core::ptr::null_mut()));
        assert!(timer.take_expired(1).is_none());
    }

    #[test]
    fn finite_timeouts_never_become_indefinite_queue_waits() {
        assert_eq!(timeout_ticks(0), None);
        assert_eq!(timeout_ticks(9_999), None);
        assert_eq!(timeout_ticks(10_001), None);
        assert_eq!(timeout_ticks(10_000), Some(1));
        assert_eq!(
            timeout_ticks(u64::from(u32::MAX) * TICK_MICROS),
            Some(u32::MAX)
        );
        assert_eq!(timeout_ticks((u64::from(u32::MAX) + 1) * TICK_MICROS), None);
        let mut timer = OneShotTimer::new();
        assert!(timer.create(callback, core::ptr::null_mut()));
        assert!(timer.start(0, u32::MAX));
        assert_eq!(timer.wait_ticks(0), u32::MAX - 1);
        assert_eq!(timer.wait_ticks(u64::from(u32::MAX)), 0);
    }
}
