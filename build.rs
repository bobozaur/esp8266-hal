use std::env;
use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;

const IROM_FLASH_OFFSET_ENV: &str = "ESP8266_HAL_IROM_FLASH_OFFSET";
const EAP_CA_CERT_PEM_NUL_FILE_ENV: &str = "ESP8266_HAL_EAP_CA_CERT_PEM_NUL_FILE";
const EAP_CLIENT_CERT_PEM_NUL_FILE_ENV: &str = "ESP8266_HAL_EAP_CLIENT_CERT_PEM_NUL_FILE";
const EAP_PRIVATE_KEY_PEM_NUL_FILE_ENV: &str = "ESP8266_HAL_EAP_PRIVATE_KEY_PEM_NUL_FILE";
const EAP_PRIVATE_KEY_PASSWORD_NUL_FILE_ENV: &str = "ESP8266_HAL_EAP_PRIVATE_KEY_PASSWORD_NUL_FILE";
const EAP_CA_CERT_DER_FILE_ENV: &str = "ESP8266_HAL_EAP_CA_CERT_DER_FILE";
const EAP_CLIENT_CERT_DER_FILE_ENV: &str = "ESP8266_HAL_EAP_CLIENT_CERT_DER_FILE";
const EAP_PRIVATE_KEY_DER_FILE_ENV: &str = "ESP8266_HAL_EAP_PRIVATE_KEY_DER_FILE";
const DEFAULT_IROM_ORIGIN: u32 = 0x4021_0000;
const IROM_FLASH_BASE: u32 = 0x4020_0000;

fn main() {
    println!("cargo:rustc-check-cfg=cfg(esp8266_hal_eap_ca_cert_pem_nul_file)");
    println!("cargo:rustc-check-cfg=cfg(esp8266_hal_eap_client_cert_pem_nul_file)");
    println!("cargo:rustc-check-cfg=cfg(esp8266_hal_eap_private_key_pem_nul_file)");
    println!("cargo:rustc-check-cfg=cfg(esp8266_hal_eap_private_key_password_nul_file)");
    println!("cargo:rustc-check-cfg=cfg(esp8266_hal_eap_ca_cert_der_file)");
    println!("cargo:rustc-check-cfg=cfg(esp8266_hal_eap_client_cert_der_file)");
    println!("cargo:rustc-check-cfg=cfg(esp8266_hal_eap_private_key_der_file)");

    let wifi_target = env::var_os("CARGO_FEATURE_WIFI").is_some()
        && env::var("TARGET").as_deref() == Ok("xtensa-esp8266-none-elf");

    // Put the linker script somewhere the linker can find it
    let out = &PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let memory_x = memory_x();
    File::create(out.join("memory.x"))
        .unwrap()
        .write_all(memory_x.as_bytes())
        .unwrap();

    let alias_x = alias_x();
    File::create(out.join("alias.x"))
        .unwrap()
        .write_all(alias_x.as_bytes())
        .unwrap();

    // Override xtensa-lx-rt's link.x with our own version that collects
    // .rodata2.* / .text2.* / .literal2.* orphan sections from the SDK WiFi
    // blob libraries. Without this, the linker creates misaligned segments
    // that crash the ESP8266 ROM bootloader.
    let wifi_link_inputs = if wifi_target {
        "INCLUDE esp8266.rom.ld\nINCLUDE esp8266.peripherals.ld\n\
         EXTERN(ets_printf ets_delay_us ieee80211_deliver_data)"
    } else {
        ""
    };
    let link_x = include_str!("ld/link.x").replace("/* WIFI_LINK_INPUTS */", wifi_link_inputs);
    File::create(out.join("link.x"))
        .unwrap()
        .write_all(link_x.as_bytes())
        .unwrap();

    println!("cargo:rustc-link-search={}", out.display());

    if env::var_os("CARGO_FEATURE_WIFI_ENTERPRISE").is_some() {
        if env::var_os(EAP_CA_CERT_PEM_NUL_FILE_ENV).is_some() {
            println!("cargo:rustc-cfg=esp8266_hal_eap_ca_cert_pem_nul_file");
        }
        if env::var_os(EAP_CLIENT_CERT_PEM_NUL_FILE_ENV).is_some() {
            println!("cargo:rustc-cfg=esp8266_hal_eap_client_cert_pem_nul_file");
        }
        if env::var_os(EAP_PRIVATE_KEY_PEM_NUL_FILE_ENV).is_some() {
            println!("cargo:rustc-cfg=esp8266_hal_eap_private_key_pem_nul_file");
        }
        if env::var_os(EAP_PRIVATE_KEY_PASSWORD_NUL_FILE_ENV).is_some() {
            println!("cargo:rustc-cfg=esp8266_hal_eap_private_key_password_nul_file");
        }
        if env::var_os(EAP_CA_CERT_DER_FILE_ENV).is_some() {
            println!("cargo:rustc-cfg=esp8266_hal_eap_ca_cert_der_file");
        }
        if env::var_os(EAP_CLIENT_CERT_DER_FILE_ENV).is_some() {
            println!("cargo:rustc-cfg=esp8266_hal_eap_client_cert_der_file");
        }
        if env::var_os(EAP_PRIVATE_KEY_DER_FILE_ENV).is_some() {
            println!("cargo:rustc-cfg=esp8266_hal_eap_private_key_der_file");
        }
    }

    if wifi_target {
        let linker_scripts = PathBuf::from(
            env::var_os("DEP_ESP8266_WIFI_LD_SCRIPTS")
                .expect("esp8266-wifi-sys did not export its vendored linker scripts"),
        );
        for script in ["esp8266.rom.ld", "esp8266.peripherals.ld"] {
            fs::copy(linker_scripts.join(script), out.join(script))
                .unwrap_or_else(|error| panic!("failed to stage {script}: {error}"));
        }
    }

    // Re-run when linker scripts change.
    println!("cargo:rerun-if-changed=ld/memory.x");
    println!("cargo:rerun-if-changed=ld/link.x");
    println!("cargo:rerun-if-changed=ld/rom.x");
    println!("cargo:rerun-if-changed=ld/ram.x");
    println!("cargo:rerun-if-env-changed={IROM_FLASH_OFFSET_ENV}");
    println!("cargo:rerun-if-env-changed={EAP_CA_CERT_PEM_NUL_FILE_ENV}");
    println!("cargo:rerun-if-env-changed={EAP_CLIENT_CERT_PEM_NUL_FILE_ENV}");
    println!("cargo:rerun-if-env-changed={EAP_PRIVATE_KEY_PEM_NUL_FILE_ENV}");
    println!("cargo:rerun-if-env-changed={EAP_PRIVATE_KEY_PASSWORD_NUL_FILE_ENV}");
    println!("cargo:rerun-if-env-changed={EAP_CA_CERT_DER_FILE_ENV}");
    println!("cargo:rerun-if-env-changed={EAP_CLIENT_CERT_DER_FILE_ENV}");
    println!("cargo:rerun-if-env-changed={EAP_PRIVATE_KEY_DER_FILE_ENV}");
}

fn alias_x() -> &'static str {
    if cfg!(feature = "all_in_ram") {
        include_str!("ld/ram.x")
    } else {
        include_str!("ld/rom.x")
    }
}

fn memory_x() -> String {
    let mut memory = include_str!("ld/memory.x").to_owned();
    let Some(offset) = irom_flash_offset() else {
        return memory;
    };
    let origin = IROM_FLASH_BASE
        .checked_add(offset)
        .expect("ESP8266_HAL_IROM_FLASH_OFFSET overflows IROM address");
    memory = memory.replace(
        &format!("ORIGIN = 0x{DEFAULT_IROM_ORIGIN:08x}"),
        &format!("ORIGIN = 0x{origin:08x}"),
    );
    memory
}

fn irom_flash_offset() -> Option<u32> {
    let raw = env::var(IROM_FLASH_OFFSET_ENV).ok()?;
    let offset = parse_u32_literal(&raw).unwrap_or_else(|| {
        panic!("{IROM_FLASH_OFFSET_ENV} must be a decimal or 0x-prefixed number")
    });
    if offset < 0x10000 || offset & 0xffff != 0 {
        panic!("{IROM_FLASH_OFFSET_ENV} must be 64 KiB aligned and at least 0x10000");
    }
    Some(offset)
}

fn parse_u32_literal(raw: &str) -> Option<u32> {
    let value = raw.trim();
    if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        u32::from_str_radix(hex, 16).ok()
    } else {
        value.parse().ok()
    }
}
