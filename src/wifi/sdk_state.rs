//! Authoritative native initialization state, including failed cleanup.

use portable_atomic::{AtomicU8, Ordering};

const CLEAN: u8 = 0;
const NATIVE_LIVE: u8 = 1;
const READY: u8 = 2;

pub(crate) struct SdkState(AtomicU8);

impl SdkState {
    pub(crate) const fn new() -> Self {
        Self(AtomicU8::new(CLEAN))
    }

    /// Mark native ownership before entering a potentially partial initializer.
    pub(crate) fn begin(&self) {
        self.0.store(NATIVE_LIVE, Ordering::Release);
    }

    pub(crate) fn ready(&self) {
        self.0.store(READY, Ordering::Release);
    }

    pub(crate) fn cleanup(&self, complete: bool) {
        self.0.store(
            if complete { CLEAN } else { NATIVE_LIVE },
            Ordering::Release,
        );
    }

    pub(crate) fn is_initialized(&self) -> bool {
        self.0.load(Ordering::Acquire) == READY
    }

    pub(crate) fn requires_quarantine(&self) -> bool {
        self.0.load(Ordering::Acquire) != CLEAN
    }
}

#[cfg(test)]
mod tests {
    use super::SdkState;

    #[test]
    fn failed_partial_initialization_cleanup_retains_native_ownership() {
        let state = SdkState::new();
        assert!(!state.requires_quarantine());
        state.begin();
        // Native initialization acquired state; supplicant initialization failed.
        state.cleanup(false);
        assert!(state.requires_quarantine());
        assert!(!state.is_initialized());
        state.cleanup(true);
        assert!(!state.requires_quarantine());
    }

    #[test]
    fn ready_state_survives_only_until_teardown_begins() {
        let state = SdkState::new();
        state.begin();
        state.ready();
        assert!(state.is_initialized());
        state.begin();
        assert!(!state.is_initialized());
        assert!(state.requires_quarantine());
        state.cleanup(true);
        assert!(!state.requires_quarantine());
    }
}
