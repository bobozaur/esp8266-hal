//! Rust replacements for ESP8266 RTOS SDK C override files.
//!
//! These modules provide `#[no_mangle] extern "C"` functions that the WiFi
//! blob libraries expect to find at link time. They replace the C files
//! formerly in `esp8266-wifi-sys/sdk_overrides/`.
//!
//! # What was replaced
//!
//! | C file | Rust module | Purpose |
//! |--------|-------------|---------|
//! | `spi_flash_raw.c` | [`cache`] | IRAM cache enable/disable |
//! | `system_api.c` | [`system`] | MAC address, chip info, CPU freq |
//! | `phy_init.c` | [`phy_init`] | PHY/RF calibration, clock init |
//! | `esp_wifi.c` | [`wifi_init`] | WiFi init, TX path |

pub mod cache; // replaces spi_flash_raw.c — IRAM cache control
pub mod nvs; // fixed-size volatile replacement for the IDF NVS component
pub mod phy_init; // replaces phy_init.c
pub mod runtime; // remaining SDK runtime ABI adapters
pub mod system; // replaces system_api.c
pub mod wifi_init; // replaces esp_wifi.c
