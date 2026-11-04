//! FreeRTOS compatibility layer — pure Rust replacements for the FreeRTOS
//! kernel symbols expected by the ESP8266 WiFi blob libraries.
//!
//! # Architecture
//!
//! The WiFi blobs (`libcore.a`, `libnet80211.a`, `libpp.a`, `libphy.a`) call
//! FreeRTOS functions through two layers:
//!
//! 1. **`__wifi_*` shim functions** (in [`wifi_os_adapter`]) — clean adapter
//!    matching the `esp_wifi_osi.h` interface.
//! 2. **Direct FreeRTOS symbols** — `vPortEnterCritical`, `pxCurrentTCB`,
//!    `vTaskSwitchContext`, and the SDK object APIs.
//!
//! This module provides both layers in Rust. The few architectural
//! save/restore instructions that Rust cannot express directly are kept as
//! local inline assembly beside the scheduler state they operate on.

#[cfg(target_arch = "xtensa")]
pub mod critical;
#[cfg(target_arch = "xtensa")]
pub mod entry;
#[cfg(target_arch = "xtensa")]
pub mod event_group;
#[cfg_attr(not(target_arch = "xtensa"), allow(dead_code, unused_imports))]
pub mod memory;
#[cfg(target_arch = "xtensa")]
pub mod port;
#[cfg(target_arch = "xtensa")]
pub mod queue;
#[cfg(target_arch = "xtensa")]
pub mod rx_bridge;
#[cfg(target_arch = "xtensa")]
pub mod semphr;
#[cfg(target_arch = "xtensa")]
pub mod task;
#[cfg(target_arch = "xtensa")]
pub mod timer;
#[cfg(target_arch = "xtensa")]
pub mod wifi_os_adapter;
