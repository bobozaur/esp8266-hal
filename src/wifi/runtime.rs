//! Explicit, persistent SDK runtime initialization.
//!
//! The runtime reserves FRC2, the CPU compare timer and the software interrupt.
//! It captures the calling CPU stack as the application task; that stack must
//! remain excluded from the application heap for the rest of this boot. The
//! allocator is supplied by the application and must already be initialized.
//! Stopping Wi-Fi leaves the runtime, its tasks and timer infrastructure alive.
//! Modem power saving is supported through the controller. Direct native CPU
//! light-sleep calls and raw CPU clock-register changes are unsupported: they
//! bypass the runtime's cycle-counter and timer ownership.

use core::mem::MaybeUninit;

use portable_atomic::{AtomicBool, Ordering};

use crate::timer::Timer2;
use crate::wifi_core::{allocator::RuntimeAllocator, WifiAllocator};

use super::compat::{entry, memory, port};

static TAKEN: AtomicBool = AtomicBool::new(false);
static mut RUNTIME: MaybeUninit<Runtime> = MaybeUninit::uninit();

/// Failure to initialize the once-per-boot SDK runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeError {
    /// This boot has already attempted runtime initialization.
    AlreadyInitialized,
    /// Initialization requires ordinary task context with interrupts enabled.
    InvalidContext,
    /// A mandatory scheduler task or queue could not be allocated.
    OutOfMemory,
}

/// A permanent runtime token. There is no runtime teardown operation.
///
/// Obtain this token before constructing Wi-Fi or running the optional executor.
/// The application task stays on its original stack; the HAL never reclaims it.
pub struct Runtime {
    _timer: Timer2,
}

impl Runtime {
    /// Initialize newlib and start the SDK scheduler explicitly.
    ///
    /// Call once, from ordinary main/task context with interrupts enabled. The
    /// provider must meet [`WifiAllocator`]'s safety contract. `timer` is consumed
    /// even on error; an initialization failure requires reset before retrying.
    /// This does not initialize or start the Wi-Fi radio.
    pub fn init(
        timer: Timer2,
        allocator: &'static dyn WifiAllocator,
    ) -> Result<&'static Self, RuntimeError> {
        let ps: u32;
        // SAFETY: reading PS has no side effects and detects ISR/critical context
        // even before an application interrupt enters the SDK dispatch table.
        unsafe { core::arch::asm!("rsr {0}, ps", out(reg) ps, options(nomem, nostack)) };
        if ps & 0x1f != 0 {
            return Err(RuntimeError::InvalidContext);
        }
        if TAKEN.swap(true, Ordering::AcqRel) {
            return Err(RuntimeError::AlreadyInitialized);
        }

        // SAFETY: initialization has one owner, the provider is permanent, and
        // no SDK task/interrupt can allocate before scheduler preparation.
        critical_section::with(|_| unsafe {
            memory::install(allocator);
            entry::init_newlib_reent();
            port::start_compat_scheduler().map_err(|()| RuntimeError::OutOfMemory)?;
            core::ptr::addr_of_mut!(RUNTIME).write(MaybeUninit::new(Self { _timer: timer }));
            Ok((&*core::ptr::addr_of!(RUNTIME)).assume_init_ref())
        })
    }

    pub(crate) fn allocator(&self) -> RuntimeAllocator {
        RuntimeAllocator::new(memory::allocator())
    }
}
