use core::cell::UnsafeCell;

pub(crate) struct Locked<T> {
    inner: UnsafeCell<T>,
}

impl<T> Locked<T> {
    pub const fn new(value: T) -> Self {
        Self {
            inner: UnsafeCell::new(value),
        }
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        critical_section::with(|_| unsafe { f(&mut *self.inner.get()) })
    }

    pub fn with_ref<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        critical_section::with(|_| unsafe { f(&*self.inner.get()) })
    }
}

// SAFETY: Access is serialized through `critical_section::with`.
unsafe impl<T: Send> Send for Locked<T> {}
// SAFETY: Access is serialized through `critical_section::with`.
unsafe impl<T: Send> Sync for Locked<T> {}
