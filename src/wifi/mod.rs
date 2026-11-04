//! ESP8266 Wi-Fi with explicit application-owned runtime and allocation.
//!
//! The owning Wi-Fi object keeps native callback memory alive. Its controller
//! handles configuration and connection operations; independent station/AP
//! interfaces carry frames to the application's selected network stack.
//! Initialization, radio activation and reconnection are application choices.

pub use crate::wifi_core::*;

#[cfg(any(target_arch = "xtensa", test))]
pub(crate) mod compat;
#[cfg(target_arch = "xtensa")]
pub(crate) mod error;
#[cfg(target_arch = "xtensa")]
pub mod runtime;
#[cfg(target_arch = "xtensa")]
pub(crate) mod sdk;
#[cfg(any(test, target_arch = "xtensa"))]
pub(crate) mod tx_buffer;
#[cfg(target_arch = "xtensa")]
pub(crate) use error::EspError;
#[cfg(target_arch = "xtensa")]
pub use runtime::{Runtime, RuntimeError};

#[cfg(all(feature = "embassy", target_arch = "xtensa"))]
pub mod executor;
#[cfg(any(test, target_arch = "xtensa"))]
pub(crate) mod runtime_clock;
#[cfg(any(test, target_arch = "xtensa"))]
pub(crate) mod sdk_state;
#[cfg(all(feature = "embassy", target_arch = "xtensa"))]
pub(crate) mod timer_driver;

#[cfg(all(feature = "wifi-smoltcp", any(target_arch = "xtensa", test)))]
pub(crate) mod device;
#[cfg(all(feature = "wifi-smoltcp", any(target_arch = "xtensa", test)))]
/// Synchronous network-stack adapter.
pub mod smoltcp {
    pub use super::device::WifiDevice;
}
#[cfg(all(feature = "wifi-embassy-net", any(target_arch = "xtensa", test)))]
pub mod embassy_net;
#[cfg(all(
    any(feature = "wifi-smoltcp", feature = "wifi-embassy-net"),
    any(target_arch = "xtensa", test)
))]
pub(crate) mod neighbor;

/// Yield the current runtime task for scheduler ticks (10 ms each).
///
/// Initialize [`Runtime`] before using runtime delays. Async applications can
/// use their executor's timer facilities instead.
#[cfg(target_arch = "xtensa")]
pub fn delay_ticks(ticks: u32) {
    unsafe { compat::task::vTaskDelay(ticks) };
}
