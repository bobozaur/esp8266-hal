//! Driver-owned hardware and runtime services used by the native wireless ABI.

#[cfg(target_arch = "xtensa")]
pub mod cache;
#[cfg(target_arch = "xtensa")]
pub mod nvs;
#[cfg(target_arch = "xtensa")]
pub mod phy_init;
#[cfg(target_arch = "xtensa")]
pub mod runtime;
#[cfg_attr(not(target_arch = "xtensa"), allow(dead_code))]
mod services;
#[cfg(target_arch = "xtensa")]
pub mod system;
#[cfg(target_arch = "xtensa")]
pub mod wifi_init;
