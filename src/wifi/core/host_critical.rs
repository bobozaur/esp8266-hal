extern crate std;

use std::cell::{Cell, RefCell};
use std::sync::{Mutex, MutexGuard};

static GLOBAL_MUTEX: Mutex<()> = Mutex::new(());

std::thread_local! {
    static IS_LOCKED: Cell<bool> = const { Cell::new(false) };
    static THREAD_GUARD: RefCell<Option<MutexGuard<'static, ()>>> = const { RefCell::new(None) };
}

struct HostCriticalSection;
critical_section::set_impl!(HostCriticalSection);

unsafe impl critical_section::Impl for HostCriticalSection {
    unsafe fn acquire() -> critical_section::RawRestoreState {
        IS_LOCKED.with(|locked| {
            if locked.get() {
                return 1;
            }

            let guard = match GLOBAL_MUTEX.lock() {
                Ok(guard) => guard,
                Err(error) => error.into_inner(),
            };
            THREAD_GUARD.with(|slot| *slot.borrow_mut() = Some(guard));
            locked.set(true);
            0
        })
    }

    unsafe fn release(nested: critical_section::RawRestoreState) {
        if nested != 0 {
            return;
        }

        THREAD_GUARD.with(|slot| drop(slot.borrow_mut().take()));
        IS_LOCKED.with(|locked| locked.set(false));
    }
}
