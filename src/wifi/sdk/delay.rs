//! FRC2-based `ets_delay_us` replacement with init-mode bypass.
//!
//! During WiFi init (register_chipv6_phy), the blob depends on ets_delay_us
//! returning instantly (broken CCOUNT-based behavior). After init, scan/auth
//! need real delays for proper dwell timing.
//!
//! The wrapper uses a flag to switch between modes:
//! - Init mode (default): preserve the SDK CCOUNT delay
//! - Runtime mode: use FRC2-based accurate delay

use portable_atomic::{AtomicBool, Ordering};

const FRC2_COUNT: *const u32 = 0x60000624 as *const u32;
const FRC2_TICKS_PER_US: u32 = 5;

/// When true, use FRC2-based delay. When false, preserve the SDK init behavior.
static USE_FRC2_DELAY: AtomicBool = AtomicBool::new(false);

/// Enable FRC2-based delays. Call after `esp_wifi_init` completes.
pub fn enable_frc2_delay() {
    USE_FRC2_DELAY.store(true, Ordering::Release);
}

/// SDK-compatible `ets_delay_us` with accurate runtime delays.
#[no_mangle]
pub unsafe extern "C" fn ets_delay_us(us: u32) {
    if USE_FRC2_DELAY.load(Ordering::Relaxed) {
        // Runtime mode: FRC2-based accurate delay
        unsafe {
            let start = FRC2_COUNT.read_volatile();
            let target = us.saturating_mul(FRC2_TICKS_PER_US);
            while FRC2_COUNT.read_volatile().wrapping_sub(start) < target {}
        }
    } else {
        extern "C" {
            static g_esp_ticks_per_us: u32;
        }
        let ticks_per_us = unsafe { g_esp_ticks_per_us };
        let target = (us & u16::MAX as u32).wrapping_mul(ticks_per_us);
        let start = xtensa_lx::timer::get_cycle_count();
        while xtensa_lx::timer::get_cycle_count().wrapping_sub(start) < target {}
    }
}
