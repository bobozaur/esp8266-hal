//! Monotonic runtime time over the free-running, wrapping CPU cycle counter.

const TICK_US: u64 = 10_000;
/// Leave room for the register-write sequence; a post-write check also handles
/// an intervening NMI delaying programming beyond this margin.
pub(crate) const MIN_COMPARE_CYCLES: u32 = 256;

pub(crate) struct RuntimeClock {
    previous: u32,
    micros: u64,
    fractional_cycles: u32,
    ticks: u64,
}

pub(crate) struct Elapsed {
    pub(crate) ticks: u32,
    pub(crate) next_tick: u32,
}

impl RuntimeClock {
    pub(crate) const fn new(counter: u32) -> Self {
        Self {
            previous: counter,
            micros: 0,
            fractional_cycles: 0,
            ticks: 0,
        }
    }

    /// Samples occur at least once per counter period; compare IRQs normally
    /// arrive at 100Hz. Frequency changes settle the previous conversion first.
    fn observe(&mut self, counter: u32, cycles_per_us: u32) {
        let cycles = counter.wrapping_sub(self.previous) as u64 + self.fractional_cycles as u64;
        self.previous = counter;
        self.micros += cycles / cycles_per_us as u64;
        self.fractional_cycles = (cycles % cycles_per_us as u64) as u32;
    }

    pub(crate) fn advance(&mut self, counter: u32, cycles_per_us: u32) -> Elapsed {
        self.observe(counter, cycles_per_us);
        let ticks = self.micros / TICK_US;
        let elapsed = Elapsed {
            ticks: (ticks - self.ticks) as u32,
            next_tick: self.next_tick(counter, cycles_per_us),
        };
        self.ticks = ticks;
        elapsed
    }

    pub(crate) fn change_frequency(&mut self, counter: u32, old: u32, new: u32) -> u32 {
        self.observe(counter, old);
        self.fractional_cycles = (self.fractional_cycles as u64 * new as u64 / old as u64) as u32;
        // Any tick crossed while changing clocks is dispatched by the next IRQ.
        if self.micros / TICK_US > self.ticks {
            counter
        } else {
            self.next_tick(counter, new)
        }
    }

    fn next_tick(&self, counter: u32, cycles_per_us: u32) -> u32 {
        let remaining_us = TICK_US - self.micros % TICK_US;
        counter.wrapping_add(
            (remaining_us * cycles_per_us as u64 - self.fractional_cycles as u64) as u32,
        )
    }

    pub(crate) fn micros(&self, counter: u32, cycles_per_us: u32) -> u64 {
        self.micros
            + (counter.wrapping_sub(self.previous) as u64 + self.fractional_cycles as u64)
                / cycles_per_us as u64
    }
}

/// All runtime deadlines are within one tick, well inside the signed half range.
#[inline(always)]
pub(crate) fn compare_deadline(now: u32, requested: u32) -> u32 {
    if (requested.wrapping_sub(now) as i32) < MIN_COMPARE_CYCLES as i32 {
        now.wrapping_add(MIN_COMPARE_CYCLES)
    } else {
        requested
    }
}

#[cfg(test)]
mod tests {
    use super::{compare_deadline, RuntimeClock, MIN_COMPARE_CYCLES};

    #[test]
    fn short_alarm_interrupts_do_not_accelerate_sdk_ticks() {
        let mut clock = RuntimeClock::new(0);
        for index in 1..100 {
            let sample = index * 8_000;
            let elapsed = clock.advance(sample, 80);
            assert_eq!(elapsed.ticks, 0);
            assert_eq!(clock.micros(sample, 80), index as u64 * 100);
        }
        assert_eq!(clock.advance(800_000, 80).ticks, 1);
    }

    #[test]
    fn computation_time_fractional_cycles_and_wrap_are_accounted() {
        let origin = u32::MAX - 999;
        let mut clock = RuntimeClock::new(origin);
        let first = clock.advance(origin.wrapping_add(799_999), 80);
        assert_eq!(first.ticks, 0);
        // Work after a sample keeps advancing time and can pass the old deadline.
        let after_work = origin.wrapping_add(800_500);
        assert_eq!(clock.micros(after_work, 80), 10_006);
        assert_eq!(
            compare_deadline(after_work, first.next_tick),
            after_work.wrapping_add(MIN_COMPARE_CYCLES)
        );
        let next = clock.advance(after_work, 80);
        assert_eq!(next.ticks, 1);
        assert_eq!(next.next_tick, origin.wrapping_add(1_600_000));
        assert_eq!(clock.advance(origin.wrapping_add(2_920_000), 80).ticks, 2);
    }

    #[test]
    fn changing_cpu_frequency_preserves_the_previous_time_epoch() {
        let mut clock = RuntimeClock::new(0);
        assert_eq!(clock.micros(400_000, 80), 5_000);
        assert_eq!(clock.change_frequency(400_000, 80, 160), 1_200_000);
        assert_eq!(clock.micros(560_000, 160), 6_000);
        assert_eq!(clock.advance(1_200_000, 160).ticks, 1);
        assert_eq!(clock.micros(1_200_000, 160), 10_000);
    }

    #[test]
    fn near_past_and_wrapping_deadlines_remain_in_the_future() {
        for now in [0, 1_000, u32::MAX - 100] {
            for requested in [now.wrapping_sub(1), now, now.wrapping_add(1)] {
                assert_eq!(
                    compare_deadline(now, requested),
                    now.wrapping_add(MIN_COMPARE_CYCLES)
                );
            }
            assert_eq!(
                compare_deadline(now, now.wrapping_add(800_000)),
                now.wrapping_add(800_000)
            );
        }
    }
}

/// Alarm ownership is independent of IRQ programming. Taking a callback clears
/// the deadline before the callback can re-arm its slot.
#[cfg(any(feature = "embassy", test))]
pub(crate) struct Alarm {
    pub(crate) at: u64,
    pub(crate) callback: fn(*mut ()),
    pub(crate) ctx: *mut (),
}

#[cfg(any(feature = "embassy", test))]
impl Alarm {
    pub(crate) const fn new(callback: fn(*mut ())) -> Self {
        Self {
            at: u64::MAX,
            callback,
            ctx: core::ptr::null_mut(),
        }
    }

    pub(crate) fn set(&mut self, now: u64, timestamp: u64) -> bool {
        self.at = if timestamp > now { timestamp } else { u64::MAX };
        timestamp > now
    }

    pub(crate) fn take_due(&mut self, now: u64) -> Option<(fn(*mut ()), *mut ())> {
        if self.at > now || self.at == u64::MAX {
            return None;
        }
        self.at = u64::MAX;
        Some((self.callback, self.ctx))
    }
}

#[cfg(test)]
mod alarm_tests {
    use super::Alarm;

    fn callback(_: *mut ()) {}

    #[test]
    fn replacing_an_alarm_with_a_past_deadline_disarms_the_previous_deadline() {
        let mut alarm = Alarm::new(callback);
        assert!(alarm.set(10, 20));
        assert!(!alarm.set(10, 10));
        assert_eq!(alarm.at, u64::MAX);
        assert!(alarm.take_due(20).is_none());
    }

    #[test]
    fn dispatching_one_alarm_preserves_the_next_and_allows_rearming() {
        let mut alarms = [Alarm::new(callback), Alarm::new(callback)];
        assert!(alarms[0].set(0, 1_000));
        assert!(alarms[1].set(0, 2_000));
        assert!(alarms[0].take_due(1_000).is_some());
        assert!(alarms[1].take_due(1_000).is_none());
        assert_eq!(alarms.iter().map(|alarm| alarm.at).min(), Some(2_000));
        assert!(alarms[0].set(1_000, 1_500));
        assert_eq!(alarms.iter().map(|alarm| alarm.at).min(), Some(1_500));
    }
}
