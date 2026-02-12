//! Embassy time driver backed by `esp_timer_get_time()`.
//!
//! `esp_timer_get_time()` returns microseconds since boot, which maps directly
//! to embassy-time's 1 MHz tick rate (`tick-hz-1_000_000`).
//!
//! # Architecture (embassy-time-driver 0.1.0)
//!
//! The 0.1.0 driver API uses an alarm-based model:
//!
//! - `allocate_alarm()` — reserves an alarm slot
//! - `set_alarm_callback()` — registers a callback + context pointer
//! - `set_alarm()` — arms an alarm for a given timestamp
//!
//! The timer queue layer (`embassy-time-queue-driver`) sits on top and
//! manages `Waker` scheduling using these primitives.
//!
//! # Alarm resolution
//!
//! Alarms are checked from two sources:
//!
//! 1. The FreeRTOS compat tick ISR (every 10ms) via `poll_alarms()`.
//! 2. When `set_alarm()` arms an alarm closer than the next tick, it adjusts
//!    CCOMPARE to fire earlier, giving microsecond-resolution wakeups.
//!
//! All access to alarm state is protected by critical sections to prevent
//! races between the tick ISR and task-level code.

use core::cell::UnsafeCell;
use core::ptr;

use embassy_time_driver::{AlarmHandle, Driver};

extern "C" {
    fn esp_timer_get_time() -> i64;
}

const MAX_ALARMS: usize = 8;

struct AlarmState {
    /// Timestamp at which this alarm should fire (u64::MAX = disarmed).
    at: u64,
    /// Callback to invoke when the alarm fires.
    callback: fn(*mut ()),
    /// Opaque context pointer passed to the callback.
    ctx: *mut (),
}

struct EspTimeDriver {
    alarms: UnsafeCell<[AlarmState; MAX_ALARMS]>,
    /// Number of alarms that have been allocated.
    alarm_count: UnsafeCell<usize>,
}

// SAFETY: ESP8266 is single-core, and all mutable access to alarm state is
// protected by critical sections (interrupt disable).
unsafe impl Send for EspTimeDriver {}
unsafe impl Sync for EspTimeDriver {}

/// Dummy no-op callback used as the initial value before a real callback is set.
fn noop_callback(_ctx: *mut ()) {}

impl EspTimeDriver {
    const fn new() -> Self {
        const EMPTY: AlarmState = AlarmState {
            at: u64::MAX,
            callback: noop_callback,
            ctx: ptr::null_mut(),
        };
        Self {
            alarms: UnsafeCell::new([EMPTY; MAX_ALARMS]),
            alarm_count: UnsafeCell::new(0),
        }
    }

    /// Get mutable access to alarms. Caller must be in a critical section.
    #[allow(clippy::mut_from_ref)]
    fn alarms(&self) -> &mut [AlarmState; MAX_ALARMS] {
        unsafe { &mut *self.alarms.get() }
    }

    /// Find the earliest alarm deadline among all allocated alarms.
    /// Returns `u64::MAX` if no alarms are armed.
    fn earliest_deadline(&self) -> u64 {
        let count = unsafe { *self.alarm_count.get() };
        let alarms = self.alarms();
        let mut earliest = u64::MAX;
        for i in 0..count {
            if alarms[i].at < earliest {
                earliest = alarms[i].at;
            }
        }
        earliest
    }

    /// Check all allocated alarms and fire any that have expired.
    #[inline(never)]
    fn check_alarms(&self) {
        let now = self.now();

        critical_section::with(|_| unsafe {
            let count = (*self.alarm_count.get()).min(MAX_ALARMS);
            let mut alarm = (*self.alarms.get()).as_mut_ptr();
            let end = alarm.add(count);

            while alarm < end {
                let state = &mut *alarm;
                if state.at <= now {
                    state.at = u64::MAX;
                    let cb = state.callback;
                    let ctx = state.ctx;
                    // Call outside the borrow but still inside the critical
                    // section — the callback may re-arm the alarm.
                    cb(ctx);
                }

                alarm = alarm.add(1);
            }
        });
    }

    /// If the earliest alarm deadline is sooner than the next tick, shorten
    /// CCOMPARE so the tick ISR fires early and dispatches the alarm.
    fn maybe_shorten_tick(&self) {
        let earliest = critical_section::with(|_| self.earliest_deadline());
        if earliest == u64::MAX {
            return;
        }

        let now = self.now();
        if earliest <= now {
            // Already expired — trigger a software interrupt to process it
            // immediately. We can't call check_alarms directly because we
            // might be inside set_alarm's critical section.
            poke_tick_timer(0);
            return;
        }

        let us_until = earliest - now;
        // Only shorten if alarm is within the next tick period (10ms = 10000us).
        if us_until < 10_000 {
            // Convert microseconds to CPU cycles.
            let ticks_per_us = unsafe { g_esp_ticks_per_us };
            if ticks_per_us > 0 {
                let cycles = (us_until as u32).saturating_mul(ticks_per_us);
                // Read current CCOUNT and set CCOMPARE to fire sooner.
                poke_tick_timer(cycles);
            }
        }
    }
}

extern "C" {
    static g_esp_ticks_per_us: u32;
}

/// Adjust CCOMPARE to fire after `cycles` CPU cycles from now.
/// If `cycles` is 0, fires as soon as possible.
fn poke_tick_timer(cycles: u32) {
    // Only shorten, never extend past the normal tick.
    let current_ccompare: u32;
    let ccount: u32;
    unsafe {
        core::arch::asm!("rsr {0}, ccompare0", out(reg) current_ccompare, options(nomem, nostack));
        core::arch::asm!("rsr {0}, ccount", out(reg) ccount, options(nomem, nostack));
    }

    let target = if cycles == 0 {
        ccount.wrapping_add(50) // fire ASAP (small margin for pipeline)
    } else {
        ccount.wrapping_add(cycles)
    };

    // Only update if our target is sooner than the existing CCOMPARE.
    // Use wrapping subtraction to handle counter wrap.
    let remaining_normal = current_ccompare.wrapping_sub(ccount);
    let remaining_new = target.wrapping_sub(ccount);

    if remaining_new < remaining_normal {
        unsafe {
            core::arch::asm!("wsr {0}, ccompare0", in(reg) target, options(nomem, nostack));
        }
    }
}

impl Driver for EspTimeDriver {
    fn now(&self) -> u64 {
        unsafe { esp_timer_get_time() as u64 }
    }

    unsafe fn allocate_alarm(&self) -> Option<AlarmHandle> {
        critical_section::with(|_| {
            let count = unsafe { &mut *self.alarm_count.get() };
            if *count < MAX_ALARMS {
                let id = *count as u8;
                *count += 1;
                Some(unsafe { AlarmHandle::new(id) })
            } else {
                None
            }
        })
    }

    fn set_alarm_callback(&self, alarm: AlarmHandle, callback: fn(*mut ()), ctx: *mut ()) {
        critical_section::with(|_| {
            let alarms = self.alarms();
            let idx = alarm.id() as usize;
            alarms[idx].callback = callback;
            alarms[idx].ctx = ctx;
        });
    }

    fn set_alarm(&self, alarm: AlarmHandle, timestamp: u64) -> bool {
        let armed = critical_section::with(|_| {
            let alarms = self.alarms();
            let idx = alarm.id() as usize;
            let now = self.now();

            if timestamp <= now {
                // Already expired — don't arm, caller will handle it.
                false
            } else {
                alarms[idx].at = timestamp;
                true
            }
        });

        if armed {
            // If this alarm is sooner than the next tick, shorten the
            // CCOMPARE interval so we wake up in time.
            self.maybe_shorten_tick();
        }

        armed
    }
}

embassy_time_driver::time_driver_impl!(static DRIVER: EspTimeDriver = EspTimeDriver::new());

/// Poll expired alarms. Called automatically from the tick ISR, and can
/// also be called by the user for lower-latency wakeup.
pub fn poll_alarms() {
    DRIVER.check_alarms();
}
