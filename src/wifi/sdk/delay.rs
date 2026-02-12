//! FRC2-based `ets_delay_us` replacement with init-mode bypass.
//!
//! During WiFi init (register_chipv6_phy), the blob depends on ets_delay_us
//! returning instantly (broken CCOUNT-based behavior). After init, scan/auth
//! need real delays for proper dwell timing.
//!
//! The wrapper uses a flag to switch between modes:
//! - Init mode (default): call the real ets_delay_us (broken, instant return)
//! - Runtime mode: use FRC2-based accurate delay

use portable_atomic::{AtomicBool, Ordering};

const FRC2_COUNT: *const u32 = 0x60000624 as *const u32;
const FRC2_TICKS_PER_US: u32 = 5;

/// When true, use FRC2-based delay. When false, call the real (broken) one.
static USE_FRC2_DELAY: AtomicBool = AtomicBool::new(false);

/// Enable FRC2-based delays. Call after `esp_wifi_init` completes.
pub fn enable_frc2_delay() {
    USE_FRC2_DELAY.store(true, Ordering::Release);
}

extern "C" {
    fn __real_ets_delay_us(us: u32);
}

/// Wrapped `ets_delay_us` — routes to FRC2 or real implementation.
#[no_mangle]
pub unsafe extern "C" fn __wrap_ets_delay_us(us: u32) {
    if USE_FRC2_DELAY.load(Ordering::Relaxed) {
        // Runtime mode: FRC2-based accurate delay
        unsafe {
            let start = FRC2_COUNT.read_volatile();
            let target = us.saturating_mul(FRC2_TICKS_PER_US);
            while FRC2_COUNT.read_volatile().wrapping_sub(start) < target {}
        }
    } else {
        // Init mode: pass through to real (CCOUNT-based, broken) delay
        unsafe { __real_ets_delay_us(us) };
    }
}
