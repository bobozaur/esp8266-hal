use core::cell::UnsafeCell;
use core::task::Waker;

pub(crate) struct CallbackWaker {
    slot: UnsafeCell<Option<Waker>>,
}

// SAFETY: Access is synchronized through `critical_section::with`, and the
// callback path is single-core on ESP8266.
unsafe impl Send for CallbackWaker {}
// SAFETY: See above; mutation is serialized by critical sections.
unsafe impl Sync for CallbackWaker {}

impl CallbackWaker {
    pub const fn new() -> Self {
        Self {
            slot: UnsafeCell::new(None),
        }
    }

    pub fn register(&self, waker: &Waker) {
        // Clone/drop vtables are caller code and may reenter registration.
        let candidate = waker.clone();
        let retired = critical_section::with(|_| {
            let slot = unsafe { &mut *self.slot.get() };
            if slot
                .as_ref()
                .is_some_and(|existing| existing.will_wake(&candidate))
            {
                Some(candidate)
            } else {
                slot.replace(candidate)
            }
        });
        drop(retired);
    }

    pub fn wake(&self) {
        let waker = critical_section::with(|_| unsafe { (*self.slot.get()).take() });
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use core::{
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
        task::{RawWaker, RawWakerVTable},
    };
    use std::{sync::Arc, task::Wake};
    struct Noop;
    impl Wake for Noop {
        fn wake(self: Arc<Self>) {}
    }
    struct Reentrant {
        slot: *const CallbackWaker,
        on_clone: AtomicBool,
        on_drop: AtomicBool,
        visits: AtomicUsize,
    }
    unsafe fn clone(data: *const ()) -> RawWaker {
        let context = unsafe { &*data.cast::<Reentrant>() };
        if context.on_clone.swap(false, Ordering::SeqCst) {
            context.visits.fetch_add(1, Ordering::SeqCst);
            unsafe { &*context.slot }.register(&Arc::new(Noop).into());
        }
        RawWaker::new(data, &VTABLE)
    }
    unsafe fn drop_raw(data: *const ()) {
        let context = unsafe { &*data.cast::<Reentrant>() };
        if context.on_drop.swap(false, Ordering::SeqCst) {
            context.visits.fetch_add(1, Ordering::SeqCst);
            unsafe { &*context.slot }.register(&Arc::new(Noop).into());
        }
    }
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, drop_raw, drop_raw, drop_raw);
    #[test]
    fn clone_and_drop_vtables_can_reenter_registration() {
        let slot = CallbackWaker::new();
        let context = Reentrant {
            slot: &slot,
            on_clone: AtomicBool::new(true),
            on_drop: AtomicBool::new(true),
            visits: AtomicUsize::new(0),
        };
        // Context and slot remain live until all raw wakers have been retired.
        let waker = unsafe {
            Waker::from_raw(RawWaker::new(
                (&context as *const Reentrant).cast(),
                &VTABLE,
            ))
        };
        slot.register(&waker);
        slot.register(&Arc::new(Noop).into());
        assert_eq!(context.visits.load(Ordering::SeqCst), 2);
        drop(waker);
        slot.wake();
    }
}
