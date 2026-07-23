//! WiFi driver for the ESP8266, built on the RTOS SDK.
//!
//! The public API is a typed, caller-owned Rust driver. The target backend maps
//! that API directly to the pinned ESP8266 SDK ABI; there is no second legacy
//! controller, configuration model, or event bus underneath it.
//!
//! The recommended networking path is:
//!
//! 1. build a [`WifiDriver`]
//! 2. connect with a temporary station handle from `driver.sta()`
//! 3. hand ownership to networking with `driver.sta_link()`
//!
//! # Architecture (Embassy-first)
//!
//! The embassy executor owns the main thread. The FreeRTOS compat scheduler
//! runs in the background (via tick ISR + PendSV) to service WiFi blob tasks.
//!
//! RX packet ingress, link-state tracking, wake registration, and packet
//! storage all live in caller-owned [`WifiResources`].
//!
//! # Typestate quick start
//!
//! ```rust,no_run
//! use esp8266_hal::wifi::{
//!     ClientConfig, InitConfig, Ssid, StationCredential, WifiDriver, WifiResources,
//! };
//!
//! let resources = WifiResources::<8, 4>::new();
//! let driver = WifiDriver::new(&resources)
//!     .initialize(InitConfig::default())
//!     .expect("init");
//!
//! let config = ClientConfig::new(
//!     Ssid::try_from("MyNetwork").expect("ssid"),
//!     StationCredential::open(),
//! );
//!
//! let mut driver = driver.configure_sta(config).expect("config").start().expect("start");
//! let mut station = driver.sta();
//! station.connect_blocking(15_000).expect("connect");
//! ```
//!
// FFI compat layer: all `extern "C"` functions called from C/blobs.
pub(crate) mod ap_netif;
#[allow(clippy::missing_safety_doc, clippy::needless_range_loop)]
#[doc(hidden)]
pub(crate) mod compat;
pub(crate) mod timer_driver;

pub(crate) mod device;
pub(crate) mod error;

#[doc(hidden)]
pub mod executor;
pub(crate) mod sdk;

#[cfg(feature = "wifi-embassy-net")]
pub use crate::wifi_core::embassy_net;

/// `smoltcp` adapter types over owned [`wifi_core`](crate::wifi_core) links.
pub mod smoltcp {
    pub use super::device::{ApWifiDevice, LinkDevice, WifiDevice};
}

pub use crate::wifi_core::*;
pub(crate) use error::EspError;
#[doc(hidden)]
pub use static_cell::StaticCell as __StaticCell;

// These hidden entry hooks keep macro expansions stable without exposing the
// FreeRTOS-compat implementation modules as public API.
#[doc(hidden)]
pub fn __init_runtime_heap() {
    compat::memory::init();
}

#[doc(hidden)]
pub fn __init_newlib_reent() {
    compat::entry::init_newlib_reent();
}

#[doc(hidden)]
pub fn __reclaim_boot_stack_reservation() {
    compat::memory::reclaim_boot_stack_reservation();
}

#[doc(hidden)]
pub unsafe fn __run_sdk_main_task(entry: unsafe extern "C" fn(*mut core::ffi::c_void)) -> ! {
    unsafe { compat::entry::run_sdk_main_task(entry) }
}

/// Yield the current compat task for `ticks` scheduler ticks.
///
/// This is primarily useful in synchronous examples after their terminal
/// marker has been emitted. Async code should prefer `embassy_time::Timer`.
pub fn delay_ticks(ticks: u32) {
    unsafe { compat::task::vTaskDelay(ticks) };
}
