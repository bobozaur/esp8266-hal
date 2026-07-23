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
        critical_section::with(|_| {
            let slot = unsafe { &mut *self.slot.get() };
            match slot {
                Some(existing) if existing.will_wake(waker) => {}
                _ => *slot = Some(waker.clone()),
            }
        });
    }

    #[cfg(any(target_arch = "xtensa", test))]
    pub fn wake(&self) {
        let waker = critical_section::with(|_| unsafe { (*self.slot.get()).take() });
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    #[cfg(all(feature = "wifi", target_arch = "xtensa"))]
    pub fn clear(&self) {
        let waker = critical_section::with(|_| unsafe { (*self.slot.get()).take() });
        drop(waker);
    }
}
