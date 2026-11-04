use crate::time::Duration;
use embedded_hal::watchdog::{Watchdog as WatchdogTrait, WatchdogDisable, WatchdogEnable};
use esp8266::WDT;

// based on the reverse engineering work done by mongoose-os
// https://mongoose-os.com/blog/esp8266-watchdog-timer/

const WATCHDOG_RESET_MAGIC: u32 = 0x73;

pub trait WatchdogExt {
    fn watchdog(self) -> Watchdog;
}

impl WatchdogExt for WDT {
    fn watchdog(self) -> Watchdog {
        Watchdog { watchdog: self }
    }
}

#[derive(Eq, PartialEq, Copy, Clone, Debug)]
#[repr(u8)]
pub enum StageTimeout {
    StageDisabled = 0,
    /// Roughly 0.84 seconds at 80Mhz
    Stage084Sec = 10,
    /// Roughly 1.68 seconds at 80Mhz
    Stage168Sec = 11,
    /// Roughly 3.36 seconds at 80Mhz
    Stage336Sec = 12,
    /// Roughly 6.71 seconds at 80Mhz
    Stage671Sec = 13,
    /// Roughly 13.4 seconds at 80Mhz
    Stage1342Sec = 14,
    /// Roughly 26.8 seconds at 80Mhz
    Stage2684Sec = 15,
}

impl From<StageTimeout> for Duration {
    fn from(timeout: StageTimeout) -> Duration {
        Duration::from_millis(match timeout {
            StageTimeout::StageDisabled => 0,
            StageTimeout::Stage084Sec => 840,
            StageTimeout::Stage168Sec => 1_680,
            StageTimeout::Stage336Sec => 3_360,
            StageTimeout::Stage671Sec => 6_710,
            StageTimeout::Stage1342Sec => 13_420,
            StageTimeout::Stage2684Sec => 26_840,
        })
    }
}

const STAGES: [StageTimeout; 6] = [
    StageTimeout::Stage084Sec,
    StageTimeout::Stage168Sec,
    StageTimeout::Stage336Sec,
    StageTimeout::Stage671Sec,
    StageTimeout::Stage1342Sec,
    StageTimeout::Stage2684Sec,
];

fn upper_stage(duration: Duration) -> StageTimeout {
    if duration.is_zero() {
        return StageTimeout::StageDisabled;
    }
    STAGES
        .into_iter()
        .find(|&stage| Duration::from(stage) >= duration)
        .unwrap_or(StageTimeout::Stage2684Sec)
}

fn stages_for_duration(duration: Duration) -> (StageTimeout, StageTimeout) {
    if duration.is_zero() {
        return (StageTimeout::StageDisabled, StageTimeout::StageDisabled);
    }
    let first = STAGES
        .into_iter()
        .rev()
        .find(|&stage| Duration::from(stage) <= duration)
        .unwrap_or(StageTimeout::Stage084Sec);
    (first, upper_stage(duration.saturating_sub(first.into())))
}

pub struct Watchdog {
    watchdog: WDT,
}

impl WatchdogTrait for Watchdog {
    fn feed(&mut self) {
        self.watchdog
            .wdt_rst()
            .write(|w| unsafe { w.bits(WATCHDOG_RESET_MAGIC) })
    }
}

impl WatchdogEnable for Watchdog {
    type Time = Duration;

    /// Start with a period rounded up to hardware stages, capped at 53.68s.
    /// Zero disables the watchdog. The peripheral clock is fixed at 80 MHz.
    fn start<T: Into<Duration>>(&mut self, period: T) {
        let (first, second) = stages_for_duration(period.into());
        self.start_stages(first, second);
    }
}

impl Watchdog {
    /// Configure the two hardware stages directly. A disabled first stage
    /// disables the watchdog; a disabled second stage uses only the first.
    pub fn start_stages(&mut self, first: StageTimeout, second: StageTimeout) {
        if first == StageTimeout::StageDisabled {
            self.disable();
            return;
        }
        self.watchdog.wdt_ctl().write(|w| {
            w.unknown_3()
                .set_bit()
                .unknown_4()
                .set_bit()
                .unknown_5()
                .set_bit()
                .stage_1_disable()
                .bit(second == StageTimeout::StageDisabled)
        });
        self.watchdog
            .wdt_op()
            .write(|w| unsafe { w.bits(first as u32) });
        if second != StageTimeout::StageDisabled {
            self.watchdog
                .wdt_op_nd()
                .write(|w| unsafe { w.bits(second as u32) });
        }
        self.feed();
        self.watchdog.wdt_ctl().modify(|_, w| w.enable().set_bit());
    }
}

impl WatchdogDisable for Watchdog {
    fn disable(&mut self) {
        self.watchdog
            .wdt_ctl()
            .modify(|_, w| w.enable().clear_bit());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duration_uses_bounded_stages_without_overflow() {
        for millis in [
            0, 1, 500, 840, 1_500, 1_680, 2_500, 3_360, 6_710, 13_420, 26_840, 27_000, 50_000,
            53_680, 100_000,
        ] {
            let duration = Duration::from_millis(millis);
            let (first, second) = stages_for_duration(duration);
            let actual = Duration::from(first) + Duration::from(second);
            assert!(actual >= duration.min(Duration::from_millis(53_680)));
        }
        assert_eq!(
            stages_for_duration(Duration::from_secs(u64::MAX)),
            (StageTimeout::Stage2684Sec, StageTimeout::Stage2684Sec)
        );
        assert_eq!(
            stages_for_duration(Duration::ZERO),
            (StageTimeout::StageDisabled, StageTimeout::StageDisabled)
        );
    }
}
