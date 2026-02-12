//! Embassy executor integration for ESP8266 with FreeRTOS compat scheduler.
//!
//! Replaces `arch-spin`'s busy-loop executor with a task-blocking executor
//! that yields the CPU when no embassy tasks are ready.
//!
//! # Design
//!
//! The core problem with `arch-spin` is that its `loop { poll() }` never
//! yields. On ESP8266, the WiFi blob's FreeRTOS tasks must preempt and run.
//! The spin loop also breaks embassy's waker mechanism: `wake()` enqueues
//! the task, but the executor's `dequeue_all()` timing window under
//! preemption means the enqueue is invisible to the current poll cycle.
//!
//! In the ESP8266 RTOS SDK, ordinary tasks block through FreeRTOS objects
//! such as queues, event groups, and `vTaskDelay()`, while CPU sleep lives in
//! the idle hook (`esp_internal_idle_hook()` / `esp_vApplicationWaitiHook()`).
//! Our async `uiT` main task therefore should not execute raw `waiti`
//! directly. Instead it keeps a simple atomic pending flag and blocks
//! indefinitely through the compat scheduler until the pender wakes it.

use core::marker::PhantomData;

use embassy_executor::raw;
pub use embassy_executor::Spawner;
use portable_atomic::{AtomicBool, AtomicUsize, Ordering};

// ---------------------------------------------------------------------------
// Pender flag
// ---------------------------------------------------------------------------

/// Atomic flag set by the pender, cleared by the executor after `poll()`.
/// Public so SoftIsrHdl can check if the soft interrupt was from the pender.
pub static PEND: AtomicBool = AtomicBool::new(false);
static EXECUTOR_TASK: AtomicUsize = AtomicUsize::new(usize::MAX);

#[inline(always)]
fn is_in_isr() -> bool {
    let ps: u32;
    unsafe { core::arch::asm!("rsr {0}, ps", out(reg) ps, options(nomem, nostack)) };
    (ps & 0xF) > 0
}

/// The pender — called by embassy whenever a task's waker is invoked.
///
/// Sets the atomic `PEND` flag and wakes the blocked executor task through the
/// compat scheduler, matching the SDK's event-driven task shape more closely
/// than a periodic delay loop.
#[export_name = "__pender"]
fn __pender(_context: *mut ()) {
    PEND.store(true, Ordering::Release);

    let task_idx = EXECUTOR_TASK.load(Ordering::Acquire);
    if task_idx == usize::MAX {
        return;
    }

    unsafe {
        crate::wifi::compat::critical::vPortEnterCritical();
        let need_switch = crate::wifi::compat::task::wake_task(task_idx);
        crate::wifi::compat::critical::vPortExitCritical();

        if need_switch {
            if is_in_isr() {
                crate::wifi::compat::port::portYIELD_FROM_ISR();
            } else {
                crate::wifi::compat::port::PendSV(1);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Executor
// ---------------------------------------------------------------------------

/// Embassy executor that blocks the owning compat task when idle.
///
/// Drop-in replacement for `embassy_executor::Executor` (from `arch-spin`).
pub struct Executor {
    inner: raw::Executor,
    not_send: PhantomData<*mut ()>,
}

impl Executor {
    /// Create a new sleeping executor.
    pub fn new() -> Self {
        Self {
            inner: raw::Executor::new(core::ptr::null_mut()),
            not_send: PhantomData,
        }
    }

    /// Run the executor forever.
    ///
    /// The `init` closure is called once with a [`Spawner`] to spawn the
    /// initial task(s). After `init` returns, the executor enters its main
    /// loop.
    ///
    /// This function never returns.
    pub fn run(&'static mut self, init: impl FnOnce(Spawner)) -> ! {
        let task_idx = unsafe { crate::wifi::compat::task::current_task_index() };
        EXECUTOR_TASK.store(task_idx, Ordering::Release);
        init(self.inner.spawner());

        loop {
            // If the pender signaled (or this is the first iteration after
            // spawn), poll the executor to process ready tasks.
            if PEND.swap(false, Ordering::Acquire) {
                unsafe { self.inner.poll() };
                // After poll, loop back immediately to check PEND again
                // (a wake might have fired during poll).
                continue;
            }

            // No pending work. Block the executor task indefinitely and let
            // the pender wake it directly, mirroring SDK event waits.
            unsafe {
                crate::wifi::compat::critical::vPortEnterCritical();
                if !PEND.load(Ordering::Acquire) {
                    crate::wifi::compat::task::block_current_task(0);
                    crate::wifi::compat::critical::vPortExitCritical();
                    crate::wifi::compat::port::PendSV(1);
                } else {
                    crate::wifi::compat::critical::vPortExitCritical();
                }
            }
        }
    }
}
