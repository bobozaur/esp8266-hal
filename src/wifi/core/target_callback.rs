use super::{locked::Locked, resources::Resources, InitError};

struct Registration {
    resources: *const Resources,
    active: bool,
    callbacks: usize,
}
// SAFETY: publication and callback admission are serialized by a critical section;
// Wifi retains its allocation through quiescence or leaks it on failed teardown.
unsafe impl Send for Registration {}
static REGISTRATION: Locked<Registration> = Locked::new(Registration {
    resources: core::ptr::null(),
    active: false,
    callbacks: 0,
});

pub(crate) struct CallbackRegistration;
impl CallbackRegistration {
    pub(crate) fn install(resources: &Resources) -> Result<Self, InitError> {
        REGISTRATION.with(|r| {
            if r.active || r.callbacks != 0 {
                return Err(InitError::AlreadyInitialized);
            }
            r.resources = resources;
            r.active = true;
            Ok(Self)
        })
    }
    pub(crate) fn quiescent() -> bool {
        REGISTRATION.with(|r| !r.active && r.callbacks == 0)
    }
}
impl Drop for CallbackRegistration {
    fn drop(&mut self) {
        REGISTRATION.with(|r| {
            r.active = false;
            r.resources = core::ptr::null();
        })
    }
}

/// Counts every native callback, including rejected RX packets that still invoke
/// the SDK's release routine after resource admission has closed.
pub(crate) struct CallbackInvocation {
    resources: *const Resources,
}
impl CallbackInvocation {
    pub(crate) fn enter() -> Self {
        REGISTRATION.with(|r| {
            r.callbacks += 1;
            Self {
                resources: if r.active {
                    r.resources
                } else {
                    core::ptr::null()
                },
            }
        })
    }
    pub(crate) fn resources(&self) -> Option<&Resources> {
        if self.resources.is_null() {
            None
        } else {
            // SAFETY: entry increments in-flight count before releasing the
            // registration lock. Wifi cannot free storage while it is nonzero.
            Some(unsafe { &*self.resources })
        }
    }
}
impl Drop for CallbackInvocation {
    fn drop(&mut self) {
        REGISTRATION.with(|r| r.callbacks -= 1)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{allocator::test_allocator, QueueConfig};
    use super::*;
    #[test]
    fn closed_admission_still_counts_native_release_callbacks() {
        let resources = Resources::try_new(&QueueConfig::default(), test_allocator()).unwrap();
        let registration = CallbackRegistration::install(&resources).unwrap();
        let admitted = CallbackInvocation::enter();
        assert!(admitted.resources().is_some());
        drop(registration);
        let rejected = CallbackInvocation::enter();
        assert!(rejected.resources().is_none());
        assert!(!CallbackRegistration::quiescent());
        drop(admitted);
        assert!(!CallbackRegistration::quiescent());
        assert!(matches!(
            CallbackRegistration::install(&resources),
            Err(InitError::AlreadyInitialized)
        ));
        drop(rejected);
        assert!(CallbackRegistration::quiescent());
    }
}
