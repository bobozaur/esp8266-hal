extern crate std;

use std::cell::Cell;
use std::mem::MaybeUninit;
use std::sync::{Mutex, MutexGuard};

static GLOBAL_MUTEX: Mutex<()> = Mutex::new(());
static mut GLOBAL_GUARD: MaybeUninit<MutexGuard<'static, ()>> = MaybeUninit::uninit();

std::thread_local!(static IS_LOCKED: Cell<bool> = const { Cell::new(false) });

struct HostCriticalSection;
critical_section::set_impl!(HostCriticalSection);

unsafe impl critical_section::Impl for HostCriticalSection {
    unsafe fn acquire() -> critical_section::RawRestoreState {
        IS_LOCKED.with(|locked| {
            if locked.get() {
                return 1;
            }

            locked.set(true);
            let guard = match GLOBAL_MUTEX.lock() {
                Ok(guard) => guard,
                Err(error) => error.into_inner(),
            };
            unsafe { GLOBAL_GUARD.write(guard) };
            0
        })
    }

    unsafe fn release(nested: critical_section::RawRestoreState) {
        if nested != 0 {
            return;
        }

        #[allow(let_underscore_lock)]
        let _ = unsafe { GLOBAL_GUARD.assume_init_read() };
        IS_LOCKED.with(|locked| locked.set(false));
    }
}
