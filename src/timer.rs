//! ESP8266 timers, clocked from the fixed 80 MHz peripheral bus.

use crate::time::Duration;
use embedded_hal::blocking::delay::{DelayMs, DelayUs};
use embedded_hal::timer::{Cancel, CountDown, Periodic};
use esp8266::{DPORT, TIMER};
use void::Void;

// The /256 prescaler gives an exact 3.2 microsecond tick at 80 MHz.
const TICK_NANOS: u64 = 3_200;
const TIMER1_MAX: u32 = 0x007f_ffff;

pub trait TimerExt {
    /// Split the peripheral into independently owned timers.
    ///
    /// Wi-Fi runtime initialization consumes Timer2 for the rest of the boot.
    fn timers(self) -> (Timer1, Timer2);
}

impl TimerExt for TIMER {
    fn timers(self) -> (Timer1, Timer2) {
        (Timer1::new(), Timer2::new())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DurationTooLong;

fn duration_ticks(duration: Duration, max: u32) -> Result<u32, DurationTooLong> {
    let nanos = duration
        .as_secs()
        .checked_mul(1_000_000_000)
        .and_then(|whole| whole.checked_add(u64::from(duration.subsec_nanos())))
        .ok_or(DurationTooLong)?;
    let ticks = (nanos / TICK_NANOS + u64::from(nanos % TICK_NANOS != 0)).max(1);
    if ticks > u64::from(max) {
        Err(DurationTooLong)
    } else {
        Ok(ticks as u32)
    }
}

macro_rules! impl_timer {
    ($name:ident, $ctrl:ident, $load:ident, $alarm:ident, $int:ident, $clear:ident, $max:expr, $load_value:expr) => {
        pub struct $name {
            _private: (),
        }

        impl $name {
            fn new() -> Self {
                // SAFETY: TimerExt consumes the unique TIMER token and creates
                // exactly one owner of each disjoint timer register set.
                let timer = unsafe { &*TIMER::ptr() };
                timer.$ctrl().write(|w| {
                    w.rollover()
                        .set_bit()
                        .interrupt_type()
                        .level()
                        .prescale_divider()
                        .devided_by_256()
                        .timer_enable()
                        .clear_bit()
                });
                timer.$alarm().write(|w| unsafe { w.bits(0) });
                timer.$int().write(|w| w.$clear().set_bit());
                Self { _private: () }
            }

            /// Start a periodic countdown, rounding up to 3.2 microsecond ticks.
            /// Zero requests one tick. Oversized durations leave the timer alone.
            pub fn try_start(&mut self, duration: Duration) -> Result<(), DurationTooLong> {
                let ticks = duration_ticks(duration, $max)?;
                // SAFETY: this value owns this timer's registers exclusively.
                let timer = unsafe { &*TIMER::ptr() };
                timer.$ctrl().modify(|_, w| w.timer_enable().clear_bit());
                timer
                    .$load()
                    .write(|w| unsafe { w.bits(($load_value)(ticks)) });
                timer.$int().write(|w| w.$clear().set_bit());
                timer.$ctrl().modify(|_, w| w.timer_enable().set_bit());
                Ok(())
            }

            /// Block for a duration, splitting waits beyond the hardware range.
            pub fn delay(&mut self, mut duration: Duration) {
                let maximum = Duration::from_nanos(u64::from($max) * TICK_NANOS);
                while !duration.is_zero() {
                    let chunk = duration.min(maximum);
                    self.start(chunk);
                    nb::block!(self.wait()).unwrap();
                    duration -= chunk;
                }
                let _ = self.cancel();
            }
        }

        impl CountDown for $name {
            type Time = Duration;
            /// Panics if the duration exceeds this timer's range; use try_start
            /// for fallible input or delay for a long blocking wait.
            fn start<T: Into<Duration>>(&mut self, timeout: T) {
                self.try_start(timeout.into())
                    .expect("duration exceeds timer range");
            }
            fn wait(&mut self) -> nb::Result<(), Void> {
                // SAFETY: this value owns this timer's registers exclusively.
                let timer = unsafe { &*TIMER::ptr() };
                if timer.$ctrl().read().$int().bit_is_clear() {
                    Err(nb::Error::WouldBlock)
                } else {
                    timer.$int().write(|w| w.$clear().set_bit());
                    Ok(())
                }
            }
        }
        impl Periodic for $name {}
        impl Cancel for $name {
            type Error = Void;
            fn cancel(&mut self) -> Result<(), Void> {
                // SAFETY: this value owns this timer's registers exclusively.
                let timer = unsafe { &*TIMER::ptr() };
                timer.$ctrl().modify(|_, w| w.timer_enable().clear_bit());
                Ok(())
            }
        }
        impl DelayUs<u32> for $name {
            fn delay_us(&mut self, us: u32) {
                self.delay(Duration::from_micros(u64::from(us)));
            }
        }
        impl DelayMs<u32> for $name {
            fn delay_ms(&mut self, ms: u32) {
                self.delay(Duration::from_millis(u64::from(ms)));
            }
        }
    };
}

impl_timer!(
    Timer1,
    frc1_ctrl,
    frc1_load,
    frc1_load,
    frc1_int,
    frc1_int_clr_mask,
    TIMER1_MAX,
    |ticks| ticks
);
impl_timer!(
    Timer2,
    frc2_ctrl,
    frc2_load,
    frc2_alarm,
    frc2_int,
    frc2_int_clr_mask,
    u32::MAX,
    |ticks: u32| 0u32.wrapping_sub(ticks)
);

impl Timer1 {
    /// Enable edge interrupts. The handler must acknowledge the timer; polling
    /// wait concurrently with an interrupt handler consumes the same event.
    pub fn enable_interrupts(&mut self) {
        critical_section::with(|_| {
            let timer = unsafe { &*TIMER::ptr() };
            let dport = unsafe { &*DPORT::ptr() };
            timer.frc1_ctrl().modify(|_, w| w.interrupt_type().edge());
            dport
                .edge_int_enable()
                .modify(|_, w| w.timer1_edge_int_enable().set_bit());
        });
    }

    pub fn disable_interrupts(&mut self) {
        critical_section::with(|_| {
            let timer = unsafe { &*TIMER::ptr() };
            let dport = unsafe { &*DPORT::ptr() };
            timer.frc1_ctrl().modify(|_, w| w.interrupt_type().level());
            dport
                .edge_int_enable()
                .modify(|_, w| w.timer1_edge_int_enable().clear_bit());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tick_rounding_and_timer_ranges() {
        assert_eq!(duration_ticks(Duration::ZERO, TIMER1_MAX), Ok(1));
        assert_eq!(
            duration_ticks(Duration::from_nanos(3_200), TIMER1_MAX),
            Ok(1)
        );
        assert_eq!(
            duration_ticks(Duration::from_nanos(3_201), TIMER1_MAX),
            Ok(2)
        );
        assert_eq!(
            duration_ticks(Duration::from_secs(5), TIMER1_MAX),
            Ok(1_562_500)
        );
        let limit = Duration::from_nanos(u64::from(TIMER1_MAX) * TICK_NANOS);
        assert_eq!(duration_ticks(limit, TIMER1_MAX), Ok(TIMER1_MAX));
        assert_eq!(
            duration_ticks(limit + Duration::from_nanos(1), TIMER1_MAX),
            Err(DurationTooLong)
        );
        assert_eq!(
            duration_ticks(Duration::from_secs(u64::MAX), u32::MAX),
            Err(DurationTooLong)
        );
        assert_eq!(
            duration_ticks(Duration::from_secs(60), u32::MAX),
            Ok(18_750_000)
        );
    }
}
