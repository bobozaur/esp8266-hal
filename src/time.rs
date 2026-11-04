//! Standard durations and integer conveniences.
//!
//! Timers and the watchdog accept [`Duration`] directly. The integer helpers
//! avoid overflowing intermediate unit conversions, including multi-second waits.

pub use core::time::Duration;

pub trait U32Ext {
    fn s(self) -> Duration;
    fn ms(self) -> Duration;
    fn us(self) -> Duration;
    fn ns(self) -> Duration;
}

impl U32Ext for u32 {
    fn s(self) -> Duration {
        Duration::from_secs(u64::from(self))
    }
    fn ms(self) -> Duration {
        Duration::from_millis(u64::from(self))
    }
    fn us(self) -> Duration {
        Duration::from_micros(u64::from(self))
    }
    fn ns(self) -> Duration {
        Duration::from_nanos(u64::from(self))
    }
}
