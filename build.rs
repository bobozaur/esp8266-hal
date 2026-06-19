use std::env;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

const IROM_FLASH_OFFSET_ENV: &str = "ESP8266_HAL_IROM_FLASH_OFFSET";
const SMARTCONFIG_EVENT_TRACE_ENV: &str = "ESP8266_HAL_SMARTCONFIG_EVENT_TRACE";
const SMARTCONFIG_SNIFFER_TRACE_ENV: &str = "ESP8266_HAL_SMARTCONFIG_SNIFFER_TRACE";
const SMARTCONFIG_WIFI_TRACE_ENV: &str = "ESP8266_HAL_SMARTCONFIG_WIFI_TRACE";
const SMARTCONFIG_RX_TRACE_ENV: &str = "ESP8266_HAL_SMARTCONFIG_RX_TRACE";
const SMARTCONFIG_RX_DIAG_ENV: &str = "ESP8266_HAL_SMARTCONFIG_RX_DIAG";
const WIFI_START_TRACE_ENV: &str = "ESP8266_HAL_WIFI_START_TRACE";
const DEFAULT_IROM_ORIGIN: u32 = 0x4021_0000;
const IROM_FLASH_BASE: u32 = 0x4020_0000;

fn main() {
    println!("cargo:rustc-check-cfg=cfg(esp8266_hal_smartconfig_sniffer_trace)");
    println!("cargo:rustc-check-cfg=cfg(esp8266_hal_smartconfig_event_trace)");
    println!("cargo:rustc-check-cfg=cfg(esp8266_hal_smartconfig_wifi_trace)");
    println!("cargo:rustc-check-cfg=cfg(esp8266_hal_smartconfig_rx_trace)");
    println!("cargo:rustc-check-cfg=cfg(esp8266_hal_wifi_start_trace)");

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
    File::create(out.join("link.x"))
        .unwrap()
        .write_all(include_bytes!("ld/link.x"))
        .unwrap();

    println!("cargo:rustc-link-search={}", out.display());

    if env::var_os("CARGO_FEATURE_WIFI").is_some() {
        // Only functional wraps — NO tracing wrappers.
        // PHY/radio tracing wraps (phy_get_bb_evm, pm_set_the_time_of_phy_init,
        // fix_cache_bug, set_crystal_uart, uart_tx_wait_idle, pm_wait4wakeup,
        // misc_nvs_init, wifi_nvs_init) were identified as the root cause of
        // WiFi RX failure: UART blocking in trace_char disrupted timing-sensitive
        // calibration sequences. Removing them restored WiFi connectivity.
        println!("cargo:rustc-link-arg=-Wl,--wrap=esp_event_send"); // event bridge (functional)
        println!("cargo:rustc-link-arg=-Wl,--wrap=ets_printf"); // no-op (prevents stack overflow)
                                                                // ets_delay_us: dual-mode wrapper. During init it passes through to the
                                                                // real ROM delay; after esp_wifi_init, controller.rs switches it to the
                                                                // FRC2-based runtime delay path implemented in sdk/delay.rs.
        println!("cargo:rustc-link-arg=-Wl,--wrap=ets_delay_us");
        println!("cargo:rustc-link-arg=-Wl,--wrap=nvs_open"); // NVS stub (functional)
        println!("cargo:rustc-link-arg=-Wl,--wrap=nvs_get_blob"); // NVS stub (functional)
        println!("cargo:rustc-link-arg=-Wl,--wrap=ieee80211_deliver_data"); // functional STA RX fast-path

        if env::var_os(WIFI_START_TRACE_ENV).is_some() {
            println!("cargo:rustc-cfg=esp8266_hal_wifi_start_trace");
        }

        if env::var_os("CARGO_FEATURE_WIFI_SMARTCONFIG").is_some()
            && env::var_os(SMARTCONFIG_EVENT_TRACE_ENV).is_some()
        {
            println!("cargo:rustc-cfg=esp8266_hal_smartconfig_event_trace");
        }

        if env::var_os("CARGO_FEATURE_WIFI_SMARTCONFIG").is_some()
            && env::var_os(SMARTCONFIG_SNIFFER_TRACE_ENV).is_some()
        {
            println!("cargo:rustc-cfg=esp8266_hal_smartconfig_sniffer_trace");
            println!("cargo:rustc-link-arg=-Wl,--wrap=sc_sniffer_cb");
        }

        let smartconfig_enabled = env::var_os("CARGO_FEATURE_WIFI_SMARTCONFIG").is_some();
        let smartconfig_wifi_trace = env::var_os(SMARTCONFIG_WIFI_TRACE_ENV).is_some();
        let smartconfig_rx_trace = env::var_os(SMARTCONFIG_RX_TRACE_ENV).is_some();

        if smartconfig_enabled && smartconfig_wifi_trace {
            println!("cargo:rustc-cfg=esp8266_hal_smartconfig_wifi_trace");
            println!("cargo:rustc-link-arg=-Wl,--wrap=esp_wifi_set_channel");
            println!("cargo:rustc-link-arg=-Wl,--wrap=esp_wifi_set_promiscuous_filter");
            println!("cargo:rustc-link-arg=-Wl,--wrap=esp_wifi_set_promiscuous_rx_cb");
            println!("cargo:rustc-link-arg=-Wl,--wrap=esp_wifi_set_promiscuous");
        }

        if smartconfig_enabled && smartconfig_rx_trace {
            if smartconfig_wifi_trace {
                panic!(
                    "{SMARTCONFIG_RX_TRACE_ENV} and {SMARTCONFIG_WIFI_TRACE_ENV} both wrap esp_wifi_set_promiscuous_rx_cb; enable only one"
                );
            }
            println!("cargo:rustc-cfg=esp8266_hal_smartconfig_rx_trace");
            println!("cargo:rustc-link-arg=-Wl,--wrap=esp_wifi_set_promiscuous_rx_cb");
        }

        if env::var_os("CARGO_FEATURE_WIFI_DEBUG_TRACE").is_some() {
            // Signal-0x05 RX-path stage wrappers. These wrappers only drive
            // in-memory probe state and are not part of the functional driver
            // path. Keep them opt-in so the shipping image does not pay their
            // RAM/code-size cost.
            println!("cargo:rustc-link-arg=-Wl,--wrap=pm_sleep_for");
            println!("cargo:rustc-link-arg=-Wl,--wrap=os_timer_disarm");
            println!("cargo:rustc-link-arg=-Wl,--wrap=os_timer_setfn");
            println!("cargo:rustc-link-arg=-Wl,--wrap=os_timer_arm");
            println!("cargo:rustc-link-arg=-Wl,--wrap=pm_send_keepalive_pkt");
            println!("cargo:rustc-link-arg=-Wl,--wrap=pm_set_idle_timer");
            println!("cargo:rustc-link-arg=-Wl,--wrap=pm_wait_tx_rx_idle_fun");
            println!("cargo:rustc-link-arg=-Wl,--wrap=pm_change_to_state");
            println!("cargo:rustc-link-arg=-Wl,--wrap=sta_input");
            println!("cargo:rustc-link-arg=-Wl,--wrap=hostap_input");
            println!("cargo:rustc-link-arg=-Wl,--wrap=ieee80211_recv_action");
            println!("cargo:rustc-link-arg=-Wl,--wrap=scan_parse_beacon");
            println!("cargo:rustc-link-arg=-Wl,--wrap=scan_build_chan_list");
            println!("cargo:rustc-link-arg=-Wl,--wrap=scan_hidden_ssid");
            println!("cargo:rustc-link-arg=-Wl,--wrap=ets_timer_proc");
            println!("cargo:rustc-link-arg=-Wl,--wrap=ets_timer_setfn");
            println!("cargo:rustc-link-arg=-Wl,--wrap=ets_timer_arm");
            println!("cargo:rustc-link-arg=-Wl,--wrap=scan_start");
            println!("cargo:rustc-link-arg=-Wl,--wrap=cnx_sta_associated");
            println!("cargo:rustc-link-arg=-Wl,--wrap=cnx_sta_connect_cmd");
            println!("cargo:rustc-link-arg=-Wl,--wrap=cnx_start_handoff_cb");
            println!("cargo:rustc-link-arg=-Wl,--wrap=cnx_handshake_timeout");
            println!("cargo:rustc-link-arg=-Wl,--wrap=cnx_assoc_timeout");
            println!("cargo:rustc-link-arg=-Wl,--wrap=cnx_auth_timeout");
            println!("cargo:rustc-link-arg=-Wl,--wrap=dhcp_bind_check");
            println!("cargo:rustc-link-arg=-Wl,--wrap=pp_post");
            println!("cargo:rustc-link-arg=-Wl,--wrap=ppEnqueueTxDone");
            println!("cargo:rustc-link-arg=-Wl,--wrap=rcUpdateTxDone");
            println!("cargo:rustc-link-arg=-Wl,--wrap=ppEnqueueRxq");
            println!("cargo:rustc-link-arg=-Wl,--wrap=lmacRxDone");
            println!("cargo:rustc-link-arg=-Wl,--wrap=ppProcessTxQ");
            println!("cargo:rustc-link-arg=-Wl,--wrap=tx_pwctrl_background");
            println!("cargo:rustc-link-arg=-Wl,--wrap=pm_reset_keepalive_time");
            println!("cargo:rustc-link-arg=-Wl,--wrap=ieee80211_decap");
            println!("cargo:rustc-link-arg=-Wl,--wrap=ieee80211_crypto_decap");
            println!("cargo:rustc-link-arg=-Wl,--wrap=ieee80211_setup_rates");
            println!("cargo:rustc-link-arg=-Wl,--wrap=ieee80211_sta_new_state");
            println!("cargo:rustc-link-arg=-Wl,--wrap=ieee80211_ccmp_decrypt");
            println!("cargo:rustc-link-arg=-Wl,--wrap=ieee80211_send_mgmt");
        }
    }

    // Re-run when linker scripts change.
    println!("cargo:rerun-if-changed=ld/memory.x");
    println!("cargo:rerun-if-changed=ld/link.x");
    println!("cargo:rerun-if-changed=ld/rom.x");
    println!("cargo:rerun-if-changed=ld/ram.x");
    println!("cargo:rerun-if-env-changed={IROM_FLASH_OFFSET_ENV}");
    println!("cargo:rerun-if-env-changed={SMARTCONFIG_EVENT_TRACE_ENV}");
    println!("cargo:rerun-if-env-changed={SMARTCONFIG_SNIFFER_TRACE_ENV}");
    println!("cargo:rerun-if-env-changed={SMARTCONFIG_WIFI_TRACE_ENV}");
    println!("cargo:rerun-if-env-changed={SMARTCONFIG_RX_TRACE_ENV}");
    println!("cargo:rerun-if-env-changed={SMARTCONFIG_RX_DIAG_ENV}");
    println!("cargo:rerun-if-env-changed={WIFI_START_TRACE_ENV}");
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
