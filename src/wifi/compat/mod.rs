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
//!    `vTaskSwitchContext`, `_xt_isr_handler`, etc.
//!
//! This module provides both layers entirely in Rust. The Xtensa assembly
//! files for context save/restore (`xtensa_vectors.S`, `xtensa_context.S`,
//! `os_cpu_a.S`) remain compiled from C and reference our `#[no_mangle]`
//! exports at link time.

pub mod critical;
pub mod entry;
pub mod event_group;
pub mod memory;
pub mod port;
pub mod pp_stage;
pub mod queue;
pub mod semphr;
pub mod task;
pub mod timer;
pub mod wifi_os_adapter;
