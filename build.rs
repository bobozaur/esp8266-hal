use std::{env, fs, path::PathBuf};

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let wifi_target = env::var_os("CARGO_FEATURE_WIFI").is_some()
        && env::var("TARGET").as_deref() == Ok("xtensa-esp8266-none-elf");

    fs::write(out.join("memory.x"), include_bytes!("ld/memory.x")).unwrap();
    let alias = if cfg!(feature = "all_in_ram") {
        include_str!("ld/ram.x")
    } else {
        include_str!("ld/rom.x")
    };
    fs::write(out.join("alias.x"), alias).unwrap();

    // Collect the SDK's additional sections and select its native symbols and
    // NMI vector only for Wi-Fi firmware. Dependency Cargo configuration does
    // not propagate, so the final application still selects -Tlink.x.
    let wifi_inputs = if wifi_target {
        include_str!("ld/wifi.x")
    } else {
        ""
    };
    let mut link = include_str!("ld/link.x").replace("/* WIFI_LINK_INPUTS */", wifi_inputs);
    if wifi_target {
        fs::write(
            out.join("wifi-exception.x"),
            include_bytes!("ld/wifi-exception.x"),
        )
        .unwrap();
        link = link.replace("INCLUDE exception.x", "INCLUDE wifi-exception.x");
        let sdk_scripts = PathBuf::from(
            env::var_os("DEP_ESP8266_WIFI_LD_SCRIPTS")
                .expect("esp8266-wifi-sys did not export its linker scripts"),
        );
        for script in ["esp8266.rom.ld", "esp8266.peripherals.ld"] {
            fs::copy(sdk_scripts.join(script), out.join(script)).unwrap();
        }
    }
    fs::write(out.join("link.x"), link).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=ld");
}
