//! Links the checked-in ESP8266 wireless runtime.
//!
//! Bindings and native archives are deliberately generated out of band and
//! vendored with the crate. Normal firmware builds therefore do not compile C
//! sources or need an ESP8266 RTOS SDK checkout, bindgen, libclang, or network
//! access. This mirrors the packaging model used by esp-rs' chip-specific
//! `esp-wifi-sys-*` crates.

use std::{env, path::PathBuf};

const BASE_LIBRARIES: &[&str] = &[
    // Rust-facing SDK support and the irreducible native ABI/runtime pieces.
    "esp8266_sdk",
    "wpa_supplicant",
    "mbedtls_all",
    // Espressif's closed wireless/PHY libraries.
    "core",
    "net80211",
    "phy",
    "pp",
    "hal",
    "clk",
    "rtc",
];

// The exact newlib runtime shipped with the pinned Xtensa toolchain. Keep it
// after the optional SDK components, matching the order used to derive and
// validate the snapshot.
const C_RUNTIME_LIBRARIES: &[&str] = &["c", "m"];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/bindings.rs");
    println!("cargo:rerun-if-changed=vendor");

    // The bindings remain useful to host-side tooling, but incompatible
    // Xtensa archives must not be offered to a host linker.
    if env::var("TARGET").as_deref() != Ok("xtensa-esp8266-none-elf") {
        return;
    }

    let crate_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let libraries = crate_dir.join("vendor/libs");
    let linker_scripts = crate_dir.join("vendor/ld");

    println!("cargo:rustc-link-search=native={}", libraries.display());
    for library in BASE_LIBRARIES {
        println!("cargo:rustc-link-lib=static={library}");
    }
    if env::var_os("CARGO_FEATURE_ESP_NOW").is_some() {
        println!("cargo:rustc-link-lib=static=espnow");
    }
    if env::var_os("CARGO_FEATURE_SMARTCONFIG").is_some() {
        println!("cargo:rustc-link-lib=static=smartconfig");
    }
    for library in C_RUNTIME_LIBRARIES {
        println!("cargo:rustc-link-lib=static={library}");
    }
    // A dependency cannot propagate final-link arguments from its own build
    // script. Export the directory through `links` metadata so the HAL's
    // build script can apply these scripts to the firmware link itself.
    println!("cargo:ld-scripts={}", linker_scripts.display());
}
