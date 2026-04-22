use std::env;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

fn main() {
    // Put the linker script somewhere the linker can find it
    let out = &PathBuf::from(env::var_os("OUT_DIR").unwrap());
    File::create(out.join("memory.x"))
        .unwrap()
        .write_all(include_bytes!("ld/memory.x"))
        .unwrap();

    File::create(out.join("alias.x"))
        .unwrap()
        .write_all(if cfg!(feature = "all_in_ram") {
            include_bytes!("ld/ram.x")
        } else {
            include_bytes!("ld/rom.x")
        })
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
}
