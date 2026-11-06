//! Links the pinned ESP8266 native wireless inputs.
//!
//! ABI declarations and native archives are checked in. Firmware builds do not compile C
//! sources or need an ESP8266 RTOS SDK checkout, bindgen, libclang, or network
//! access. The driver supplies the surrounding runtime services in Rust.

use std::{env, path::PathBuf};

const BASE_LIBRARIES: &[&str] = &[
    // Retained open C protocol/crypto snapshots; see vendor/README.md.
    "wpa_supplicant",
    "mbedtls_all",
    // Unchanged SDK radio/PHY and RTC/RF binaries.
    "core",
    "net80211",
    "phy",
    "pp",
    "clk",
];

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
    // The pinned newlib snapshot supplies real native formatting/string
    // operations; its frexp implementation also satisfies formatting's math
    // dependency. The Rust driver supplies allocation and descriptor syscalls.
    println!("cargo:rustc-link-lib=static=c");
    // A dependency cannot propagate final-link arguments from its own build
    // script. Export the directory through `links` metadata so the HAL's
    // build script can apply these scripts to the firmware link itself.
    println!("cargo:ld-scripts={}", linker_scripts.display());
}
