//! Build script for esp8266-wifi-sys.
//!
//! 1. Generates `sdkconfig.h` with the CONFIG_* defines the SDK headers need.
//! 2. Runs bindgen to produce Rust FFI bindings from the SDK headers.
//! 3. Compiles the SDK C source files (FreeRTOS, esp_event, newlib, heap, log,
//!    wpa_supplicant, etc.) via the `cc` crate. A small lwIP subset is also
//!    compiled for the AP-side netif bridge; networking still remains
//!    caller-owned in the `esp8266-wifi` crate.
//! 4. Emits linker directives for the pre-built WiFi blob libraries and the
//!    ROM / peripheral / memory-layout linker scripts.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use embuild::git;

/// Pinned SDK commit. This is master HEAD (v3.4 + 112 commits) which includes
/// important bug fixes and updated pre-built WiFi blob libraries post-v3.4.
const SDK_COMMIT: &str = "da7ce37be424d71874241ca9f17cf8da0e3cdb70";
const SDK_REPO_URL: &str = "https://github.com/espressif/ESP8266_RTOS_SDK.git";
const WPS_ENROLLEE_TRACE_ENV: &str = "ESP8266_HAL_WPS_ENROLLEE_TRACE";
const EAP_TRACE_ENV: &str = "ESP8266_HAL_EAP_TRACE";
const MBEDTLS_SOFTWARE_AES_ENV: &str = "ESP8266_HAL_MBEDTLS_SOFTWARE_AES";

fn main() {
    let sdk_path = discover_sdk_path();
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    // Auto-detect the ESP Xtensa clang library if LIBCLANG_PATH is not set.
    if env::var("LIBCLANG_PATH").is_err() {
        if let Some(path) = find_esp_clang_lib() {
            env::set_var("LIBCLANG_PATH", &path);
        }
    }

    // Step 1: Generate sdkconfig.h
    let sdkconfig_h = out_dir.join("sdkconfig.h");
    generate_sdkconfig(&sdkconfig_h);
    let _ = fs::remove_file(out_dir.join("esp_wifi_driver.h"));

    // Build the master include-path list. The generated sdkconfig.h in OUT_DIR
    // comes first so it shadows any stale shim.
    let include_paths = build_include_paths(&sdk_path, &out_dir);

    // Validate paths.
    for path in &include_paths {
        if !path.exists() {
            println!(
                "cargo:warning=esp8266-wifi-sys: include path not found: {}",
                path.display()
            );
        }
    }

    // Step 2: Bindgen
    run_bindgen(&manifest_dir, &include_paths, &out_dir);

    // Step 3: Compile SDK C sources
    let gcc = find_xtensa_gcc();
    compile_sdk_sources(
        &sdk_path,
        &manifest_dir,
        &include_paths,
        &out_dir,
        gcc.as_deref(),
    );

    // Step 4: Link directives for pre-built WiFi blobs and linker scripts
    emit_link_directives(&sdk_path);

    // Rebuild triggers
    println!("cargo:rerun-if-env-changed=ESP8266_RTOS_SDK");
    println!("cargo:rerun-if-env-changed={WPS_ENROLLEE_TRACE_ENV}");
    println!("cargo:rerun-if-env-changed={EAP_TRACE_ENV}");
    println!("cargo:rerun-if-env-changed={MBEDTLS_SOFTWARE_AES_ENV}");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=ap_netif_bridge.c");
    // sdk_overrides/ fully ported to Rust (src/wifi/sdk/).
}

// ===========================================================================
// Step 1: sdkconfig.h generation
// ===========================================================================

fn generate_sdkconfig(path: &Path) {
    let content = r#"/*
 * Auto-generated sdkconfig.h for esp8266-wifi-sys.
 *
 * Provides the CONFIG_* defines that FreeRTOS, lwIP, and WiFi headers need
 * so that both bindgen and cc can parse/compile them without a full SDK build.
 * Values match the SDK Kconfig defaults.
 */

#ifndef SDKCONFIG_H
#define SDKCONFIG_H

/* ---------- IDF target ---------- */
#define CONFIG_IDF_TARGET_ESP8266 1

/* ---------- FreeRTOS ---------- */
#define CONFIG_FREERTOS_HZ 100
#define CONFIG_FREERTOS_IDLE_TASK_STACKSIZE 1024
#define CONFIG_FREERTOS_TIMER_STACKSIZE 2048
#define CONFIG_FREERTOS_ISR_STACKSIZE 512

/* ---------- Task stack sizes (esp_task.h) ---------- */
#define CONFIG_ESP_TIMER_TASK_STACK_SIZE 2048
#define CONFIG_ESP_SYSTEM_EVENT_TASK_STACK_SIZE 1024
#define CONFIG_ESP_MAIN_TASK_STACK_SIZE 3584
#define CONFIG_LWIP_TCPIP_TASK_STACK_SIZE 2048

/* ---------- System event ---------- */
#define CONFIG_ESP_SYSTEM_EVENT_QUEUE_SIZE 8

/* ---------- WiFi ---------- */
#define CONFIG_ESP8266_WIFI_RX_BUFFER_NUM 16
#define CONFIG_ESP8266_WIFI_RX_PKT_NUM 7
#define CONFIG_ESP8266_WIFI_LEFT_CONTINUOUS_RX_BUFFER_NUM 16
#define CONFIG_ESP8266_WIFI_TX_PKT_NUM 6
#define CONFIG_ESP8266_WIFI_NVS_ENABLED 1
#define CONFIG_ESP8266_WIFI_ENABLE_WPA3_SAE 1
#define CONFIG_ESP8266_WIFI_AMPDU_RX_ENABLED 0
#define CONFIG_ESP8266_WIFI_RX_BA_WIN_SIZE 6
#define CONFIG_ESP8266_WIFI_QOS_ENABLED 0
#define CONFIG_ESP8266_WIFI_AMSDU_ENABLED 0
#define CONFIG_WIFI_PPT_TASKSTACK_SIZE 5120
#define CONFIG_SCAN_AP_MAX 99

/* ---------- lwIP ---------- */
#define CONFIG_LWIP_MAX_SOCKETS 8
#define CONFIG_LWIP_MAX_RAW_PCBS 16
#define CONFIG_LWIP_MAX_ACTIVE_TCP 16
#define CONFIG_LWIP_MAX_LISTENING_TCP 16
#define CONFIG_LWIP_MAX_UDP_PCBS 16
#define CONFIG_LWIP_IP4_REASSEMBLY 0
#define CONFIG_LWIP_IP6_REASSEMBLY 0
#define CONFIG_LWIP_IP4_FRAG 0
#define CONFIG_LWIP_IP6_FRAG 0
#define CONFIG_LWIP_IP_FORWARD 0
#define CONFIG_LWIP_IPV4_NAPT 0
#define CONFIG_LWIP_BROADCAST_PING 0
#define CONFIG_LWIP_MULTICAST_PING 0
#define CONFIG_LWIP_DHCP_DOES_ARP_CHECK 1
#define CONFIG_LWIP_DHCP_MAX_NTP_SERVERS 1
#define CONFIG_LWIP_DNS_SUPPORT_MDNS_QUERIES 0
#define CONFIG_LWIP_TCP_MSS 1460
#define CONFIG_LWIP_TCP_SND_BUF_DEFAULT 2920
#define CONFIG_LWIP_TCP_WND_DEFAULT 5840
#define CONFIG_LWIP_TCP_MAXRTX 12
#define CONFIG_LWIP_TCP_SYNMAXRTX 6
#define CONFIG_LWIP_TCP_MSL 60000
#define CONFIG_LWIP_TCP_TMR_INTERVAL 250
#define CONFIG_LWIP_TCP_RTO_TIME 3000
#define CONFIG_LWIP_TCP_QUEUE_OOSEQ 1
#define CONFIG_LWIP_TCPIP_RECVMBOX_SIZE 32
#define CONFIG_LWIP_TCP_RECVMBOX_SIZE 6
#define CONFIG_LWIP_UDP_RECVMBOX_SIZE 6
#define CONFIG_LWIP_SO_REUSE 1
#define CONFIG_LWIP_SO_REUSE_RXTOALL 0
#define CONFIG_LWIP_SO_RCVBUF 0
#define CONFIG_LWIP_SO_LINGER 0
#define CONFIG_LWIP_ETHARP_TRUST_IP_MAC 0
#define CONFIG_LWIP_ESP_GRATUITOUS_ARP 0
#define CONFIG_LWIP_NETIF_LOOPBACK 0
#define CONFIG_LWIP_LOOPBACK_MAX_PBUFS 0
#define CONFIG_LWIP_NETBUF_RECVINFO 0
#define CONFIG_LWIP_TIMERS_ONDEMAND 0
#define CONFIG_LWIP_IPV6 0
#define CONFIG_LWIP_IPV6_MEMP_NUM_ND6_QUEUE 3
#define CONFIG_LWIP_IPV6_ND6_NUM_NEIGHBORS 5
#define CONFIG_LWIP_TCP_SACK_OUT 0
#define CONFIG_LWIP_WND_SCALE 0
#define CONFIG_LWIP_TCP_RCV_SCALE 0
#define CONFIG_LWIP_TCP_OVERSIZE_MSS 1
#define CONFIG_LWIP_TCP_KEEP_CONNECTION_WHEN_IP_CHANGES 0
#define CONFIG_LWIP_SOCKET 1
#define CONFIG_LWIP_DNS 1
#define CONFIG_LWIP_DHCPS_LEASE_UNIT 60
#define CONFIG_LWIP_DHCPS_MAX_STATION_NUM 8
#define CONFIG_LWIP_SNTP_UPDATE_DELAY 3600000
#define CONFIG_LWIP_LOCAL_HOSTNAME "espressif"
#define CONFIG_LWIP_L2_TO_L3_COPY 0
#define CONFIG_LWIP_STATS 0
#define CONFIG_LWIP_PPP_SUPPORT 0
#define CONFIG_IP_LOST_TIMER_INTERVAL 120
#define CONFIG_GARP_TMR_INTERVAL 60
#define CONFIG_TCPIP_LWIP 1

/* ---------- Time syscall ---------- */
#define CONFIG_ESP8266_TIME_SYSCALL_USE_FRC1 1

/* ---------- System ---------- */
#define CONFIG_ESP8266_XTAL_FREQ 26
#define CONFIG_ESP8266_XTAL_FREQ_26 1
#define CONFIG_SOC_IRAM_SIZE 0xC000
#define CONFIG_SOC_FULL_ICACHE 0
#define CONFIG_NEWLIB_STDOUT_LINE_BUFFERING 1
#define CONFIG_RESET_REASON 1

/* ---------- Console UART ---------- */
#define CONFIG_CONSOLE_UART_BAUDRATE 74880
#define CONFIG_ESP_CONSOLE_UART_BAUDRATE 74880
#define CONFIG_ESP_CONSOLE_UART_NUM 0

/* ---------- SPI Flash ---------- */
#define CONFIG_SPI_FLASH_SIZE 0x200000
#define CONFIG_SPI_FLASH_FREQ 0x0
#define CONFIG_SPI_FLASH_MODE 0x0

/* ---------- Task WDT ---------- */
#define CONFIG_ESP_TASK_WDT 1
#define CONFIG_ESP_TASK_WDT_TIMEOUT_S 15
#define CONFIG_ESP_TASK_WDT_PANIC 0
#define CONFIG_TASK_WDT 1
#define CONFIG_TASK_WDT_TIMEOUT_S 15
#define CONFIG_TASK_WDT_PANIC 0

/* ---------- Log ---------- */
#define CONFIG_LOG_DEFAULT_LEVEL 3
#define CONFIG_LOG_COLORS 1

/* ---------- NVS ---------- */
#define CONFIG_NVS_COMPATIBLE_MODE 1

/* ---------- Bootloader / partition table ---------- */
#define CONFIG_BOOTLOADER_DATA_FACTORY_AND_OTA 1
#define CONFIG_PARTITION_TABLE_OFFSET 0x8000

/* ---------- WPA supplicant ---------- */
#define CONFIG_WPA_MBEDTLS_CRYPTO 1

/* ---------- mbedTLS ---------- */
#define CONFIG_MBEDTLS_ASYMMETRIC_CONTENT_LEN 1
/* The WPA2 Enterprise fixtures use compact EAP-TLS certificates and keep the
 * largest observed server flight below this inbound record budget. The smaller
 * buffer leaves verifier-time heap for P-256 X.509 bignum work. */
#define CONFIG_MBEDTLS_SSL_IN_CONTENT_LEN 768
#define CONFIG_MBEDTLS_SSL_OUT_CONTENT_LEN 512
#define MBEDTLS_MPI_MAX_SIZE 256
#define MBEDTLS_MPI_WINDOW_SIZE 2
#define MBEDTLS_ECP_MAX_BITS 256
#define MBEDTLS_ECP_WINDOW_SIZE 2
#define MBEDTLS_ECP_FIXED_POINT_OPTIM 0
#define CONFIG_MBEDTLS_TLS_ENABLED 1
#define CONFIG_MBEDTLS_TLS_CLIENT 1
#define CONFIG_MBEDTLS_TLS_SERVER 1
#define CONFIG_MBEDTLS_SSL_PROTO_TLS1 1
#define CONFIG_MBEDTLS_SSL_PROTO_TLS1_1 1
#define CONFIG_MBEDTLS_SSL_PROTO_TLS1_2 1
/* Crypto primitives */
#define CONFIG_MBEDTLS_AES_C 1
#define CONFIG_MBEDTLS_CCM_C 1
#define CONFIG_MBEDTLS_GCM_C 1
#define CONFIG_MBEDTLS_DES_C 1
#define CONFIG_MBEDTLS_BLOWFISH_C 1
#define CONFIG_MBEDTLS_CAMELLIA_C 1
/* Asymmetric / EC */
#define CONFIG_MBEDTLS_ECDSA_C 1
#define CONFIG_MBEDTLS_ECP_C 1
#define CONFIG_MBEDTLS_ECDH_C 1
#define CONFIG_MBEDTLS_BIGNUM_C 1
#define CONFIG_MBEDTLS_PK_C 1
#define CONFIG_MBEDTLS_ECP_DP_SECP256R1_ENABLED 1
#define CONFIG_MBEDTLS_ECP_NIST_OPTIM 1
/* Key exchange */
#define CONFIG_MBEDTLS_KEY_EXCHANGE_RSA 1
#define CONFIG_MBEDTLS_KEY_EXCHANGE_ECDHE_RSA 1
#define CONFIG_MBEDTLS_KEY_EXCHANGE_ECDHE_ECDSA 1
/* PEM / X.509 */
#define CONFIG_MBEDTLS_PEM_PARSE_C 1
#define CONFIG_MBEDTLS_PEM_WRITE_C 1
/* CRL parsing is not surfaced by the HAL EAP configuration and the TLS client
 * passes a NULL CRL chain; leave it disabled to reduce verifier pressure. */
/* #define CONFIG_MBEDTLS_X509_CRL_PARSE_C 1 */

/* ---------- ESP filename macro ---------- */
#define __ESP_FILE__ __FILE__

/* ---------- PHY ---------- */
#define CONFIG_ESP_PHY_CALIBRATION_AND_DATA_STORAGE 1
#define CONFIG_ESP8266_PHY_MAX_WIFI_TX_POWER 20

/* ---------- Heap ---------- */
/* CONFIG_HEAP_TRACING left undefined (disabled) */

/* ---------- App update ---------- */
#define CONFIG_APP_EXCLUDE_PROJECT_VER_VAR 0
#define CONFIG_APP_EXCLUDE_PROJECT_NAME_VAR 0

/* ---------- VFS ---------- */
#define CONFIG_VFS_SUPPORT_TERMIOS 1
#define CONFIG_SUPPORT_TERMIOS 1

#endif /* SDKCONFIG_H */
"#;
    fs::write(path, content).expect("failed to write sdkconfig.h");
}

// ===========================================================================
// Include paths
// ===========================================================================

fn build_include_paths(sdk: &Path, out_dir: &Path) -> Vec<PathBuf> {
    vec![
        // Generated sdkconfig.h lives here — must come first.
        out_dir.to_path_buf(),
        // ESP8266 component
        sdk.join("components/esp8266/include"),
        sdk.join("components/esp8266/include/driver"),
        sdk.join("components/esp8266/source"),
        // esp_common
        sdk.join("components/esp_common/include"),
        // esp_event
        sdk.join("components/esp_event/include"),
        sdk.join("components/esp_event/private_include"),
        // tcpip_adapter
        sdk.join("components/tcpip_adapter/include"),
        // lwIP
        sdk.join("components/lwip/include/apps"),
        sdk.join("components/lwip/include/apps/sntp"),
        sdk.join("components/lwip/lwip/src/include"),
        sdk.join("components/lwip/port/esp8266/include"),
        sdk.join("components/lwip/port/esp8266/include/arch"),
        // FreeRTOS
        sdk.join("components/freertos/include"),
        sdk.join("components/freertos/include/freertos"),
        sdk.join("components/freertos/include/freertos/private"),
        sdk.join("components/freertos/port/esp8266/include"),
        sdk.join("components/freertos/port/esp8266/include/freertos"),
        // heap
        sdk.join("components/heap/include"),
        sdk.join("components/heap/port/esp8266/include"),
        // newlib
        sdk.join("components/newlib/platform_include"),
        // VFS
        sdk.join("components/vfs/include"),
        // wpa_supplicant
        sdk.join("components/wpa_supplicant/include"),
        sdk.join("components/wpa_supplicant/include/esp_supplicant"),
        sdk.join("components/wpa_supplicant/port/include"),
        sdk.join("components/wpa_supplicant/src"),
        sdk.join("components/wpa_supplicant/src/esp_supplicant"),
        // log
        sdk.join("components/log/include"),
        // spi_flash
        sdk.join("components/spi_flash/include"),
        // bootloader_support
        sdk.join("components/bootloader_support/include"),
        sdk.join("components/bootloader_support/include_priv"),
        // app_update
        sdk.join("components/app_update/include"),
        // nvs_flash
        sdk.join("components/nvs_flash/include"),
        // esp_ringbuf
        sdk.join("components/esp_ringbuf/include"),
        sdk.join("components/esp_ringbuf/include/freertos"),
        // mbedtls
        sdk.join("components/mbedtls/port/include"),
        sdk.join("components/mbedtls/port/include/esp8266"),
        sdk.join("components/mbedtls/mbedtls/include"),
    ]
}

// ===========================================================================
// Step 2: Bindgen
// ===========================================================================

fn run_bindgen(manifest_dir: &Path, include_paths: &[PathBuf], out_dir: &Path) {
    let wrapper_h = manifest_dir.join("wrapper.h");

    let mut builder = bindgen::Builder::default()
        .header(wrapper_h.to_str().unwrap())
        .use_core()
        .ctypes_prefix("core::ffi")
        .layout_tests(false)
        .derive_debug(true)
        .derive_copy(true)
        .derive_default(false)
        .default_enum_style(bindgen::EnumVariation::Consts)
        .merge_extern_blocks(true)
        .generate_comments(false)
        .prepend_enum_name(false)
        .clang_arg("-DESP_PLATFORM")
        .clang_arg("-D__XTENSA__")
        .clang_arg("-D__xtensa__")
        .clang_arg("--target=xtensa-esp-elf")
        .clang_arg("-ffreestanding");

    if let Some(newlib_inc) = find_newlib_include() {
        builder = builder.clang_arg(format!("-isystem{}", newlib_inc.display()));
    }

    for path in include_paths {
        builder = builder.clang_arg(format!("-I{}", path.display()));
    }

    // --- Allowlists ------------------------------------------------------

    // WiFi functions
    builder = builder
        .allowlist_function("esp_wifi_init")
        .allowlist_function("esp_wifi_deinit")
        .allowlist_function("esp_wifi_start")
        .allowlist_function("esp_wifi_stop")
        .allowlist_function("esp_wifi_restore")
        .allowlist_function("esp_wifi_get_state")
        .allowlist_function("esp_wifi_set_mode")
        .allowlist_function("esp_wifi_get_mode")
        .allowlist_function("esp_wifi_set_config")
        .allowlist_function("esp_wifi_get_config")
        .allowlist_function("esp_wifi_connect")
        .allowlist_function("esp_wifi_disconnect")
        .allowlist_function("esp_wifi_clear_fast_connect")
        .allowlist_function("esp_wifi_sta_get_ap_info")
        .allowlist_function("esp_wifi_deauth_sta")
        .allowlist_function("esp_wifi_ap_get_sta_list")
        .allowlist_function("esp_wifi_scan_start")
        .allowlist_function("esp_wifi_scan_stop")
        .allowlist_function("esp_wifi_scan_get_ap_num")
        .allowlist_function("esp_wifi_scan_get_ap_records")
        .allowlist_function("esp_wifi_set_channel")
        .allowlist_function("esp_wifi_get_channel")
        .allowlist_function("esp_wifi_set_bandwidth")
        .allowlist_function("esp_wifi_get_bandwidth")
        .allowlist_function("esp_wifi_set_protocol")
        .allowlist_function("esp_wifi_get_protocol")
        .allowlist_function("esp_wifi_set_country")
        .allowlist_function("esp_wifi_get_country")
        .allowlist_function("esp_wifi_set_mac")
        .allowlist_function("esp_wifi_get_mac")
        .allowlist_function("esp_wifi_set_ps")
        .allowlist_function("esp_wifi_get_ps")
        .allowlist_function("esp_wifi_set_max_tx_power")
        .allowlist_function("esp_wifi_get_max_tx_power")
        .allowlist_function("esp_wifi_set_max_tx_power_via_vdd33")
        .allowlist_function("esp_wifi_get_vdd33")
        .allowlist_function("esp_wifi_set_storage")
        .allowlist_function("esp_wifi_set_event_mask")
        .allowlist_function("esp_wifi_get_event_mask")
        .allowlist_function("esp_wifi_set_promiscuous_rx_cb")
        .allowlist_function("esp_wifi_set_promiscuous")
        .allowlist_function("esp_wifi_get_promiscuous")
        .allowlist_function("esp_wifi_set_promiscuous_filter")
        .allowlist_function("esp_wifi_get_promiscuous_filter")
        .allowlist_function("esp_wifi_set_promiscuous_ctrl_filter")
        .allowlist_function("esp_wifi_get_promiscuous_ctrl_filter")
        .allowlist_function("esp_wifi_set_vendor_ie")
        .allowlist_function("esp_wifi_set_vendor_ie_cb")
        .allowlist_function("esp_wifi_80211_tx")
        .allowlist_function("esp_wifi_set_rssi_threshold")
        .allowlist_function("esp_wifi_get_tsf_time")
        .allowlist_function("esp_wifi_set_inactive_time")
        .allowlist_function("esp_wifi_get_inactive_time");

    // WiFi internal functions
    builder = builder
        .allowlist_function("esp_wifi_internal_free_rx_buffer")
        .allowlist_function("esp_wifi_internal_tx")
        .allowlist_function("esp_wifi_internal_reg_rxcb")
        .allowlist_function("esp_wifi_internal_set_log_level")
        .allowlist_function("esp_wifi_internal_set_log_mod")
        .allowlist_function("esp_wifi_internal_get_log");

    // Event loop functions
    builder = builder
        .allowlist_function("esp_event_loop_create_default")
        .allowlist_function("esp_event_handler_register")
        .allowlist_function("esp_event_handler_unregister")
        .allowlist_function("esp_event_post");

    // Timer functions
    builder = builder.allowlist_function("esp_timer_get_time");

    // SmartConfig functions
    builder = builder
        .allowlist_function("esp_smartconfig_.*")
        .allowlist_function("esp_esptouch_set_timeout");

    // WPS functions
    builder = builder.allowlist_function("esp_wifi_wps_.*");

    // WPA2 Enterprise station functions
    builder = builder.allowlist_function("esp_wifi_sta_wpa2_ent_.*");

    // ESP-NOW functions
    builder = builder.allowlist_function("esp_now_.*");

    // System functions
    builder = builder
        .allowlist_function("esp_restart")
        .allowlist_function("esp_get_free_heap_size")
        .allowlist_function("esp_get_minimum_free_heap_size")
        .allowlist_function("esp_random")
        .allowlist_function("esp_fill_random")
        .allowlist_function("esp_read_mac")
        .allowlist_function("esp_base_mac_addr_set")
        .allowlist_function("esp_base_mac_addr_get");

    // Variables (event base externs)
    builder = builder
        .allowlist_var("WIFI_EVENT")
        .allowlist_var("SC_EVENT");

    // Macro constants
    builder = builder
        .allowlist_var("WIFI_PROTOCOL_.*")
        .allowlist_var("WIFI_PROMIS_FILTER_MASK_.*")
        .allowlist_var("WIFI_PROMIS_CTRL_FILTER_MASK_.*")
        .allowlist_var("ESP_NOW_MAX_DATA_LEN")
        .allowlist_var("ESP_NOW_ETH_ALEN")
        .allowlist_var("ESP_NOW_KEY_LEN");

    // Types
    builder = builder
        .allowlist_type("wifi_.*")
        .allowlist_type("esp_interface_t")
        .allowlist_type("esp_err_t")
        .allowlist_type("esp_event_handler_t")
        .allowlist_type("esp_event_base_t")
        .allowlist_type("esp_event_loop_handle_t")
        .allowlist_type("smartconfig_.*")
        .allowlist_type("wps_.*")
        .allowlist_type("esp_wps_config_t")
        .allowlist_type("wps_factory_information_t")
        .allowlist_type("esp_now_.*")
        .allowlist_type("vendor_ie_data_t")
        .allowlist_type("esp_pm_config.*")
        .allowlist_type("esp_mac_type_t")
        .allowlist_type("esp_cpu_freq_t")
        .allowlist_type("esp_reset_reason_t");

    // Blocklist
    builder = builder
        .blocklist_type("QueueHandle_t")
        .blocklist_type("SemaphoreHandle_t")
        .blocklist_type("TaskHandle_t")
        .blocklist_type("xQueueHandle")
        .blocklist_type("FILE")
        .blocklist_type("_reent")
        .blocklist_type("__sFILE.*")
        .blocklist_type("__FILE")
        .blocklist_type("wifi_pkt_rx_ctrl_t")
        .blocklist_type("wifi_promiscuous_pkt_t");

    let bindings = builder
        .generate()
        .expect("bindgen failed to generate bindings");

    bindings
        .write_to_file(out_dir.join("bindings.rs"))
        .expect("failed to write bindings.rs");
}

// ===========================================================================
// Step 3: Compile SDK C sources
// ===========================================================================

/// Apply common settings to a cc::Build: compiler, optimisation, and includes.
fn init_cc_build(build: &mut cc::Build, gcc: Option<&str>, include_paths: &[PathBuf]) {
    if let Some(gcc_path) = gcc {
        build.compiler(gcc_path);
    }

    build
        .opt_level_str("s")
        .warnings(false)
        .extra_warnings(false)
        .flag("-mlongcalls");

    for inc in include_paths {
        build.include(inc);
    }

    if let Some(newlib_inc) = find_newlib_include() {
        build.include(&newlib_inc);
    }

    build
        .define("ESP_PLATFORM", None)
        .define("__XTENSA__", None)
        .define("__xtensa__", None)
        .define("__ESP_FILE__", "__FILE__");
}

fn compile_sdk_sources(
    sdk: &Path,
    manifest_dir: &Path,
    include_paths: &[PathBuf],
    out_dir: &Path,
    gcc: Option<&str>,
) {
    let mut build = cc::Build::new();
    init_cc_build(&mut build, gcc, include_paths);

    // Extra defines for the main SDK build.
    build
        .define("USING_IBUS_FASTER_GET", None)
        .define("MBEDTLS_CONFIG_FILE", "\"mbedtls/esp_config.h\"")
        .define("CONFIG_SSL_USING_MBEDTLS", None)
        .define("IDF_VER", "\"v3.4\"");

    // Collect source files per component.
    let comp = |rel: &str| sdk.join(rel);
    let local = |rel: &str| manifest_dir.join(rel);

    // --- FreeRTOS ---
    // The FreeRTOS C sources (tasks.c, queue.c, timers.c, port.c, etc.) are
    // replaced by the Rust compat layer in esp8266-wifi/src/compat/. Only the
    // assembly files for context save/restore are still compiled here. The
    // FreeRTOS *include paths* are kept above since remaining C sources and
    // the assembly files reference FreeRTOS header types/constants.

    // --- FreeRTOS assembly (context switch, interrupt vectors) ---
    // These must be compiled by the SDK's xtensa-lx106-elf-gcc because they
    // use Xtensa-specific assembler directives and macros.
    //
    // xtensa_vectors.S defines exception vector entry points that overlap with
    // xtensa-lx-rt. We compile it anyway because it also provides _xt_user_exit
    // (exception return sequence) needed by the compat layer. The duplicate
    // vector symbols are resolved by --allow-multiple-definition (the first
    // definition from xtensa-lx-rt wins for the entry points).
    let asm_srcs = [
        "components/freertos/port/esp8266/xtensa_vectors.S",
        "components/freertos/port/esp8266/xtensa_context.S",
        "components/freertos/port/esp8266/os_cpu_a.S",
    ];
    let mut asm_build = cc::Build::new();
    init_cc_build(&mut asm_build, gcc, include_paths);
    asm_build
        .define("IDF_VER", "\"v3.4\"")
        // The SDK's CMake build passes `-include sdkconfig.h` globally. Assembly
        // sources need CONFIG_FREERTOS_ISR_STACKSIZE and other defines from it.
        .flag(&format!("-include{}/sdkconfig.h", out_dir.display()));
    for src in &asm_srcs {
        asm_build.file(comp(src));
    }
    // NMI handler that overrides xtensa-lx-rt's default to call the WiFi
    // blob's wDev_ProcessFiq. Essential for register_chipv6_phy's sleep/wake.
    let nmi_handler = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("nmi_handler.S");
    asm_build.file(&nmi_handler);
    asm_build.compile("xtensa_asm");

    // --- esp_event ---
    let esp_event_srcs = [
        "components/esp_event/default_event_loop.c",
        "components/esp_event/esp_event.c",
        "components/esp_event/esp_event_private.c",
        "components/esp_event/event_loop_legacy.c",
        "components/esp_event/event_send.c",
    ];
    for src in &esp_event_srcs {
        build.file(comp(src));
    }

    // --- newlib ---
    let newlib_srcs = [
        "components/newlib/src/esp_malloc.c",
        "components/newlib/src/locks.c",
        "components/newlib/src/random.c",
        "components/newlib/src/reent_init.c",
        "components/newlib/src/select.c",
        "components/newlib/src/syscall.c",
        "components/newlib/src/termios.c",
        "components/newlib/src/time.c",
    ];
    for src in &newlib_srcs {
        build.file(comp(src));
    }

    // --- heap ---
    // The C heap (heap_caps) is replaced by a Rust allocator in
    // src/wifi/compat/memory.rs (using linked_list_allocator), matching the
    // esp-alloc approach on ESP32. The Rust module exports _heap_caps_*,
    // pvPortMalloc/vPortFree, heap_caps_init, and esp_get_free_heap_size
    // symbols so all C callers link correctly.

    // --- log ---
    // The full SDK log component routes through newlib FILE streams,
    // recursive locks, and asprintf/vasprintf. In our bare-metal startup path
    // that is a large dependency surface for very early WiFi init, so we
    // provide a minimal UART-backed implementation in sdk_stubs.c instead.

    // --- esp_common ---
    let esp_common_srcs = [
        "components/esp_common/src/esp_err_to_name.c",
        "components/esp_common/src/stack_check.c",
    ];
    for src in &esp_common_srcs {
        build.file(comp(src));
    }

    // --- VFS ---
    let vfs_srcs = [
        "components/vfs/vfs.c",
        "components/vfs/vfs_uart.c",
        "components/vfs/vfs_semihost.c",
    ];
    for src in &vfs_srcs {
        build.file(comp(src));
    }

    // --- esp_ringbuf ---
    build.file(comp("components/esp_ringbuf/ringbuf.c"));

    // --- spi_flash ---
    // spi_flash_raw.c is replaced by a local IRAM-attributed override
    // because Cache_Read_Enable_2/Disable_2 must be in IRAM (called
    // while flash cache is disabled by libphy's fix_cache_bug).
    build.file(comp("components/spi_flash/src/partition.c"));
    // spi_flash_raw.c ported to Rust: src/wifi/sdk/cache.rs
    // Previous hang was caused by wrong register addresses, now fixed.
    build.file(comp("components/spi_flash/src/spi_flash.c"));

    // --- bootloader_support ---
    let boot_srcs = [
        "components/bootloader_support/src/bootloader_common.c",
        "components/bootloader_support/src/bootloader_flash.c",
        "components/bootloader_support/src/bootloader_random.c",
        "components/bootloader_support/src/bootloader_sha.c",
        "components/bootloader_support/src/bootloader_utility.c",
        "components/bootloader_support/src/efuse.c",
        "components/bootloader_support/src/esp_image_format.c",
        "components/bootloader_support/src/flash_encrypt.c",
        "components/bootloader_support/src/flash_partitions.c",
        "components/bootloader_support/src/flash_qio_mode.c",
        "components/bootloader_support/src/secure_boot.c",
        "components/bootloader_support/src/secure_boot_signatures.c",
    ];
    for src in &boot_srcs {
        build.file(comp(src));
    }

    // --- app_update ---
    build.file(comp("components/app_update/esp_ota_ops.c"));
    // esp_app_desc.c needs PROJECT_VER and PROJECT_NAME (normally injected by
    // CMake). We supply sensible defaults.
    build
        .file(comp("components/app_update/esp_app_desc.c"))
        .define("PROJECT_VER", "\"0.1.0\"")
        .define("PROJECT_NAME", "\"esp8266-wifi\"");

    // --- lwIP subset (AP-side netif bridge only) ---
    //
    // We do not enable the full SDK tcpip_adapter/lwIP ownership model. The
    // Rust driver remains the packet owner. However, hardware validation shows
    // the ESP8266 soft-AP data plane needs an AP-side netif brought up before
    // client traffic is forwarded to the registered high-layer RX callback.
    //
    // Compile the minimum open-source lwIP core needed to create a tcpip
    // thread, allocate a netif, and mark the AP-side netif up. Socket/VFS
    // layers and the SDK tcpip_adapter event handlers remain out of scope.
    let lwip_srcs = [
        "components/lwip/lwip/src/api/err.c",
        "components/lwip/lwip/src/api/tcpip.c",
        "components/lwip/lwip/src/core/def.c",
        "components/lwip/lwip/src/core/dns.c",
        "components/lwip/lwip/src/core/inet_chksum.c",
        "components/lwip/lwip/src/core/init.c",
        "components/lwip/lwip/src/core/ip.c",
        "components/lwip/lwip/src/core/mem.c",
        "components/lwip/lwip/src/core/memp.c",
        "components/lwip/lwip/src/core/netif.c",
        "components/lwip/lwip/src/core/pbuf.c",
        "components/lwip/lwip/src/core/raw.c",
        "components/lwip/lwip/src/core/stats.c",
        "components/lwip/lwip/src/core/sys.c",
        "components/lwip/lwip/src/core/tcp.c",
        "components/lwip/lwip/src/core/tcp_in.c",
        "components/lwip/lwip/src/core/tcp_out.c",
        "components/lwip/lwip/src/core/timeouts.c",
        "components/lwip/lwip/src/core/udp.c",
        "components/lwip/lwip/src/core/ipv4/autoip.c",
        "components/lwip/lwip/src/core/ipv4/dhcp.c",
        "components/lwip/lwip/src/core/ipv4/etharp.c",
        "components/lwip/lwip/src/core/ipv4/icmp.c",
        "components/lwip/lwip/src/core/ipv4/igmp.c",
        "components/lwip/lwip/src/core/ipv4/ip4.c",
        "components/lwip/lwip/src/core/ipv4/ip4_addr.c",
        "components/lwip/lwip/src/core/ipv4/ip4_frag.c",
        "components/lwip/lwip/src/netif/ethernet.c",
        "components/lwip/port/esp8266/freertos/sys_arch.c",
    ];
    for src in &lwip_srcs {
        build.file(comp(src));
    }
    build.file(local("ap_netif_bridge.c"));

    // --- esp8266 component ---
    let esp8266_srcs = [
        "components/esp8266/source/chip_boot.c",
        "components/esp8266/source/backtrace.c",
        "components/esp8266/source/esp_sleep.c",
        "components/esp8266/source/esp_timer.c",
        "components/esp8266/source/ets_printf.c",
        "components/esp8266/source/crc.c",
        "components/esp8266/source/reset_reason.c",
        "components/esp8266/source/startup.c",
        "components/esp8266/source/task_wdt.c",
        "components/esp8266/source/rom.c",
        "components/esp8266/source/hw_random.c",
        "components/esp8266/source/esp_fast_boot.c",
        // Only include drivers needed by WiFi runtime. adc.c, i2c.c, i2s.c,
        // pwm.c, spi.c, ir_tx/rx.c, ledc.c are peripheral drivers not
        // required for WiFi and some fail with GCC 15 (ROM function mismatches).
        "components/esp8266/driver/gpio.c",
        "components/esp8266/driver/uart.c",
    ];
    for src in &esp8266_srcs {
        build.file(comp(src));
    }
    if env::var("CARGO_FEATURE_SMARTCONFIG").is_ok() {
        build.file(comp("components/esp8266/source/smartconfig.c"));
    }
    // All sdk_overrides ported to Rust (src/wifi/sdk/).

    // --- Stubs for symbols not provided by compiled SDK sources ---
    //
    // Several always-compiled SDK sources reference functions that either live
    // in components we don't compile (lwIP/tcpip_adapter, NVS, FreeRTOS idle
    // task) or are normally provided by the C runtime. We provide no-op / stub
    // implementations so linking succeeds.
    let stubs_c = out_dir.join("sdk_stubs.c");
    fs::write(
        &stubs_c,
        r#"
#include "esp_err.h"
#include "esp_log.h"
#include "esp_libc.h"
#include "esp_smartconfig.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "rom/ets_sys.h"
#include <errno.h>
#include <pthread.h>
#include <stdarg.h>
#include <stdint.h>
#include <string.h>

/* ---- tcpip_adapter compatibility (full adapter still disabled) ---- */

int tcpip_adapter_set_default_wifi_handlers(void) { return ESP_OK; }
int tcpip_adapter_clear_default_wifi_handlers(void) { return ESP_OK; }
int tcpip_adapter_get_ip_info(int tcpip_if, void *ip_info) {
    return 0x106; /* ESP_ERR_INVALID_STATE */
}
void tcpip_adapter_init(void) {}

/* SmartConfig ACK: keep ACK ownership out of the SDK socket task. */
esp_err_t sc_send_ack_start(smartconfig_type_t type, uint8_t token, uint8_t *cellphone_ip) {
    (void)type;
    (void)token;
    (void)cellphone_ip;
    return ESP_OK;
}
void sc_send_ack_stop(void) {}

/* ---- IP_EVENT (normally from tcpip_adapter.c) ---- */
/* ESP_EVENT_DECLARE_BASE / ESP_EVENT_DEFINE_BASE produce a const char* */
const char *IP_EVENT = "IP_EVENT";

/* ---- Logging ---- */
/* The full SDK log component depends on newlib stdio and recursive locks.
   Keep logging available, but route it straight to the ROM UART helpers. */
static char esp_log_prefix_char(esp_log_level_t level) {
    switch (level) {
    case ESP_LOG_ERROR:   return 'E';
    case ESP_LOG_WARN:    return 'W';
    case ESP_LOG_INFO:    return 'I';
    case ESP_LOG_DEBUG:   return 'D';
    case ESP_LOG_VERBOSE: return 'V';
    default:              return 'N';
    }
}

static void esp_log_write_simple(esp_log_level_t level, const char *tag, const char *fmt, va_list ap) {
    (void)level;
    (void)tag;
    (void)fmt;
    (void)ap;
}

uint32_t esp_log_timestamp(void) {
    return 0;
}

uint32_t esp_log_early_timestamp(void) {
    return 0;
}

/* Wrapped ets_printf — complete no-op to avoid stack overflow in ppTask. */
int __wrap_ets_printf(const char *fmt, ...) {
    (void)fmt;
    return 0;
}

void esp_log_write(esp_log_level_t level, const char *tag, const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    esp_log_write_simple(level, tag, fmt, ap);
    va_end(ap);
}

void esp_early_log_write(esp_log_level_t level, const char *tag, const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    esp_log_write_simple(level, tag, fmt, ap);
    va_end(ap);
}

#ifdef CONFIG_LOG_SET_LEVEL
void esp_log_level_set(const char *tag, esp_log_level_t level) {
    (void)tag;
    (void)level;
}
#endif

putchar_like_t esp_log_set_putchar(putchar_like_t func) {
    (void)func;
    return (putchar_like_t)0;
}

/* ---- Diagnostic: read PS and VECBASE registers ---- */
unsigned int diag_read_ps(void) {
    unsigned int val;
    __asm__ volatile("rsr %0, ps" : "=a"(val));
    return val;
}
unsigned int diag_read_vecbase(void) {
    unsigned int val;
    __asm__ volatile("rsr %0, 231" : "=a"(val));
    return val;
}

void diag_trigger_nmi(void) {
    /* Write to INTSET to manually assert interrupt 14 (NMI on LX106).
       If the NMI handler fires, g_nmi_call_count increments. */
    __asm__ volatile("wsr %0, intset" :: "a"(1 << 14));
}

unsigned int diag_read_intenable(void) {
    unsigned int val;
    __asm__ volatile("rsr %0, intenable" : "=a"(val));
    return val;
}
unsigned int diag_read_interrupt(void) {
    unsigned int val;
    __asm__ volatile("rsr %0, interrupt" : "=a"(val));
    return val;
}

/* ---- WiFi init trace wrappers ---- */
/* Non-blocking: drop byte if UART FIFO full. Critical for WiFi timing. */
static void trace_char(char c) {
    volatile unsigned int *status = (volatile unsigned int *)(0x60000000 + 0x1c);
    volatile unsigned int *fifo = (volatile unsigned int *)0x60000000;
    if ((((*status) >> 16) & 0xffu) < 126u) {
        *fifo = (unsigned int)c;
    }
}

static void trace_hex_nibble(unsigned int value) {
    value &= 0xFu;
    trace_char((char)(value < 10u ? ('0' + value) : ('A' + (value - 10u))));
}

static void trace_hex8(unsigned int value) {
    trace_hex_nibble(value >> 4);
    trace_hex_nibble(value);
}

static void trace_hex32(unsigned int value) {
    for (int shift = 28; shift >= 0; shift -= 4) {
        trace_hex_nibble(value >> (unsigned int)shift);
    }
}

static int trace_is_dram_ptr(unsigned int value) {
    return value >= 0x3FFE0000u && value < 0x40000000u && (value & 3u) == 0u;
}

extern unsigned char wDevCtrl[];
extern unsigned char g_wifi_menuconfig[];

static unsigned int trace_load_u32(const unsigned char *ptr) {
    return *(volatile const unsigned int *)ptr;
}

static void trace_wdev_txnode_state(char open0, char open1, char close0, char close1) {
    unsigned int head = *(volatile unsigned int *)(wDevCtrl + 24);

    trace_char(open0);
    trace_char(open1);
    trace_hex8((unsigned int)wDevCtrl[0]);
    trace_char('/');
    trace_hex32(head);

    for (unsigned int step = 0; step < 3u; step++) {
        if (head == 0u) {
            trace_char('/');
            trace_char('Z');
            break;
        }

        if (!trace_is_dram_ptr(head)) {
            trace_char('/');
            trace_char('I');
            break;
        }

        head = *(volatile unsigned int *)(head + 8u);
        trace_char('/');
        trace_hex32(head);
    }

    trace_char(close0);
    trace_char(close1);
}

static void trace_wifi_menuconfig_state(char open0, char open1, char close0, char close1) {
    trace_char(open0);
    trace_char(open1);
    trace_hex8((unsigned int)g_wifi_menuconfig[0]);
    trace_char('/');
    trace_hex8((unsigned int)g_wifi_menuconfig[1]);
    trace_char('/');
    trace_hex8((unsigned int)g_wifi_menuconfig[2]);
    trace_char('/');
    trace_hex8((unsigned int)g_wifi_menuconfig[3]);
    trace_char('/');
    trace_hex8((unsigned int)g_wifi_menuconfig[17]);
    trace_char('/');
    trace_hex32(trace_load_u32(g_wifi_menuconfig + 4));
    trace_char('/');
    trace_hex32(trace_load_u32(g_wifi_menuconfig + 8));
    trace_char('/');
    trace_hex32(trace_load_u32(g_wifi_menuconfig + 12));
    trace_char(close0);
    trace_char(close1);
}

typedef struct {
    void *timer;
    void *fn;
    void *arg;
} trace_timer_entry_t;

static trace_timer_entry_t g_trace_timer_entries[16];
unsigned int g_timer_last_setfn_timer = 0;
unsigned int g_timer_last_setfn_fn = 0;
unsigned int g_timer_last_setfn_arg = 0;
unsigned int g_timer_last_arm_timer = 0;
unsigned int g_timer_last_arm_fn = 0;
unsigned int g_timer_last_arm_arg = 0;
unsigned int g_timer_last_arm_msec = 0;
unsigned int g_timer_last_arm_repeat = 0;

static trace_timer_entry_t *trace_timer_find_existing(void *timer) {
    int i;
    for (i = 0; i < 16; i++) {
        if (g_trace_timer_entries[i].timer == timer) {
            return &g_trace_timer_entries[i];
        }
    }
    return (trace_timer_entry_t *)0;
}

static trace_timer_entry_t *trace_timer_find_or_alloc(void *timer) {
    trace_timer_entry_t *existing = trace_timer_find_existing(timer);
    int i;

    if (existing != (trace_timer_entry_t *)0) {
        return existing;
    }

    for (i = 0; i < 16; i++) {
        if (g_trace_timer_entries[i].timer == (void *)0) {
            g_trace_timer_entries[i].timer = timer;
            return &g_trace_timer_entries[i];
        }
    }

    g_trace_timer_entries[0].timer = timer;
    return &g_trace_timer_entries[0];
}

extern int __real_phy_get_bb_evm(void);
int __wrap_phy_get_bb_evm(void) {
    trace_char('1');
    int ret = __real_phy_get_bb_evm();
    trace_char('2');
    return ret;
}

extern void __real_pm_set_the_time_of_phy_init(void);
void __wrap_pm_set_the_time_of_phy_init(void) {
    trace_char('3');
    __real_pm_set_the_time_of_phy_init();
    trace_char('4');
}

extern int __real_misc_nvs_init(void);
int __wrap_misc_nvs_init(void) {
    trace_char('5');
    int ret = __real_misc_nvs_init();
    trace_char('6');
    return ret;
}

extern int __real_wifi_nvs_init(void);
int __wrap_wifi_nvs_init(void) {
    trace_char('7');
    int ret = __real_wifi_nvs_init();
    trace_char('8');
    return ret;
}

extern void __real_fix_cache_bug(void);
void __wrap_fix_cache_bug(void) {
    trace_char('{');
    __real_fix_cache_bug();
    trace_char('}');
}

extern void __real_set_crystal_uart(void);
void __wrap_set_crystal_uart(void) {
    trace_char('[');
    __real_set_crystal_uart();
    trace_char(']');
}

extern int __real_mac_init(void);
int __wrap_mac_init(void) {
    trace_char('M');
    int ret = __real_mac_init();
    trace_char('m');
    return ret;
}

extern void __real_esp_wifi_set_rx_pbuf_mem_type(int type);
void __wrap_esp_wifi_set_rx_pbuf_mem_type(int type) {
    trace_char('P');
    __real_esp_wifi_set_rx_pbuf_mem_type(type);
    trace_char('p');
}

extern esp_err_t __real_esp_wifi_init_internal(const void *config);
esp_err_t __wrap_esp_wifi_init_internal(const void *config) {
    trace_char('I');
    esp_err_t ret = __real_esp_wifi_init_internal(config);
    trace_char('i');
    return ret;
}

extern esp_err_t __real_esp_read_mac(unsigned char *mac, int type);
esp_err_t __wrap_esp_read_mac(unsigned char *mac, int type) {
    trace_char('R');
    esp_err_t ret = __real_esp_read_mac(mac, type);
    trace_char('r');
    return ret;
}

extern esp_err_t __real_esp_base_mac_addr_get(unsigned char *mac);
esp_err_t __wrap_esp_base_mac_addr_get(unsigned char *mac) {
    trace_char('B');
    esp_err_t ret = __real_esp_base_mac_addr_get(mac);
    trace_char('b');
    return ret;
}

extern esp_err_t __real_esp_efuse_mac_get_default(unsigned char *mac);
esp_err_t __wrap_esp_efuse_mac_get_default(unsigned char *mac) {
    trace_char('E');
    esp_err_t ret = __real_esp_efuse_mac_get_default(mac);
    trace_char('e');
    return ret;
}

extern void __real_esp_phy_load_cal_and_init(int module);
void __wrap_esp_phy_load_cal_and_init(int module) {
    trace_char('L');
    __real_esp_phy_load_cal_and_init(module);
    trace_char('l');
}

extern void __real_phy_enable_agc(void);
void __wrap_phy_enable_agc(void) {
    trace_char('!');
    trace_char('a');
    __real_phy_enable_agc();
    trace_char('!');
    trace_char('A');
}

extern void __real_ieee80211_phy_init(unsigned int value);
void __wrap_ieee80211_phy_init(unsigned int value) {
    trace_char('!');
    trace_char('b');
    __real_ieee80211_phy_init(value);
    trace_char('!');
    trace_char('B');
}

extern void __real_lmacInit(void);
void __wrap_lmacInit(void) {
    trace_char('!');
    trace_char('c');
    __real_lmacInit();
    trace_char('!');
    trace_char('C');
}

extern void __real_wDev_Initialize(void);
void __wrap_wDev_Initialize(void) {
    trace_char('!');
    trace_char('d');
    __real_wDev_Initialize();
    trace_char('!');
    trace_char('D');
    trace_wdev_txnode_state('!', 'x', '!', 'X');
    trace_wifi_menuconfig_state('!', 'm', '!', 'M');
}

extern void __real_pp_attach(void);
void __wrap_pp_attach(void) {
    trace_char('!');
    trace_char('e');
    __real_pp_attach();
    trace_char('!');
    trace_char('E');
}

extern void __real_ieee80211_ifattach(void *ic);
void __wrap_ieee80211_ifattach(void *ic) {
    trace_char('!');
    trace_char('f');
    __real_ieee80211_ifattach(ic);
    trace_char('!');
    trace_char('F');
}

extern void __real_pm_attach(void);
void __wrap_pm_attach(void) {
    trace_char('!');
    trace_char('g');
    __real_pm_attach();
    trace_char('!');
    trace_char('G');
}

extern uint16_t __real_wifi_nvs_get_sta_listen_interval(void);
uint16_t __wrap_wifi_nvs_get_sta_listen_interval(void) {
    trace_char('%');
    trace_char('n');
    uint16_t ret = __real_wifi_nvs_get_sta_listen_interval();
    trace_char('%');
    trace_char('N');
    return ret;
}

extern int __real_scan_start(void *p1, unsigned int p2, void *p3, void *p4);
int __wrap_scan_start(void *p1, unsigned int p2, void *p3, void *p4) {
    trace_char('^');
    trace_char('s');
    int ret = __real_scan_start(p1, p2, p3, p4);
    trace_char(ret == 0 ? '0' : (ret == 1 ? '1' : 'x'));
    trace_char('^');
    trace_char('S');
    return ret;
}

extern int __real_pm_scan_start(void);
int __wrap_pm_scan_start(void) {
    trace_char('^');
    trace_char('p');
    int ret = __real_pm_scan_start();
    trace_char(ret ? '1' : '0');
    trace_char('^');
    trace_char('P');
    return ret;
}

extern void __real_chip_v6_set_chan(unsigned int chan);
void __wrap_chip_v6_set_chan(unsigned int chan) {
    trace_char('^');
    trace_char('c');
    trace_char('0' + (chan & 0xF));
    __real_chip_v6_set_chan(chan);
    trace_char('^');
    trace_char('C');
}

extern void __real_pm_reset_param(void);
void __wrap_pm_reset_param(void) {
    trace_char('%');
    trace_char('p');
    __real_pm_reset_param();
    trace_char('%');
    trace_char('P');
}

extern int __real_ppRegisterTxCallback(void (*cb)(void), uint8_t id);
int __wrap_ppRegisterTxCallback(void (*cb)(void), uint8_t id) {
    trace_char('%');
    trace_char('t');
    trace_char('0' + (id & 0x0F));
    int ret = __real_ppRegisterTxCallback(cb, id);
    trace_char('%');
    trace_char('T');
    return ret;
}

extern void __real_pm_goto_rf_off(void);
void __wrap_pm_goto_rf_off(void) {
    trace_char('!');
    trace_char('h');
    __real_pm_goto_rf_off();
    trace_char('!');
    trace_char('H');
}

extern void __real__xt_isr_mask(unsigned int mask);
void __wrap__xt_isr_mask(unsigned int mask) {
    trace_char('!');
    trace_char('i');
    __real__xt_isr_mask(mask);
    trace_char('!');
    trace_char('I');
}

extern void __real_cnx_attach(void *ic);
void __wrap_cnx_attach(void *ic) {
    trace_char('!');
    trace_char('j');
    __real_cnx_attach(ic);
    trace_char('!');
    trace_char('J');
}

extern void __real_wifi_timer_init(void);
void __wrap_wifi_timer_init(void) {
    trace_char('!');
    trace_char('k');
    __real_wifi_timer_init();
    trace_char('!');
    trace_char('K');
}

extern void __real_esf_buf_setup(void);
void __wrap_esf_buf_setup(void) {
    trace_char('#');
    trace_char('e');
    __real_esf_buf_setup();
    trace_char('#');
    trace_char('E');
}

extern void __real_os_timer_setfn(void *timer, void *fn, void *arg);
void __wrap_os_timer_setfn(void *timer, void *fn, void *arg) {
    trace_timer_entry_t *entry = trace_timer_find_or_alloc(timer);
    entry->timer = timer;
    entry->fn = fn;
    entry->arg = arg;
    g_timer_last_setfn_timer = (unsigned int)timer;
    g_timer_last_setfn_fn = (unsigned int)fn;
    g_timer_last_setfn_arg = (unsigned int)arg;
    trace_char('#');
    trace_char('s');
    __real_os_timer_setfn(timer, fn, arg);
    trace_char('#');
    trace_char('S');
}

typedef int esp_err_t;
typedef struct { int event_id; } system_event_t;
extern esp_err_t __real_esp_event_send(system_event_t *event);
esp_err_t __wrap_esp_event_send(system_event_t *event) {
    trace_char('V');
    trace_hex8(event ? (unsigned int)event->event_id : 0xFF);
    esp_err_t ret = __real_esp_event_send(event);
    trace_char('v');
    return ret;
}

extern void __real_os_timer_disarm(void *timer);
void __wrap_os_timer_disarm(void *timer) {
    trace_char('$');
    trace_char('a');
    __real_os_timer_disarm(timer);
    trace_char('$');
    trace_char('A');
}

extern void __real_os_timer_arm(void *timer, unsigned int msec, unsigned int repeat_flag);
void __wrap_os_timer_arm(void *timer, unsigned int msec, unsigned int repeat_flag) {
    trace_timer_entry_t *entry = trace_timer_find_existing(timer);
    g_timer_last_arm_timer = (unsigned int)timer;
    g_timer_last_arm_fn = entry != (trace_timer_entry_t *)0 ? (unsigned int)entry->fn : 0u;
    g_timer_last_arm_arg = entry != (trace_timer_entry_t *)0 ? (unsigned int)entry->arg : 0u;
    g_timer_last_arm_msec = msec;
    g_timer_last_arm_repeat = repeat_flag;
    trace_char('$');
    trace_char('b');
    __real_os_timer_arm(timer, msec, repeat_flag);
    trace_char('$');
    trace_char('B');
}

extern int __real_pm_rf_is_closed(void);
int __wrap_pm_rf_is_closed(void) {
    trace_char('$');
    trace_char('c');
    int ret = __real_pm_rf_is_closed();
    trace_char(ret ? '1' : '0');
    trace_char('$');
    trace_char('C');
    return ret;
}

extern void __real_noise_check_loop(unsigned int enable);
void __wrap_noise_check_loop(unsigned int enable) {
    trace_char('$');
    trace_char('d');
    __real_noise_check_loop(enable);
    trace_char('$');
    trace_char('D');
}

extern int __real_RxNodeNum(void);
int __wrap_RxNodeNum(void) {
    trace_char('$');
    trace_char('e');
    int ret = __real_RxNodeNum();
    trace_char('$');
    trace_char('E');
    trace_wdev_txnode_state('$', 'x', '$', 'X');
    return ret;
}

extern int __real_TxNodeNum(void);
int __wrap_TxNodeNum(void) {
    trace_char('$');
    trace_char('f');
    int ret = __real_TxNodeNum();
    trace_char('$');
    trace_char('F');
    return ret;
}

extern int __real_read_hw_noisefloor(void);
int __wrap_read_hw_noisefloor(void) {
    trace_char('$');
    trace_char('g');
    int ret = __real_read_hw_noisefloor();
    trace_char('$');
    trace_char('G');
    return ret;
}

extern void __real_lmac_check_ac(void);
void __wrap_lmac_check_ac(void) {
    trace_char('$');
    trace_char('h');
    __real_lmac_check_ac();
    trace_char('$');
    trace_char('H');
}

extern esp_err_t __real_esp_phy_rf_init(const void *init_data, int mode, void *calibration_data, int module);
esp_err_t __wrap_esp_phy_rf_init(const void *init_data, int mode, void *calibration_data, int module) {
    trace_char('F');
    esp_err_t ret = __real_esp_phy_rf_init(init_data, mode, calibration_data, module);
    trace_char('f');
    return ret;
}

extern int __real_register_chipv6_phy(unsigned char *buf);
int __wrap_register_chipv6_phy(unsigned char *buf) {
    trace_char('C');
    int ret = __real_register_chipv6_phy(buf);
    trace_char('c');
    return ret;
}

extern void __real_sleep_reset_analog_rtcreg_8266(void);
void __wrap_sleep_reset_analog_rtcreg_8266(void) {
    trace_char('S');
    __real_sleep_reset_analog_rtcreg_8266();
    trace_char('s');
}

extern void __real_pm_wait4wakeup(int mode);
void __wrap_pm_wait4wakeup(int mode) {
    trace_char('W');
    __real_pm_wait4wakeup(mode);
    trace_char('w');
}

extern int __real_nvs_open(const char *name, int open_mode, int *out);
int __wrap_nvs_open(const char *name, int open_mode, int *out) {
    trace_char('N');
    int ret = __real_nvs_open(name, open_mode, out);
    trace_char('n');
    return ret;
}

extern int __real_nvs_get_blob(int handle, const char *key, void *value, unsigned int *length);
int __wrap_nvs_get_blob(int handle, const char *key, void *value, unsigned int *length) {
    trace_char('G');
    int ret = __real_nvs_get_blob(handle, key, value, length);
    trace_char('g');
    return ret;
}

extern void __real_uart_tx_wait_idle(unsigned int uart_no);
void __wrap_uart_tx_wait_idle(unsigned int uart_no) {
    trace_char('U');
    __real_uart_tx_wait_idle(uart_no);
    trace_char('u');
}

/* ---- NVS stubs (WiFi blob uses NVS for calibration & config storage) ---- */

typedef int32_t nvs_handle;
enum {
    FAKE_NVS_HANDLE_PHY = 1,
    FAKE_NVS_HANDLE_BACKUP_MAC = 2,
};

typedef struct {
    uint8_t backup_mac_data[16];
    unsigned int backup_mac_len;
    uint8_t backup_mac_valid;
    uint8_t phy_cal_data[128];
    unsigned int phy_cal_data_len;
    uint8_t phy_cal_data_valid;
    uint8_t phy_dc_table[125 * sizeof(uint32_t)];
    unsigned int phy_dc_table_len;
    uint8_t phy_dc_table_valid;
} fake_nvs_store_t;

static fake_nvs_store_t s_fake_nvs;

#define ESP_ERR_NVS_NOT_FOUND 0x1102
#define ESP_ERR_NVS_NOT_INITIALIZED 0x1105

int nvs_flash_init(void) { return ESP_OK; }
int nvs_flash_erase(void) {
    memset(&s_fake_nvs, 0, sizeof(s_fake_nvs));
    return ESP_OK;
}

int nvs_open(const char *name, int open_mode, nvs_handle *out) {
    (void)open_mode;
    if (!out || !name) {
        return 0x102; /* ESP_ERR_INVALID_ARG */
    }
    if (strcmp(name, "phy") == 0) {
        *out = FAKE_NVS_HANDLE_PHY;
        return ESP_OK;
    }
    if (strcmp(name, "backup_mac") == 0) {
        *out = FAKE_NVS_HANDLE_BACKUP_MAC;
        return ESP_OK;
    }
    *out = 0;
    return ESP_OK;
}
void nvs_close(nvs_handle h) { (void)h; }
int nvs_commit(nvs_handle h) { (void)h; return ESP_OK; }
int nvs_get_i8(nvs_handle h, const char *k, int8_t *v)   { (void)h; (void)k; (void)v; return ESP_ERR_NVS_NOT_FOUND; }
int nvs_set_i8(nvs_handle h, const char *k, int8_t v)    { (void)h; (void)k; (void)v; return ESP_OK; }
int nvs_get_u8(nvs_handle h, const char *k, uint8_t *v)  { (void)h; (void)k; (void)v; return ESP_ERR_NVS_NOT_FOUND; }
int nvs_set_u8(nvs_handle h, const char *k, uint8_t v)   { (void)h; (void)k; (void)v; return ESP_OK; }
int nvs_get_u16(nvs_handle h, const char *k, uint16_t *v){ (void)h; (void)k; (void)v; return ESP_ERR_NVS_NOT_FOUND; }
int nvs_set_u16(nvs_handle h, const char *k, uint16_t v) { (void)h; (void)k; (void)v; return ESP_OK; }
int nvs_get_blob(nvs_handle h, const char *k, void *v, unsigned int *l) {
    if (!k || !v || !l) {
        return 0x102; /* ESP_ERR_INVALID_ARG */
    }
    if (h == FAKE_NVS_HANDLE_PHY && strcmp(k, "cal_data") == 0) {
        if (!s_fake_nvs.phy_cal_data_valid) {
            return ESP_ERR_NVS_NOT_FOUND;
        }
        if (*l < s_fake_nvs.phy_cal_data_len) {
            return 0x104; /* ESP_ERR_INVALID_SIZE */
        }
        memcpy(v, s_fake_nvs.phy_cal_data, s_fake_nvs.phy_cal_data_len);
        *l = s_fake_nvs.phy_cal_data_len;
        return ESP_OK;
    }
    if (h == FAKE_NVS_HANDLE_PHY && strcmp(k, "dc_table") == 0) {
        if (!s_fake_nvs.phy_dc_table_valid) {
            return ESP_ERR_NVS_NOT_FOUND;
        }
        if (*l < s_fake_nvs.phy_dc_table_len) {
            return 0x104; /* ESP_ERR_INVALID_SIZE */
        }
        memcpy(v, s_fake_nvs.phy_dc_table, s_fake_nvs.phy_dc_table_len);
        *l = s_fake_nvs.phy_dc_table_len;
        return ESP_OK;
    }
    if (h == FAKE_NVS_HANDLE_BACKUP_MAC && strcmp(k, "backup_mac_data") == 0) {
        if (!s_fake_nvs.backup_mac_valid) {
            return ESP_ERR_NVS_NOT_FOUND;
        }
        if (*l < s_fake_nvs.backup_mac_len) {
            return 0x104; /* ESP_ERR_INVALID_SIZE */
        }
        memcpy(v, s_fake_nvs.backup_mac_data, s_fake_nvs.backup_mac_len);
        *l = s_fake_nvs.backup_mac_len;
        return ESP_OK;
    }
    return ESP_ERR_NVS_NOT_FOUND;
}
int nvs_set_blob(nvs_handle h, const char *k, const void *v, unsigned int l) {
    if (!k || !v) {
        return 0x102; /* ESP_ERR_INVALID_ARG */
    }
    if (h == FAKE_NVS_HANDLE_PHY && strcmp(k, "cal_data") == 0 && l <= sizeof(s_fake_nvs.phy_cal_data)) {
        memcpy(s_fake_nvs.phy_cal_data, v, l);
        s_fake_nvs.phy_cal_data_len = l;
        s_fake_nvs.phy_cal_data_valid = 1;
        return ESP_OK;
    }
    if (h == FAKE_NVS_HANDLE_PHY && strcmp(k, "dc_table") == 0 && l <= sizeof(s_fake_nvs.phy_dc_table)) {
        memcpy(s_fake_nvs.phy_dc_table, v, l);
        s_fake_nvs.phy_dc_table_len = l;
        s_fake_nvs.phy_dc_table_valid = 1;
        return ESP_OK;
    }
    if (h == FAKE_NVS_HANDLE_BACKUP_MAC && strcmp(k, "backup_mac_data") == 0 && l <= sizeof(s_fake_nvs.backup_mac_data)) {
        memcpy(s_fake_nvs.backup_mac_data, v, l);
        s_fake_nvs.backup_mac_len = l;
        s_fake_nvs.backup_mac_valid = 1;
        return ESP_OK;
    }
    return 0x102; /* ESP_ERR_INVALID_ARG */
}

/* ---- FreeRTOS idle task (referenced by esp_sleep.c) ---- */
extern unsigned int compat_prvGetExpectedIdleTime(void);
unsigned int prvGetExpectedIdleTime(void) { return compat_prvGetExpectedIdleTime(); }

/* ---- lwIP bridge shims ---- */
void *pvTaskGetThreadLocalStoragePointer(TaskHandle_t xTaskToQuery, BaseType_t xIndex) {
    return pvTaskGetThreadLocalStorageBufferPointer(xTaskToQuery, xIndex);
}

void esp_vfs_lwip_sockets_register(void) {}

/* ---- pthread stubs (newlib/lwIP reference a tiny subset) ---- */
typedef void (*pthread_destructor_t)(void *);
#define STUB_PTHREAD_KEYS 3
static pthread_destructor_t s_stub_pthread_destructors[STUB_PTHREAD_KEYS];
static pthread_key_t s_stub_next_pthread_key = 1;

static BaseType_t stub_pthread_key_index(pthread_key_t key) {
    if (key <= 0 || key > STUB_PTHREAD_KEYS) {
        return -1;
    }
    return (BaseType_t)(key - 1);
}

int pthread_setcancelstate(int state, int *oldstate) {
    (void)state; (void)oldstate; return 0;
}
int pthread_key_create(pthread_key_t *key, pthread_destructor_t destructor) {
    if (key == NULL || s_stub_next_pthread_key > STUB_PTHREAD_KEYS) {
        return ENOMEM;
    }
    *key = s_stub_next_pthread_key;
    s_stub_pthread_destructors[s_stub_next_pthread_key - 1] = destructor;
    s_stub_next_pthread_key++;
    return 0;
}
int pthread_key_delete(pthread_key_t key) {
    BaseType_t index = stub_pthread_key_index(key);
    if (index < 0) {
        return EINVAL;
    }
    s_stub_pthread_destructors[index] = NULL;
    return 0;
}
void *pthread_getspecific(pthread_key_t key) {
    BaseType_t index = stub_pthread_key_index(key);
    if (index < 0) {
        return NULL;
    }
    return pvTaskGetThreadLocalStoragePointer(NULL, index);
}
int pthread_setspecific(pthread_key_t key, const void *value) {
    BaseType_t index = stub_pthread_key_index(key);
    if (index < 0) {
        return EINVAL;
    }
    vTaskSetThreadLocalStoragePointer(NULL, index, (void *)value);
    return 0;
}
int esp_pthread_init(void) { return 0; }

/* ---- xtbsp_clock_freq_hz (FreeRTOS port.c, normally a macro) ---- */
#ifdef xtbsp_clock_freq_hz
#undef xtbsp_clock_freq_hz
#endif
unsigned int xtbsp_clock_freq_hz(void) { return 80000000; }

/* ---- Interrupt handlers (override xtensa-lx-rt defaults) ---- */
/* The xtensa-lx-rt default handlers are no-ops. Timer (CCOMPARE) interrupts
   are NOT clearable via INTCLEAR and require resetting CCOUNT/CCOMPARE.
   Without a proper handler, the timer re-fires infinitely. */
extern void xPortSysTickHandle(void *arg);
extern void SoftIsrHdl(void *arg);

__attribute__((section(".rwtext")))
void __ccompare_interrupt(void *frame) {
    xPortSysTickHandle(frame);
}

/* Software interrupt (used by PendSV for deferred context switches) */
__attribute__((section(".rwtext")))
void __soft_interrupt(void *frame) {
    SoftIsrHdl(frame);
}

/* ---- panicHandler (referenced by xtensa_vectors.S) ---- */
extern void uart_tx_wait_idle(unsigned int);
void panicHandler(void *frame) {
    (void)frame;
    /* Infinite loop with periodic UART output so we know we're here */
    volatile unsigned int *uart_fifo = (volatile unsigned int *)0x60000000;
    for (;;) {
        *uart_fifo = '!';
        for (volatile int i = 0; i < 800000; i++) {}
    }
}

/* ---- C malloc/free routing ---- */
/* Route newlib's malloc/free through our Rust heap (pvPortMalloc/vPortFree) */
/* so that blob code calling C malloc works correctly. */
void *_malloc_r(struct _reent *reent, size_t size) {
    (void)reent;
    return pvPortMalloc(size);
}

void _free_r(struct _reent *reent, void *ptr) {
    (void)reent;
    vPortFree(ptr);
}

extern void *esp8266_hal_realloc(void *ptr, unsigned int size);

void *_realloc_r(struct _reent *reent, void *ptr, size_t size) {
    (void)reent;
    return esp8266_hal_realloc(ptr, size);
}

void *_calloc_r(struct _reent *reent, size_t count, size_t size) {
    (void)reent;
    return pvPortCalloc(count, size);
}

void *_sbrk_r(struct _reent *reent, ptrdiff_t incr) {
    (void)reent; (void)incr;
    return (void *)-1; /* never used — malloc routed through pvPortMalloc */
}

/* ---- pm_wait4wakeup override ---- */
/* The blob's pm_wait4wakeup polls 0x60000728 bits[1:0] for wakeup.
   In our bare-metal boot path the RTC sleep/wake cycle may not complete
   because the RTC clock source isn't configured. Override with a brief
   delay that gives the hardware time to settle. */
void pm_wait4wakeup(int mode) {
    (void)mode;
    /* Brief delay (~10ms at 80MHz) to let analog settle */
    for (volatile int i = 0; i < 800000; i++) {}
}

/* ---- UART wait stubs ---- */
/* The SDK's uart_tx_wait_idle polls TX FIFO which can hang if UART clock
   is misconfigured after rtc_init_clk. Override with a brief delay. */
void uart_tx_wait_idle(unsigned int uart_no) {
    (void)uart_no;
    /* Brief busy-wait (~1ms at 80MHz) to let any pending TX drain */
    for (volatile int i = 0; i < 80000; i++) {}
}

/* ---- Error check (used by VFS/newlib code) ---- */
void _esp_error_check_failed(int rc, const char *file, int line, const char *func, const char *expr) {
    (void)rc; (void)file; (void)line; (void)func; (void)expr;
    while(1) {} /* hang on error */
}

/* ---- Atomic builtins (ESP8266 is single-core, no HW atomics) ---- */
uint32_t __atomic_load_4(const volatile void *p, int mo) {
    (void)mo; return *(const volatile uint32_t *)p;
}
void __atomic_store_1(volatile void *p, uint8_t v, int mo) {
    (void)mo; *(volatile uint8_t *)p = v;
}
void __atomic_store_4(volatile void *p, uint32_t v, int mo) {
    (void)mo; *(volatile uint32_t *)p = v;
}
"#,
    )
    .expect("failed to write sdk_stubs.c");
    build.file(&stubs_c);

    // --- wpa_supplicant ---
    // Defines that match the SDK CMakeLists.txt.
    let wpa_defines = [
        "__ets__",
        "ESP_SUPPLICANT",
        "IEEE8021X_EAPOL",
        "EAP_PEER_METHOD",
        "EAP_MSCHAPv2",
        "EAP_TTLS",
        "EAP_TLS",
        "EAP_PEAP",
        "USE_WPA2_TASK",
        "CONFIG_WPS2",
        "CONFIG_WPS_PIN",
        "USE_WPS_TASK",
        "ESPRESSIF_USE",
        "ESP8266_WORKAROUND",
        "CONFIG_ECC",
        "CONFIG_IEEE80211W",
        "CONFIG_WPA3_SAE",
        "CONFIG_DPP",
        "CONFIG_WNM",
    ];
    // We compile wpa_supplicant as a separate cc::Build because it needs
    // different defines.
    let mut wpa_build = cc::Build::new();
    init_cc_build(&mut wpa_build, gcc, include_paths);
    wpa_build.include(comp("components/wpa_supplicant/src/crypto"));
    wpa_build
        .define("MBEDTLS_CONFIG_FILE", "\"mbedtls/esp_config.h\"")
        .define("CONFIG_SSL_USING_MBEDTLS", None);
    for def in &wpa_defines {
        wpa_build.define(def, None);
    }

    let wpa_srcs = [
        "components/wpa_supplicant/port/os_xtensa.c",
        "components/wpa_supplicant/src/ap/ap_config.c",
        "components/wpa_supplicant/src/ap/ieee802_1x.c",
        "components/wpa_supplicant/src/ap/wpa_auth.c",
        "components/wpa_supplicant/src/ap/wpa_auth_ie.c",
        "components/wpa_supplicant/src/common/sae.c",
        "components/wpa_supplicant/src/common/wpa_common.c",
        "components/wpa_supplicant/src/common/dpp.c",
        "components/wpa_supplicant/src/utils/bitfield.c",
        "components/wpa_supplicant/src/crypto/aes-ctr.c",
        "components/wpa_supplicant/src/crypto/aes-siv.c",
        "components/wpa_supplicant/src/crypto/sha256-kdf.c",
        "components/wpa_supplicant/src/crypto/aes-cbc.c",
        "components/wpa_supplicant/src/crypto/aes-ccm.c",
        "components/wpa_supplicant/src/crypto/aes-internal-dec.c",
        "components/wpa_supplicant/src/crypto/aes-internal-enc.c",
        "components/wpa_supplicant/src/crypto/aes-internal.c",
        "components/wpa_supplicant/src/crypto/aes-omac1.c",
        "components/wpa_supplicant/src/crypto/aes-unwrap.c",
        "components/wpa_supplicant/src/crypto/aes-wrap.c",
        "components/wpa_supplicant/src/crypto/bignum.c",
        "components/wpa_supplicant/src/crypto/ccmp.c",
        "components/wpa_supplicant/src/crypto/crypto_mbedtls.c",
        "components/wpa_supplicant/src/crypto/crypto_mbedtls-bignum.c",
        "components/wpa_supplicant/src/crypto/crypto_mbedtls-ec.c",
        "components/wpa_supplicant/src/crypto/crypto_ops.c",
        "components/wpa_supplicant/src/crypto/crypto_internal-cipher.c",
        "components/wpa_supplicant/src/crypto/crypto_internal-modexp.c",
        "components/wpa_supplicant/src/crypto/crypto_internal-rsa.c",
        "components/wpa_supplicant/src/crypto/crypto_mbedtls-rsa.c",
        "components/wpa_supplicant/src/crypto/crypto_internal.c",
        "components/wpa_supplicant/src/crypto/des-internal.c",
        "components/wpa_supplicant/src/crypto/dh_group5.c",
        "components/wpa_supplicant/src/crypto/dh_groups.c",
        "components/wpa_supplicant/src/crypto/md4-internal.c",
        "components/wpa_supplicant/src/crypto/md5-internal.c",
        "components/wpa_supplicant/src/crypto/md5.c",
        "components/wpa_supplicant/src/crypto/ms_funcs.c",
        "components/wpa_supplicant/src/crypto/rc4.c",
        "components/wpa_supplicant/src/crypto/sha1-internal.c",
        "components/wpa_supplicant/src/crypto/sha1-pbkdf2.c",
        "components/wpa_supplicant/src/crypto/sha1.c",
        "components/wpa_supplicant/src/crypto/sha256-internal.c",
        "components/wpa_supplicant/src/crypto/sha256.c",
        "components/wpa_supplicant/src/crypto/sha1-tlsprf.c",
        "components/wpa_supplicant/src/crypto/sha256-tlsprf.c",
        "components/wpa_supplicant/src/crypto/sha384-tlsprf.c",
        "components/wpa_supplicant/src/crypto/sha256-prf.c",
        "components/wpa_supplicant/src/crypto/tls_mbedtls.c",
        "components/wpa_supplicant/src/eap_peer/chap.c",
        "components/wpa_supplicant/src/eap_peer/eap.c",
        "components/wpa_supplicant/src/eap_peer/eap_common.c",
        "components/wpa_supplicant/src/eap_peer/eap_mschapv2.c",
        "components/wpa_supplicant/src/eap_peer/eap_peap.c",
        "components/wpa_supplicant/src/eap_peer/eap_peap_common.c",
        "components/wpa_supplicant/src/eap_peer/eap_tls.c",
        "components/wpa_supplicant/src/eap_peer/eap_tls_common.c",
        "components/wpa_supplicant/src/eap_peer/eap_ttls.c",
        "components/wpa_supplicant/src/eap_peer/mschapv2.c",
        "components/wpa_supplicant/src/esp_supplicant/esp_hostap.c",
        "components/wpa_supplicant/src/esp_supplicant/esp_wpa2.c",
        "components/wpa_supplicant/src/esp_supplicant/esp_wpa_main.c",
        "components/wpa_supplicant/src/esp_supplicant/esp_wpas_glue.c",
        "components/wpa_supplicant/src/esp_supplicant/esp_wps.c",
        "components/wpa_supplicant/src/esp_supplicant/esp_wpa3.c",
        "components/wpa_supplicant/src/rsn_supp/pmksa_cache.c",
        "components/wpa_supplicant/src/rsn_supp/wpa.c",
        "components/wpa_supplicant/src/rsn_supp/wpa_ie.c",
        "components/wpa_supplicant/src/utils/base64.c",
        "components/wpa_supplicant/src/utils/common.c",
        "components/wpa_supplicant/src/utils/ext_password.c",
        "components/wpa_supplicant/src/utils/uuid.c",
        "components/wpa_supplicant/src/utils/wpabuf.c",
        "components/wpa_supplicant/src/utils/wpa_debug.c",
        "components/wpa_supplicant/src/utils/json.c",
        "components/wpa_supplicant/src/wps/wps.c",
        "components/wpa_supplicant/src/wps/wps_attr_build.c",
        "components/wpa_supplicant/src/wps/wps_attr_parse.c",
        "components/wpa_supplicant/src/wps/wps_attr_process.c",
        "components/wpa_supplicant/src/wps/wps_common.c",
        "components/wpa_supplicant/src/wps/wps_dev_attr.c",
        "components/wpa_supplicant/src/wps/wps_enrollee.c",
        "components/wpa_supplicant/src/wps/wps_registrar.c",
        "components/wpa_supplicant/src/wps/wps_validate.c",
    ];
    for src in &wpa_srcs {
        match *src {
            "components/wpa_supplicant/src/eap_peer/eap.c"
                if env::var_os(EAP_TRACE_ENV).is_some() =>
            {
                wpa_build.file(patch_eap_core_trace_source(&comp(src), out_dir));
            }
            "components/wpa_supplicant/src/eap_peer/eap_peap.c"
                if env::var_os(EAP_TRACE_ENV).is_some() =>
            {
                wpa_build.file(patch_eap_peap_trace_source(&comp(src), out_dir));
            }
            "components/wpa_supplicant/src/eap_peer/eap_tls_common.c"
                if env::var_os(EAP_TRACE_ENV).is_some() =>
            {
                wpa_build.file(patch_eap_tls_common_trace_source(&comp(src), out_dir));
            }
            "components/wpa_supplicant/src/eap_peer/eap_tls_common.c" => {
                wpa_build.file(patch_eap_tls_common_source(&comp(src), out_dir));
            }
            "components/wpa_supplicant/src/crypto/tls_mbedtls.c"
                if env::var_os(EAP_TRACE_ENV).is_some() =>
            {
                wpa_build.file(patch_tls_mbedtls_eap_trace_source(&comp(src), out_dir));
            }
            "components/wpa_supplicant/src/crypto/tls_mbedtls.c" => {
                wpa_build.file(patch_tls_mbedtls_source(&comp(src), out_dir));
            }
            "components/wpa_supplicant/src/crypto/aes-internal-enc.c" => {
                wpa_build.file(patch_aes_internal_user_source(&comp(src), out_dir));
            }
            "components/wpa_supplicant/src/crypto/aes-internal-dec.c" => {
                wpa_build.file(patch_aes_internal_user_source(&comp(src), out_dir));
            }
            "components/wpa_supplicant/src/crypto/aes-internal.c" => {
                wpa_build.file(patch_aes_internal_source(&comp(src), out_dir));
            }
            "components/wpa_supplicant/src/esp_supplicant/esp_wpa2.c"
                if env::var_os(EAP_TRACE_ENV).is_some() =>
            {
                wpa_build.file(patch_esp_wpa2_eap_trace_source(&comp(src), out_dir));
            }
            "components/wpa_supplicant/src/esp_supplicant/esp_wpa2.c" => {
                wpa_build.file(patch_esp_wpa2_source(&comp(src), out_dir));
            }
            "components/wpa_supplicant/src/esp_supplicant/esp_wps.c" => {
                wpa_build.file(patch_esp_wps_source(&comp(src), out_dir));
            }
            "components/wpa_supplicant/src/wps/wps_attr_build.c" => {
                wpa_build.file(patch_wps_attr_build_source(&comp(src), out_dir));
            }
            "components/wpa_supplicant/src/wps/wps_enrollee.c" => {
                if env::var_os(WPS_ENROLLEE_TRACE_ENV).is_some() {
                    wpa_build.file(patch_wps_enrollee_source(&comp(src), out_dir));
                } else {
                    wpa_build.file(comp(src));
                }
            }
            _ => {
                wpa_build.file(comp(src));
            }
        }
    }

    // Compile the main SDK archive.
    build.compile("esp8266_sdk");

    // Compile wpa_supplicant as a separate archive.
    wpa_build.compile("wpa_supplicant");

    // --- mbedTLS (compile separately with its own config) ---
    compile_mbedtls(sdk, include_paths, out_dir, gcc);
}

fn eap_trace_hook() -> &'static str {
    r#"
static void esp8266_hal_eap_trace_char(char c) {
    volatile unsigned int *status = (volatile unsigned int *)(0x60000000 + 0x1c);
    volatile unsigned int *fifo = (volatile unsigned int *)0x60000000;
    if ((((*status) >> 16) & 0xffu) < 126u) {
        *fifo = (unsigned int)c;
    }
}
static void esp8266_hal_eap_trace_nibble(unsigned int value) {
    value &= 0xFu;
    esp8266_hal_eap_trace_char((char)(value < 10u ? ('0' + value) : ('A' + (value - 10u))));
}
static void esp8266_hal_eap_trace_hex8(unsigned int value) {
    esp8266_hal_eap_trace_nibble(value >> 4);
    esp8266_hal_eap_trace_nibble(value);
}
static void esp8266_hal_eap_trace_hex32(unsigned int value) {
    for (int shift = 28; shift >= 0; shift -= 4) {
        esp8266_hal_eap_trace_nibble(value >> (unsigned int)shift);
    }
}
static void esp8266_hal_eap_trace_code(char code) {
    esp8266_hal_eap_trace_char('~');
    esp8266_hal_eap_trace_char(code);
}
"#
}

fn patch_eap_core_trace_source(source: &Path, out_dir: &Path) -> PathBuf {
    let patched = out_dir.join("eap_core_trace.c");
    let source_text = fs::read_to_string(source).expect("failed to read eap.c");
    let patched_text = replace_required(
        &source_text,
        "#include \"supplicant_opt.h\"",
        &format!("#include \"supplicant_opt.h\"\n{}", eap_trace_hook()),
        "eap.c EAP trace hook",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tif (sm->m && sm->m->get_identity) {\n\t\tidentity = sm->m->get_identity(sm,\n\t\t\t\t\tsm->eap_method_priv,\n\t\t\t\t\t&identity_len);\n\t} else if (!encrypted && config->anonymous_identity) {",
        "\tif (sm->m && sm->m->get_identity) {\n\t\tidentity = sm->m->get_identity(sm,\n\t\t\t\t\tsm->eap_method_priv,\n\t\t\t\t\t&identity_len);\n\t\tesp8266_hal_eap_trace_code('j');\n\t\tesp8266_hal_eap_trace_hex32((unsigned int)identity_len);\n\t} else if (!encrypted && config->anonymous_identity) {",
        "eap.c identity method trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\t} else if (!encrypted && config->anonymous_identity) {\n\t\tidentity = config->anonymous_identity;\n\t\tidentity_len = config->anonymous_identity_len;\n\t} else {\n\t\tidentity = config->identity;\n\t\tidentity_len = config->identity_len;\n\t}",
        "\t} else if (!encrypted && config->anonymous_identity) {\n\t\tidentity = config->anonymous_identity;\n\t\tidentity_len = config->anonymous_identity_len;\n\t\tesp8266_hal_eap_trace_code('k');\n\t\tesp8266_hal_eap_trace_hex32((unsigned int)identity_len);\n\t} else {\n\t\tidentity = config->identity;\n\t\tidentity_len = config->identity_len;\n\t\tesp8266_hal_eap_trace_code('K');\n\t\tesp8266_hal_eap_trace_hex32((unsigned int)identity_len);\n\t}",
        "eap.c identity source trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tif (identity == NULL) {\n        wpa_printf(MSG_ERROR, \"EAP: Build Identity Resp-> identity was not available\\n\");\n\t\treturn NULL;\n\t}",
        "\tif (identity == NULL) {\n\t\tesp8266_hal_eap_trace_code('J');\n        wpa_printf(MSG_ERROR, \"EAP: Build Identity Resp-> identity was not available\\n\");\n\t\treturn NULL;\n\t}\n\tesp8266_hal_eap_trace_code('E');\n\tesp8266_hal_eap_trace_hex32((unsigned int)encrypted);\n\tesp8266_hal_eap_trace_hex32((unsigned int)identity_len);",
        "eap.c identity response length trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\t/* anonymous identity */\n\tif (g_wpa_anonymous_identity && g_wpa_anonymous_identity_len > 0) {",
        "\tesp8266_hal_eap_trace_code('c');\n\tesp8266_hal_eap_trace_hex32((unsigned int)g_wpa_anonymous_identity_len);\n\tesp8266_hal_eap_trace_hex32((unsigned int)g_wpa_username_len);\n\tesp8266_hal_eap_trace_hex32((unsigned int)g_wpa_password_len);\n\t/* anonymous identity */\n\tif (g_wpa_anonymous_identity && g_wpa_anonymous_identity_len > 0) {",
        "eap.c config globals trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tif (g_wpa_password && g_wpa_password_len) {\n\t\tsm->config.password_len = g_wpa_password_len;\n\t\tsm->config.password = (u8 *)os_zalloc(sm->config.password_len);\n\t\tif (sm->config.password == NULL)\n\t\t\treturn -2;\n\t\tos_memcpy(sm->config.password, g_wpa_password, sm->config.password_len);\n\t}\n\n\tif (g_wpa_new_password && g_wpa_new_password_len) {",
        "\tif (g_wpa_password && g_wpa_password_len) {\n\t\tsm->config.password_len = g_wpa_password_len;\n\t\tsm->config.password = (u8 *)os_zalloc(sm->config.password_len);\n\t\tif (sm->config.password == NULL)\n\t\t\treturn -2;\n\t\tos_memcpy(sm->config.password, g_wpa_password, sm->config.password_len);\n\t}\n\tesp8266_hal_eap_trace_code('C');\n\tesp8266_hal_eap_trace_hex32((unsigned int)sm->config.anonymous_identity_len);\n\tesp8266_hal_eap_trace_hex32((unsigned int)sm->config.identity_len);\n\tesp8266_hal_eap_trace_hex32((unsigned int)sm->config.password_len);\n\n\tif (g_wpa_new_password && g_wpa_new_password_len) {",
        "eap.c config copied trace",
    );
    fs::write(&patched, patched_text).expect("failed to write patched eap.c");
    patched
}

fn patch_esp_wpa2_eap_trace_source(source: &Path, out_dir: &Path) -> PathBuf {
    let patched = out_dir.join("esp_wpa2_eap_trace.c");
    let source_text = fs::read_to_string(source).expect("failed to read esp_wpa2.c");
    let patched_text = replace_required(
        &source_text,
        "#include \"esp_wifi_driver.h\"",
        &format!("#include \"esp_wifi_driver.h\"\n{}", eap_trace_hook()),
        "esp_wpa2.c EAP trace hook",
    );
    let patched_text = replace_required(
        &patched_text,
        "    type = *pos++;\n    if (type == EAP_TYPE_IDENTITY) {",
        "    type = *pos++;\n    esp8266_hal_eap_trace_code('R');\n    esp8266_hal_eap_trace_hex8(type);\n    if (type == EAP_TYPE_IDENTITY) {",
        "esp_wpa2.c request type trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "        resp = sm->m->process(sm, sm->eap_method_priv,\n                              &m_res, reqData);",
        "        esp8266_hal_eap_trace_code('p');\n        resp = sm->m->process(sm, sm->eap_method_priv,\n                              &m_res, reqData);",
        "esp_wpa2.c existing method process trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "        if (m == NULL) {\n            goto build_nak;\n        }",
        "        if (m == NULL) {\n            esp8266_hal_eap_trace_code('N');\n            goto build_nak;\n        }",
        "esp_wpa2.c method lookup failure trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "        sm->eap_method_priv = sm->m->init(sm);\n        if (sm->eap_method_priv == NULL) {",
        "        sm->eap_method_priv = sm->m->init(sm);\n        if (sm->eap_method_priv == NULL) {\n            esp8266_hal_eap_trace_code('I');",
        "esp_wpa2.c method init failure trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "        if (sm->m->process) {\n            resp = sm->m->process(sm, sm->eap_method_priv, &m_res, reqData);\n        }\n    }\n\n    if (sm->m->isKeyAvailable",
        "        if (sm->m->process) {\n            esp8266_hal_eap_trace_code('P');\n            resp = sm->m->process(sm, sm->eap_method_priv, &m_res, reqData);\n        }\n    }\n    esp8266_hal_eap_trace_code(resp ? '1' : '0');\n\n    if (sm->m->isKeyAvailable",
        "esp_wpa2.c response pointer trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "build_nak:\n    resp = (struct wpabuf *)eap_sm_build_nak(sm, type, ehdr->identifier);",
        "build_nak:\n    esp8266_hal_eap_trace_code('n');\n    resp = (struct wpabuf *)eap_sm_build_nak(sm, type, ehdr->identifier);",
        "esp_wpa2.c NAK trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "    if (resp == NULL) {\n        wpa_printf(MSG_ERROR, \"Response build fail, return.\");\n        return ESP_FAIL;\n    }\n    ret = eap_sm_send_eapol(sm, resp);",
        "    if (resp == NULL) {\n        esp8266_hal_eap_trace_code('Z');\n        wpa_printf(MSG_ERROR, \"Response build fail, return.\");\n        return ESP_FAIL;\n    }\n    esp8266_hal_eap_trace_code('S');\n    ret = eap_sm_send_eapol(sm, resp);\n    esp8266_hal_eap_trace_code(ret == ESP_OK ? 's' : 'F');",
        "esp_wpa2.c send trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "static void wpa2_sendto_wrapper(void *buffer, uint16_t len)\n{\n    esp_wifi_internal_tx(WIFI_IF_STA, buffer, len);\n}",
        "static void wpa2_sendto_wrapper(void *buffer, uint16_t len)\n{\n    int ret;\n    esp8266_hal_eap_trace_code('W');\n    esp8266_hal_eap_trace_hex32((unsigned int)len);\n    ret = esp_wifi_internal_tx(WIFI_IF_STA, buffer, len);\n    esp8266_hal_eap_trace_code(ret == ESP_OK ? 'w' : 'X');\n    esp8266_hal_eap_trace_hex32((unsigned int)ret);\n}",
        "esp_wpa2.c raw EAPOL tx trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "int eap_sm_send_eapol(struct eap_sm *sm, struct wpabuf *resp)\n{\n    size_t outlen;\n    int ret;\n    u8 *outbuf = NULL;",
        "int eap_sm_send_eapol(struct eap_sm *sm, struct wpabuf *resp)\n{\n    size_t outlen;\n    int ret;\n    u8 *outbuf = NULL;\n    int stack_backed = 0;\n    u8 stack_eapol[sizeof(struct l2_ethhdr) + sizeof(struct ieee802_1x_hdr) + 512];",
        "esp_wpa2.c EAPOL send stack fallback locals",
    );
    let patched_text = replace_required(
        &patched_text,
        "    outbuf = wpa2_sm_alloc_eapol(sm, IEEE802_1X_TYPE_EAP_PACKET,\n                                 wpabuf_head_u8(resp), wpabuf_len(resp),\n                                 &outlen, NULL);\n    if (!outbuf) {\n        return ESP_ERR_NO_MEM;\n    }\n\n    ret = wpa2_sm_ether_send(sm, bssid, ETH_P_EAPOL, outbuf, outlen);\n    wpa2_sm_free_eapol(outbuf);",
        "    outbuf = wpa2_sm_alloc_eapol(sm, IEEE802_1X_TYPE_EAP_PACKET,\n                                 wpabuf_head_u8(resp), wpabuf_len(resp),\n                                 &outlen, NULL);\n    if (!outbuf && wpabuf_len(resp) <= 512) {\n        struct ieee802_1x_hdr *hdr;\n        esp8266_hal_eap_trace_code('B');\n        hdr = (struct ieee802_1x_hdr *)(stack_eapol + sizeof(struct l2_ethhdr));\n        outlen = sizeof(struct ieee802_1x_hdr) + wpabuf_len(resp);\n        hdr->version = 0x01;\n        hdr->type = IEEE802_1X_TYPE_EAP_PACKET;\n        hdr->length = host_to_be16(wpabuf_len(resp));\n        memcpy(hdr + 1, wpabuf_head_u8(resp), wpabuf_len(resp));\n        outbuf = (u8 *)hdr;\n        stack_backed = 1;\n    }\n    if (!outbuf) {\n        esp8266_hal_eap_trace_code('b');\n        return ESP_ERR_NO_MEM;\n    }\n\n    ret = wpa2_sm_ether_send(sm, bssid, ETH_P_EAPOL, outbuf, outlen);\n    if (!stack_backed) {\n        wpa2_sm_free_eapol(outbuf);\n    }",
        "esp_wpa2.c EAPOL send stack fallback",
    );
    let patched_text = replace_required(
        &patched_text,
        "static int eap_sm_rx_eapol(u8 *src_addr, u8 *buf, u32 len, uint8_t *bssid)\n{\n    struct eap_sm *sm = gEapSm;\n\n    if (!sm) {\n        return ESP_FAIL;\n    }",
        "static int eap_sm_rx_eapol(u8 *src_addr, u8 *buf, u32 len, uint8_t *bssid)\n{\n    struct eap_sm *sm = gEapSm;\n    int ret;\n\n    esp8266_hal_eap_trace_code('3');\n    esp8266_hal_eap_trace_hex32((unsigned int)len);\n    if (!sm) {\n        esp8266_hal_eap_trace_code('m');\n        return ESP_FAIL;\n    }",
        "esp_wpa2.c EAPOL rx entry trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "    if (!sm) {\n        esp8266_hal_eap_trace_code('m');\n        return ESP_FAIL;\n    }\n#ifdef USE_WPA2_TASK\n    {",
        "    if (!sm) {\n        esp8266_hal_eap_trace_code('m');\n        return ESP_FAIL;\n    }\n#ifdef USE_WPA2_TASK\n    if (len > 384) {\n        esp8266_hal_eap_trace_code('L');\n        ret = eap_sm_rx_eapol_internal(src_addr, buf, len, bssid);\n        esp8266_hal_eap_trace_code('d');\n        esp8266_hal_eap_trace_hex32((unsigned int)ret);\n        return ret;\n    }\n    {",
        "esp_wpa2.c direct large EAPOL RX trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "        if (!param) {\n            return ESP_ERR_NO_MEM;\n        }\n\n        param->buf = (u8 *)os_zalloc(len);   /* free in task */\n        if (!param->buf) {\n            os_free(param);\n            return ESP_ERR_NO_MEM;\n        }",
        "        if (!param) {\n            esp8266_hal_eap_trace_code('4');\n            ret = eap_sm_rx_eapol_internal(src_addr, buf, len, bssid);\n            esp8266_hal_eap_trace_code('d');\n            esp8266_hal_eap_trace_hex32((unsigned int)ret);\n            return ret;\n        }\n\n        param->buf = (u8 *)os_zalloc(len);   /* free in task */\n        if (!param->buf) {\n            esp8266_hal_eap_trace_code('5');\n            os_free(param);\n            ret = eap_sm_rx_eapol_internal(src_addr, buf, len, bssid);\n            esp8266_hal_eap_trace_code('d');\n            esp8266_hal_eap_trace_hex32((unsigned int)ret);\n            return ret;\n        }",
        "esp_wpa2.c EAPOL rx queue allocation trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "        wpa2_rxq_enqueue(param);\n        return wpa2_post(SIG_WPA2_RX, 0);",
        "        wpa2_rxq_enqueue(param);\n        ret = wpa2_post(SIG_WPA2_RX, 0);\n        esp8266_hal_eap_trace_code('6');\n        esp8266_hal_eap_trace_hex32((unsigned int)ret);\n        return ret;",
        "esp_wpa2.c EAPOL rx post trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "static int wpa2_ent_rx_eapol(u8 *src_addr, u8 *buf, u32 len, uint8_t *bssid)\n{\n    struct ieee802_1x_hdr *hdr;\n    int ret = ESP_OK;\n\n    hdr = (struct ieee802_1x_hdr *) buf;\n\n    switch (hdr->type) {",
        "static int wpa2_ent_rx_eapol(u8 *src_addr, u8 *buf, u32 len, uint8_t *bssid)\n{\n    struct ieee802_1x_hdr *hdr;\n    int ret = ESP_OK;\n\n    hdr = (struct ieee802_1x_hdr *) buf;\n    esp8266_hal_eap_trace_code('2');\n    esp8266_hal_eap_trace_hex8((unsigned int)hdr->type);\n    esp8266_hal_eap_trace_hex32((unsigned int)len);\n\n    switch (hdr->type) {",
        "esp_wpa2.c EAPOL dispatch trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "static int eap_sm_rx_eapol_internal(u8 *src_addr, u8 *buf, u32 len, uint8_t *bssid)\n{\n    struct eap_sm *sm = gEapSm;",
        "static int eap_sm_rx_eapol_internal(u8 *src_addr, u8 *buf, u32 len, uint8_t *bssid)\n{\n    struct eap_sm *sm = gEapSm;\n    esp8266_hal_eap_trace_code('7');\n    esp8266_hal_eap_trace_hex32((unsigned int)len);",
        "esp_wpa2.c EAPOL internal entry trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "    struct wpabuf *req = NULL;\n    u8 *tmp;\n    int ret = ESP_FAIL;",
        "    struct wpabuf *req = NULL;\n    struct wpabuf stack_req;\n    int stack_req_used = 0;\n    u8 *tmp;\n    int ret = ESP_FAIL;",
        "esp_wpa2.c EAPOL internal stack request locals",
    );
    let patched_text = replace_required(
        &patched_text,
        "    if (len < sizeof(*hdr) + sizeof(*ehdr)) {",
        "    if (len < sizeof(*hdr) + sizeof(*ehdr)) {\n        esp8266_hal_eap_trace_code('8');",
        "esp_wpa2.c EAPOL short frame trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "    if (hdr->type != IEEE802_1X_TYPE_EAP_PACKET) {",
        "    esp8266_hal_eap_trace_code('8');\n    esp8266_hal_eap_trace_hex32((unsigned int)plen);\n    if (hdr->type != IEEE802_1X_TYPE_EAP_PACKET) {\n        esp8266_hal_eap_trace_code('9');",
        "esp_wpa2.c EAPOL header trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "    if (plen > len - sizeof(*hdr) || plen < sizeof(*ehdr)) {",
        "    if (plen > len - sizeof(*hdr) || plen < sizeof(*ehdr)) {\n        esp8266_hal_eap_trace_code('0');",
        "esp_wpa2.c EAPOL payload length trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "        req = wpabuf_alloc_copy((u8 *)ehdr, len - sizeof(*hdr));\n        ret = eap_sm_process_request(sm, req);\n        break;",
        "        req = wpabuf_alloc_copy((u8 *)ehdr, len - sizeof(*hdr));\n        esp8266_hal_eap_trace_code(req ? 'q' : 'Q');\n        if (!req) {\n            wpabuf_set(&stack_req, (u8 *)ehdr, len - sizeof(*hdr));\n            req = &stack_req;\n            stack_req_used = 1;\n            esp8266_hal_eap_trace_code('v');\n        }\n        ret = eap_sm_process_request(sm, req);\n        esp8266_hal_eap_trace_code('r');\n        esp8266_hal_eap_trace_hex32((unsigned int)ret);\n        break;",
        "esp_wpa2.c EAP request allocation/process trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "_out:\n    wpabuf_free(req);",
        "_out:\n    if (!stack_req_used) {\n        wpabuf_free(req);\n    }",
        "esp_wpa2.c EAP stack request free guard",
    );
    fs::write(&patched, patched_text).expect("failed to write patched esp_wpa2.c");
    patched
}

fn patch_esp_wpa2_source(source: &Path, out_dir: &Path) -> PathBuf {
    let patched = out_dir.join("esp_wpa2_patched.c");
    let source_text = fs::read_to_string(source).expect("failed to read esp_wpa2.c");
    let patched_text = replace_required(
        &source_text,
        "int eap_sm_send_eapol(struct eap_sm *sm, struct wpabuf *resp)\n{\n    size_t outlen;\n    int ret;\n    u8 *outbuf = NULL;",
        "int eap_sm_send_eapol(struct eap_sm *sm, struct wpabuf *resp)\n{\n    size_t outlen;\n    int ret;\n    u8 *outbuf = NULL;\n    int stack_backed = 0;\n    u8 stack_eapol[sizeof(struct l2_ethhdr) + sizeof(struct ieee802_1x_hdr) + 512];",
        "esp_wpa2.c EAPOL send stack fallback locals",
    );
    let patched_text = replace_required(
        &patched_text,
        "    outbuf = wpa2_sm_alloc_eapol(sm, IEEE802_1X_TYPE_EAP_PACKET,\n                                 wpabuf_head_u8(resp), wpabuf_len(resp),\n                                 &outlen, NULL);\n    if (!outbuf) {\n        return ESP_ERR_NO_MEM;\n    }\n\n    ret = wpa2_sm_ether_send(sm, bssid, ETH_P_EAPOL, outbuf, outlen);\n    wpa2_sm_free_eapol(outbuf);",
        "    outbuf = wpa2_sm_alloc_eapol(sm, IEEE802_1X_TYPE_EAP_PACKET,\n                                 wpabuf_head_u8(resp), wpabuf_len(resp),\n                                 &outlen, NULL);\n    if (!outbuf && wpabuf_len(resp) <= 512) {\n        struct ieee802_1x_hdr *hdr;\n        hdr = (struct ieee802_1x_hdr *)(stack_eapol + sizeof(struct l2_ethhdr));\n        outlen = sizeof(struct ieee802_1x_hdr) + wpabuf_len(resp);\n        hdr->version = 0x01;\n        hdr->type = IEEE802_1X_TYPE_EAP_PACKET;\n        hdr->length = host_to_be16(wpabuf_len(resp));\n        memcpy(hdr + 1, wpabuf_head_u8(resp), wpabuf_len(resp));\n        outbuf = (u8 *)hdr;\n        stack_backed = 1;\n    }\n    if (!outbuf) {\n        return ESP_ERR_NO_MEM;\n    }\n\n    ret = wpa2_sm_ether_send(sm, bssid, ETH_P_EAPOL, outbuf, outlen);\n    if (!stack_backed) {\n        wpa2_sm_free_eapol(outbuf);\n    }",
        "esp_wpa2.c EAPOL send stack fallback",
    );
    let patched_text = replace_required(
        &patched_text,
        "static int eap_sm_rx_eapol(u8 *src_addr, u8 *buf, u32 len, uint8_t *bssid)\n{\n    struct eap_sm *sm = gEapSm;\n\n    if (!sm) {\n        return ESP_FAIL;\n    }\n#ifdef USE_WPA2_TASK\n    {",
        "static int eap_sm_rx_eapol(u8 *src_addr, u8 *buf, u32 len, uint8_t *bssid)\n{\n    struct eap_sm *sm = gEapSm;\n\n    if (!sm) {\n        return ESP_FAIL;\n    }\n#ifdef USE_WPA2_TASK\n    if (len > 384) {\n        return eap_sm_rx_eapol_internal(src_addr, buf, len, bssid);\n    }\n    {",
        "esp_wpa2.c direct large EAPOL RX",
    );
    let patched_text = replace_required(
        &patched_text,
        "        if (!param) {\n            return ESP_ERR_NO_MEM;\n        }\n\n        param->buf = (u8 *)os_zalloc(len);   /* free in task */\n        if (!param->buf) {\n            os_free(param);\n            return ESP_ERR_NO_MEM;\n        }",
        "        if (!param) {\n            return eap_sm_rx_eapol_internal(src_addr, buf, len, bssid);\n        }\n\n        param->buf = (u8 *)os_zalloc(len);   /* free in task */\n        if (!param->buf) {\n            os_free(param);\n            return eap_sm_rx_eapol_internal(src_addr, buf, len, bssid);\n        }",
        "esp_wpa2.c EAPOL rx allocation fallback",
    );
    let patched_text = replace_required(
        &patched_text,
        "    struct wpabuf *req = NULL;\n    u8 *tmp;\n    int ret = ESP_FAIL;",
        "    struct wpabuf *req = NULL;\n    struct wpabuf stack_req;\n    int stack_req_used = 0;\n    u8 *tmp;\n    int ret = ESP_FAIL;",
        "esp_wpa2.c EAPOL internal stack request locals",
    );
    let patched_text = replace_required(
        &patched_text,
        "        req = wpabuf_alloc_copy((u8 *)ehdr, len - sizeof(*hdr));\n        ret = eap_sm_process_request(sm, req);\n        break;",
        "        req = wpabuf_alloc_copy((u8 *)ehdr, len - sizeof(*hdr));\n        if (!req) {\n            wpabuf_set(&stack_req, (u8 *)ehdr, len - sizeof(*hdr));\n            req = &stack_req;\n            stack_req_used = 1;\n        }\n        ret = eap_sm_process_request(sm, req);\n        break;",
        "esp_wpa2.c EAP request allocation fallback",
    );
    let patched_text = replace_required(
        &patched_text,
        "_out:\n    wpabuf_free(req);",
        "_out:\n    if (!stack_req_used) {\n        wpabuf_free(req);\n    }",
        "esp_wpa2.c EAP stack request free guard",
    );
    fs::write(&patched, patched_text).expect("failed to write patched esp_wpa2.c");
    patched
}

fn patch_eap_peap_trace_source(source: &Path, out_dir: &Path) -> PathBuf {
    let patched = out_dir.join("eap_peap_trace.c");
    let source_text = fs::read_to_string(source).expect("failed to read eap_peap.c");
    let patched_text = replace_required(
        &source_text,
        "#include \"eap_peer/eap_methods.h\"",
        &format!("#include \"eap_peer/eap_methods.h\"\n{}", eap_trace_hook()),
        "eap_peap.c EAP trace hook",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tdata = (struct eap_peap_data *)os_zalloc(sizeof(*data));\n\tif (data == NULL)\n\t\treturn NULL;",
        "\tesp8266_hal_eap_trace_code('A');\n\tdata = (struct eap_peap_data *)os_zalloc(sizeof(*data));\n\tif (data == NULL) {\n\t\tesp8266_hal_eap_trace_code('D');\n\t\treturn NULL;\n\t}",
        "eap_peap.c alloc trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tif (config && config->phase1 &&\n\t    eap_peap_parse_phase1(data, config->phase1) < 0) {\n\t\teap_peap_deinit(sm, data);\n\t\treturn NULL;\n\t}",
        "\tif (config && config->phase1 &&\n\t    eap_peap_parse_phase1(data, config->phase1) < 0) {\n\t\tesp8266_hal_eap_trace_code('1');\n\t\teap_peap_deinit(sm, data);\n\t\treturn NULL;\n\t}",
        "eap_peap.c phase1 trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tif (eap_peer_select_phase2_methods(config, \"auth=\",\n\t\t\t\t\t   &data->phase2_types,\n\t\t\t\t\t   &data->num_phase2_types) < 0) {\n\t\teap_peap_deinit(sm, data);\n\t\treturn NULL;\n\t}",
        "\tif (eap_peer_select_phase2_methods(config, \"auth=\",\n\t\t\t\t\t   &data->phase2_types,\n\t\t\t\t\t   &data->num_phase2_types) < 0) {\n\t\tesp8266_hal_eap_trace_code('2');\n\t\teap_peap_deinit(sm, data);\n\t\treturn NULL;\n\t}\n\tesp8266_hal_eap_trace_code('M');",
        "eap_peap.c phase2 selection trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tif (eap_peer_tls_ssl_init(sm, &data->ssl, config, EAP_TYPE_PEAP)) {\n\t\twpa_printf(MSG_INFO, \"EAP-PEAP: Failed to initialize SSL.\");\n\t\teap_peap_deinit(sm, data);\n\t\treturn NULL;\n\t}\n\n\treturn data;",
        "\tesp8266_hal_eap_trace_code('T');\n\tif (eap_peer_tls_ssl_init(sm, &data->ssl, config, EAP_TYPE_PEAP)) {\n\t\tesp8266_hal_eap_trace_code('t');\n\t\twpa_printf(MSG_INFO, \"EAP-PEAP: Failed to initialize SSL.\");\n\t\teap_peap_deinit(sm, data);\n\t\treturn NULL;\n\t}\n\n\tesp8266_hal_eap_trace_code('p');\n\treturn data;",
        "eap_peap.c tls init trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tpos = eap_peer_tls_process_init(sm, &data->ssl, EAP_TYPE_PEAP, ret,\n\t\t\t\t\treqData, &left, &flags);\n\tif (pos == NULL)\n\t\treturn NULL;",
        "\tesp8266_hal_eap_trace_code('m');\n\tpos = eap_peer_tls_process_init(sm, &data->ssl, EAP_TYPE_PEAP, ret,\n\t\t\t\t\treqData, &left, &flags);\n\tif (pos == NULL) {\n\t\tesp8266_hal_eap_trace_code('j');\n\t\treturn NULL;\n\t}\n\tesp8266_hal_eap_trace_code('J');\n\tesp8266_hal_eap_trace_hex32((unsigned int)left);\n\tesp8266_hal_eap_trace_hex8((unsigned int)flags);",
        "eap_peap.c process init trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\t\tres = eap_peer_tls_process_helper(sm, &data->ssl,\n\t\t\t\t\t\t  EAP_TYPE_PEAP,\n\t\t\t\t\t\t  data->peap_version, id, pos,\n\t\t\t\t\t\t  left, &resp);\n\n\t\tif (tls_connection_established(sm->ssl_ctx, data->ssl.conn)) {",
        "\t\tesp8266_hal_eap_trace_code('H');\n\t\tesp8266_hal_eap_trace_hex32((unsigned int)left);\n\t\tres = eap_peer_tls_process_helper(sm, &data->ssl,\n\t\t\t\t\t\t  EAP_TYPE_PEAP,\n\t\t\t\t\t\t  data->peap_version, id, pos,\n\t\t\t\t\t\t  left, &resp);\n\t\tesp8266_hal_eap_trace_code('g');\n\t\tesp8266_hal_eap_trace_hex32((unsigned int)res);\n\t\tesp8266_hal_eap_trace_code(resp ? 'G' : 'g');\n\n\t\tif (tls_connection_established(sm->ssl_ctx, data->ssl.conn)) {",
        "eap_peap.c helper return trace",
    );
    fs::write(&patched, patched_text).expect("failed to write patched eap_peap.c");
    patched
}

fn patch_eap_tls_common_source(source: &Path, out_dir: &Path) -> PathBuf {
    let patched = out_dir.join("eap_tls_common_patched.c");
    let source_text = fs::read_to_string(source).expect("failed to read eap_tls_common.c");
    let patched_text = replace_required(
        &source_text,
        "\tif (wpabuf_resize(&data->tls_in, in_len) < 0) {\n\t\twpa_printf(MSG_INFO, \"SSL: Could not allocate memory for TLS \"\n\t\t\t   \"data\");\n\t\teap_peer_tls_reset_input(data);\n\t\treturn -1;\n\t}\n\tif (in_data)\n\t\twpabuf_put_buf(data->tls_in, in_data);\n\tdata->tls_in_left -= in_len;",
        "\tif (data->tls_in && in_len == data->tls_in_left) {\n\t\textern void esp8266_hal_tls_set_extra_input(struct tls_connection *conn,\n\t\t\t\t\t\t\t\t  const u8 *data,\n\t\t\t\t\t\t\t\t  size_t len);\n\t\tif (in_data)\n\t\t\tesp8266_hal_tls_set_extra_input(data->conn, wpabuf_head_u8(in_data), in_len);\n\t\tdata->tls_in_left = 0;\n\t\treturn 0;\n\t}\n\tif (wpabuf_resize(&data->tls_in, in_len) < 0) {\n\t\twpa_printf(MSG_INFO, \"SSL: Could not allocate memory for TLS \"\n\t\t\t   \"data\");\n\t\teap_peer_tls_reset_input(data);\n\t\treturn -1;\n\t}\n\tif (in_data)\n\t\twpabuf_put_buf(data->tls_in, in_data);\n\tdata->tls_in_left -= in_len;",
        "eap_tls_common.c avoid copying final TLS input fragment",
    );
    let patched_text = replace_required(
        &patched_text,
        "\t} else {\n\t\t/* No fragments in this message, so just make a copy of it. */\n\t\tdata->tls_in_left = 0;\n\t\tdata->tls_in = wpabuf_dup(in_data);\n\t\tif (data->tls_in == NULL)\n\t\t\treturn NULL;\n\t}\n\n\treturn data->tls_in;",
        "\t} else {\n\t\t/* No fragments in this message, so use the caller-provided view. */\n\t\tdata->tls_in_left = 0;\n\t\treturn in_data;\n\t}\n\n\treturn data->tls_in;",
        "eap_tls_common.c avoid copying unfragmented TLS input",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tappl_data = NULL;\n\tdata->tls_out = tls_connection_handshake(data->ssl_ctx, data->conn,\n\t\t\t\t\t\t msg, &appl_data);",
        "\tif (msg == data->tls_in) {\n\t\textern void esp8266_hal_tls_set_input_owner(struct tls_connection *conn,\n\t\t\t\t\t\t\t\t  struct wpabuf *owner);\n\t\tesp8266_hal_tls_set_input_owner(data->conn, data->tls_in);\n\t\tdata->tls_in = NULL;\n\t\tdata->tls_in_left = data->tls_in_total = 0;\n\t}\n\tappl_data = NULL;\n\tdata->tls_out = tls_connection_handshake(data->ssl_ctx, data->conn,\n\t\t\t\t\t\t msg, &appl_data);",
        "eap_tls_common.c transfer reassembled TLS input ownership",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tdata->tls_out_limit = config->fragment_size;",
        "\tdata->tls_out_limit = config->fragment_size;\n\tif (data->tls_out_limit > 256 && config->client_cert && config->private_key &&\n\t    eap_get_config_blob(sm, (const char *)config->client_cert) &&\n\t    eap_get_config_blob(sm, (const char *)config->private_key))\n\t\tdata->tls_out_limit = 256;",
        "eap_tls_common.c client-certificate fragment cap",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tint ret = 0;\n\n\t*out_data = NULL;",
        "\tint ret = 0;\n\tint esp8266_hal_eap_input_reserved = 0;\n\n\t*out_data = NULL;\n\tif (in_len > 0) {\n\t\textern void esp8266_hal_eap_rsa_reserve(void);\n\t\tesp8266_hal_eap_rsa_reserve();\n\t\tesp8266_hal_eap_input_reserved = 1;\n\t}",
        "eap_tls_common.c helper input reserve",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tif (data->tls_out && wpabuf_len(data->tls_out) > 0 && in_len > 0) {\n\t\twpa_printf(MSG_DEBUG, \"SSL: Received non-ACK when output \"\n\t\t\t   \"fragments are waiting to be sent out\");\n\t\treturn -1;\n\t}",
        "\tif (data->tls_out && wpabuf_len(data->tls_out) > 0 && in_len > 0) {\n\t\twpa_printf(MSG_DEBUG, \"SSL: Received non-ACK when output \"\n\t\t\t   \"fragments are waiting to be sent out\");\n\t\tif (esp8266_hal_eap_input_reserved) {\n\t\t\textern void esp8266_hal_eap_rsa_release(void);\n\t\t\tesp8266_hal_eap_rsa_release();\n\t\t}\n\t\treturn -1;\n\t}",
        "eap_tls_common.c helper pending output release",
    );
    let patched_text = replace_required(
        &patched_text,
        "\t*out_data = NULL;\n\tif (in_len > 0) {\n\t\textern void esp8266_hal_eap_rsa_reserve(void);\n\t\tesp8266_hal_eap_rsa_reserve();\n\t\tesp8266_hal_eap_input_reserved = 1;\n\t}\n\n\tif (data->tls_out && wpabuf_len(data->tls_out) > 0 && in_len > 0) {",
        "\t*out_data = NULL;\n\n\tif (data->tls_out && wpabuf_len(data->tls_out) > 0 && in_len > 0) {",
        "eap_tls_common.c defer input reserve until output guard",
    );
    let patched_text = replace_required(
        &patched_text,
        "\t\treturn -1;\n\t}\n\n\tif (data->tls_out == NULL || wpabuf_len(data->tls_out) == 0) {",
        "\t\treturn -1;\n\t}\n\n\tif (in_len > 0) {\n\t\textern int esp8266_hal_tls_restore_deferred_input(struct tls_connection *conn);\n\t\textern void esp8266_hal_eap_rsa_reserve(void);\n\t\tret = esp8266_hal_tls_restore_deferred_input(data->conn);\n\t\tif (ret != 0)\n\t\t\treturn -1;\n\t\tesp8266_hal_eap_rsa_reserve();\n\t\tesp8266_hal_eap_input_reserved = 1;\n\t}\n\n\tif (data->tls_out == NULL || wpabuf_len(data->tls_out) == 0) {",
        "eap_tls_common.c restore deferred input before reserve",
    );
    let patched_text = replace_required(
        &patched_text,
        "\t\tif (res) {\n\t\t\t/*\n\t\t\t * Input processing failed (res = -1) or more data is\n\t\t\t * needed (res = 1).\n\t\t\t */\n\t\t\treturn res;\n\t\t}",
        "\t\tif (res) {\n\t\t\t/*\n\t\t\t * Input processing failed (res = -1) or more data is\n\t\t\t * needed (res = 1).\n\t\t\t */\n\t\t\tif (esp8266_hal_eap_input_reserved) {\n\t\t\t\textern void esp8266_hal_eap_rsa_release(void);\n\t\t\t\tesp8266_hal_eap_rsa_release();\n\t\t\t}\n\t\t\treturn res;\n\t\t}",
        "eap_tls_common.c helper input result release",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tif (data->tls_out == NULL) {\n\t\t/*\n\t\t * No outgoing fragments remaining from the previous message\n\t\t * and no new message generated. This indicates an error in TLS\n\t\t * processing.\n\t\t */\n\t\teap_peer_tls_reset_output(data);\n\t\treturn -1;\n\t}",
        "\tif (data->tls_out == NULL) {\n\t\t/*\n\t\t * No outgoing fragments remaining from the previous message\n\t\t * and no new message generated. This indicates an error in TLS\n\t\t * processing.\n\t\t */\n\t\teap_peer_tls_reset_output(data);\n\t\tif (esp8266_hal_eap_input_reserved) {\n\t\t\textern void esp8266_hal_eap_rsa_release(void);\n\t\t\tesp8266_hal_eap_rsa_release();\n\t\t}\n\t\treturn -1;\n\t}",
        "eap_tls_common.c helper no output release",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tif (data->tls_out == NULL || wpabuf_len(data->tls_out) == 0) {\n\t\t/*\n\t\t * TLS negotiation should now be complete since all other cases\n\t\t * needing more data should have been caught above based on\n\t\t * the TLS Message Length field.\n\t\t */\n\t\twpa_printf(MSG_DEBUG, \"SSL: No data to be sent out\");\n\t\twpabuf_free(data->tls_out);\n\t\tdata->tls_out = NULL;\n\t\treturn 1;\n\t}",
        "\tif (data->tls_out == NULL || wpabuf_len(data->tls_out) == 0) {\n\t\t/*\n\t\t * TLS negotiation should now be complete since all other cases\n\t\t * needing more data should have been caught above based on\n\t\t * the TLS Message Length field.\n\t\t */\n\t\twpa_printf(MSG_DEBUG, \"SSL: No data to be sent out\");\n\t\twpabuf_free(data->tls_out);\n\t\tdata->tls_out = NULL;\n\t\tif (esp8266_hal_eap_input_reserved) {\n\t\t\textern void esp8266_hal_eap_rsa_release(void);\n\t\t\tesp8266_hal_eap_rsa_release();\n\t\t}\n\t\treturn 1;\n\t}",
        "eap_tls_common.c helper empty output release",
    );
    let patched_text = replace_required(
        &patched_text,
        "\t*out_data = eap_tls_msg_alloc(eap_type, 1 + length_included * 4 + len,\n\t\t\t\t      EAP_CODE_RESPONSE, id);\n\tif (*out_data == NULL) {\n\t    printf(\"[Debug] out_data is null, return \\n\");\n\t\treturn -1;\n    } ",
        "\t{\n\t\textern void esp8266_hal_alloc_context_set(unsigned int context);\n\t\textern void esp8266_hal_alloc_context_clear(void);\n\t\tesp8266_hal_alloc_context_set(0x1001u);\n\t\t*out_data = eap_tls_msg_alloc(eap_type, 1 + length_included * 4 + len,\n\t\t\t\t      EAP_CODE_RESPONSE, id);\n\t\tesp8266_hal_alloc_context_clear();\n\t}\n\tif (*out_data == NULL) {\n\t    printf(\"[Debug] out_data is null, return \\n\");\n\t\treturn -1;\n    } ",
        "eap_tls_common.c output alloc context",
    );
    let patched_text = replace_required(
        &patched_text,
        "\t/* Send the pending message (in fragments, if needed). */\n\treturn eap_tls_process_output(data, eap_type, peap_version, id, ret,\n\t\t\t\t      out_data);",
        "\t/* Send the pending message (in fragments, if needed). */\n\tret = eap_tls_process_output(data, eap_type, peap_version, id, ret,\n\t\t\t\t      out_data);\n\tif (ret >= 0 && in_len == 0 && *out_data != NULL) {\n\t\textern void esp8266_hal_eap_rsa_boost(void);\n\t\tesp8266_hal_eap_rsa_boost();\n\t}\n\tif (esp8266_hal_eap_input_reserved) {\n\t\textern void esp8266_hal_eap_rsa_release(void);\n\t\tesp8266_hal_eap_rsa_release();\n\t}\n\treturn ret;",
        "eap_tls_common.c helper output release",
    );
    fs::write(&patched, patched_text).expect("failed to write patched eap_tls_common.c");
    patched
}

fn patch_eap_tls_common_trace_source(source: &Path, out_dir: &Path) -> PathBuf {
    let patched = out_dir.join("eap_tls_common_trace.c");
    let source_text = fs::read_to_string(source).expect("failed to read eap_tls_common.c");
    let patched_text = replace_required(
        &source_text,
        "#include \"eap_peer/eap_methods.h\"",
        &format!("#include \"eap_peer/eap_methods.h\"\n{}", eap_trace_hook()),
        "eap_tls_common.c EAP trace hook",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tdata->conn = tls_connection_init(data->ssl_ctx);\n\tif (data->conn == NULL) {\n\t\twpa_printf(MSG_INFO, \"SSL: Failed to initialize new TLS \"\n\t\t\t   \"connection\");\n\t\treturn -1;\n\t}\n\n\tres = tls_connection_set_params(data->ssl_ctx, data->conn, params);",
        "\tesp8266_hal_eap_trace_code('N');\n\tdata->conn = tls_connection_init(data->ssl_ctx);\n\tif (data->conn == NULL) {\n\t\tesp8266_hal_eap_trace_code('n');\n\t\twpa_printf(MSG_INFO, \"SSL: Failed to initialize new TLS \"\n\t\t\t   \"connection\");\n\t\treturn -1;\n\t}\n\n\tesp8266_hal_eap_trace_code('S');\n\tres = tls_connection_set_params(data->ssl_ctx, data->conn, params);\n\tesp8266_hal_eap_trace_code(res == 0 ? 's' : 'F');\n\tesp8266_hal_eap_trace_hex32((unsigned int)res);",
        "eap_tls_common.c set params trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tstruct tls_connection_params params;\n\n\tif (config == NULL)\n\t\treturn -1;",
        "\tstruct tls_connection_params params;\n\n\tesp8266_hal_eap_trace_code('L');\n\tif (config == NULL) {\n\t\tesp8266_hal_eap_trace_code('c');\n\t\treturn -1;\n\t}",
        "eap_tls_common.c ssl init entry trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tif (eap_tls_params_from_conf(sm, data, &params, config, data->phase2) < 0) /* no phase2 */\n\t\treturn -1;\n\n\tif (eap_tls_init_connection(sm, data, config, &params) < 0)\n\t\treturn -1;",
        "\tesp8266_hal_eap_trace_code('C');\n\tif (eap_tls_params_from_conf(sm, data, &params, config, data->phase2) < 0) { /* no phase2 */\n\t\tesp8266_hal_eap_trace_code('c');\n\t\treturn -1;\n\t}\n\n\tesp8266_hal_eap_trace_code('K');\n\tif (eap_tls_init_connection(sm, data, config, &params) < 0) {\n\t\tesp8266_hal_eap_trace_code('k');\n\t\treturn -1;\n\t}",
        "eap_tls_common.c ssl init progress trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\treturn 0;\n}\n\n\n/**\n * eap_peer_tls_ssl_deinit",
        "\tesp8266_hal_eap_trace_code('l');\n\treturn 0;\n}\n\n\n/**\n * eap_peer_tls_ssl_deinit",
        "eap_tls_common.c ssl init success trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\ttls_in_len = data->tls_in ? wpabuf_len(data->tls_in) : 0;\n\tin_len = in_data ? wpabuf_len(in_data) : 0;",
        "\ttls_in_len = data->tls_in ? wpabuf_len(data->tls_in) : 0;\n\tin_len = in_data ? wpabuf_len(in_data) : 0;\n\tesp8266_hal_eap_trace_code('<');\n\tesp8266_hal_eap_trace_hex32((unsigned int)data->tls_in_left);\n\tesp8266_hal_eap_trace_hex32((unsigned int)tls_in_len);\n\tesp8266_hal_eap_trace_hex32((unsigned int)in_len);",
        "eap_tls_common.c reassembly state trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tif (wpabuf_resize(&data->tls_in, in_len) < 0) {\n\t\twpa_printf(MSG_INFO, \"SSL: Could not allocate memory for TLS \"\n\t\t\t   \"data\");\n\t\teap_peer_tls_reset_input(data);\n\t\treturn -1;\n\t}\n\tif (in_data)\n\t\twpabuf_put_buf(data->tls_in, in_data);\n\tdata->tls_in_left -= in_len;",
        "\tif (data->tls_in && in_len == data->tls_in_left) {\n\t\textern void esp8266_hal_tls_set_extra_input(struct tls_connection *conn,\n\t\t\t\t\t\t\t\t  const u8 *data,\n\t\t\t\t\t\t\t\t  size_t len);\n\t\tesp8266_hal_eap_trace_code('+');\n\t\tesp8266_hal_eap_trace_hex32((unsigned int)in_len);\n\t\tif (in_data)\n\t\t\tesp8266_hal_tls_set_extra_input(data->conn, wpabuf_head_u8(in_data), in_len);\n\t\tdata->tls_in_left = 0;\n\t\treturn 0;\n\t}\n\tif (wpabuf_resize(&data->tls_in, in_len) < 0) {\n\t\tesp8266_hal_eap_trace_code('!');\n\t\tesp8266_hal_eap_trace_hex32((unsigned int)in_len);\n\t\twpa_printf(MSG_INFO, \"SSL: Could not allocate memory for TLS \"\n\t\t\t   \"data\");\n\t\teap_peer_tls_reset_input(data);\n\t\treturn -1;\n\t}\n\tesp8266_hal_eap_trace_code('>');\n\tesp8266_hal_eap_trace_hex32((unsigned int)(tls_in_len + in_len));\n\tif (in_data)\n\t\twpabuf_put_buf(data->tls_in, in_data);\n\tdata->tls_in_left -= in_len;",
        "eap_tls_common.c reassembly resize trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\t} else {\n\t\t/* No fragments in this message, so just make a copy of it. */\n\t\tdata->tls_in_left = 0;\n\t\tdata->tls_in = wpabuf_dup(in_data);\n\t\tif (data->tls_in == NULL)\n\t\t\treturn NULL;\n\t}\n\n\treturn data->tls_in;",
        "\t} else {\n\t\t/* No fragments in this message, so use the caller-provided view. */\n\t\tdata->tls_in_left = 0;\n\t\treturn in_data;\n\t}\n\n\treturn data->tls_in;",
        "eap_tls_common.c avoid copying unfragmented TLS input trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\twpabuf_set(&buf, in_data, in_len);\n\tmsg = eap_peer_tls_data_reassemble(data, &buf, &need_more_input);\n\tif (msg == NULL)\n\t\treturn need_more_input ? 1 : -1;",
        "\tesp8266_hal_eap_trace_code('I');\n\tesp8266_hal_eap_trace_hex32((unsigned int)in_len);\n\twpabuf_set(&buf, in_data, in_len);\n\tmsg = eap_peer_tls_data_reassemble(data, &buf, &need_more_input);\n\tif (msg == NULL) {\n\t\tesp8266_hal_eap_trace_code('u');\n\t\tesp8266_hal_eap_trace_hex32((unsigned int)need_more_input);\n\t\treturn need_more_input ? 1 : -1;\n\t}\n\tesp8266_hal_eap_trace_code('V');\n\tesp8266_hal_eap_trace_hex32((unsigned int)wpabuf_len(msg));",
        "eap_tls_common.c input reassembly trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tappl_data = NULL;\n\tdata->tls_out = tls_connection_handshake(data->ssl_ctx, data->conn,\n\t\t\t\t\t\t msg, &appl_data);",
        "\tif (msg == data->tls_in) {\n\t\textern void esp8266_hal_tls_set_input_owner(struct tls_connection *conn,\n\t\t\t\t\t\t\t\t  struct wpabuf *owner);\n\t\tesp8266_hal_eap_trace_code(']');\n\t\tesp8266_hal_tls_set_input_owner(data->conn, data->tls_in);\n\t\tdata->tls_in = NULL;\n\t\tdata->tls_in_left = data->tls_in_total = 0;\n\t}\n\tappl_data = NULL;\n\tdata->tls_out = tls_connection_handshake(data->ssl_ctx, data->conn,\n\t\t\t\t\t\t msg, &appl_data);\n\tesp8266_hal_eap_trace_code('Y');\n\tesp8266_hal_eap_trace_hex32(data->tls_out ? (unsigned int)wpabuf_len(data->tls_out) : 0xFFFFFFFFu);\n\tesp8266_hal_eap_trace_code(appl_data ? 'A' : 'a');",
        "eap_tls_common.c handshake result trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tdata->tls_out_limit = config->fragment_size;",
        "\tdata->tls_out_limit = config->fragment_size;\n\tif (data->tls_out_limit > 256 && config->client_cert && config->private_key &&\n\t    eap_get_config_blob(sm, (const char *)config->client_cert) &&\n\t    eap_get_config_blob(sm, (const char *)config->private_key)) {\n\t\tdata->tls_out_limit = 256;\n\t\tesp8266_hal_eap_trace_code('m');\n\t}",
        "eap_tls_common.c client-certificate fragment cap trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tif (data->tls_out == NULL)\n\t\treturn -1;\n\tlen = wpabuf_len(data->tls_out) - data->tls_out_pos;",
        "\tif (data->tls_out == NULL) {\n\t\tesp8266_hal_eap_trace_code('O');\n\t\treturn -1;\n\t}\n\tlen = wpabuf_len(data->tls_out) - data->tls_out_pos;\n\tesp8266_hal_eap_trace_code('o');\n\tesp8266_hal_eap_trace_hex32((unsigned int)len);\n\tesp8266_hal_eap_trace_hex32((unsigned int)wpabuf_len(data->tls_out));",
        "eap_tls_common.c output length trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\t*out_data = eap_tls_msg_alloc(eap_type, 1 + length_included * 4 + len,\n\t\t\t\t      EAP_CODE_RESPONSE, id);\n\tif (*out_data == NULL) {\n\t    printf(\"[Debug] out_data is null, return \\n\");\n\t\treturn -1;\n    } ",
        "\t{\n\t\textern void esp8266_hal_alloc_context_set(unsigned int context);\n\t\textern void esp8266_hal_alloc_context_clear(void);\n\t\tesp8266_hal_alloc_context_set(0x1001u);\n\t\t*out_data = eap_tls_msg_alloc(eap_type, 1 + length_included * 4 + len,\n\t\t\t\t      EAP_CODE_RESPONSE, id);\n\t\tesp8266_hal_alloc_context_clear();\n\t}\n\tif (*out_data == NULL) {\n\t\tesp8266_hal_eap_trace_code('z');\n\t\tesp8266_hal_eap_trace_hex32((unsigned int)(1 + length_included * 4 + len));\n\t    printf(\"[Debug] out_data is null, return \\n\");\n\t\treturn -1;\n    }\n\tesp8266_hal_eap_trace_code('Z');\n\tesp8266_hal_eap_trace_hex32((unsigned int)wpabuf_len(*out_data)); ",
        "eap_tls_common.c output alloc trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tint ret = 0;\n\n\t*out_data = NULL;",
        "\tint ret = 0;\n\tint esp8266_hal_eap_input_reserved = 0;\n\n\t*out_data = NULL;",
        "eap_tls_common.c helper input reserve flag trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\t*out_data = NULL;\n\n\tif (data->tls_out && wpabuf_len(data->tls_out) > 0 && in_len > 0) {",
        "\t*out_data = NULL;\n\tesp8266_hal_eap_trace_code('h');\n\tesp8266_hal_eap_trace_hex32((unsigned int)in_len);\n\tif (in_len > 0) {\n\t\textern void esp8266_hal_eap_rsa_reserve(void);\n\t\tesp8266_hal_eap_trace_code('{');\n\t\tesp8266_hal_eap_rsa_reserve();\n\t\tesp8266_hal_eap_input_reserved = 1;\n\t}\n\n\tif (data->tls_out && wpabuf_len(data->tls_out) > 0 && in_len > 0) {",
        "eap_tls_common.c helper entry trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tif (data->tls_out && wpabuf_len(data->tls_out) > 0 && in_len > 0) {\n\t\twpa_printf(MSG_DEBUG, \"SSL: Received non-ACK when output \"\n\t\t\t   \"fragments are waiting to be sent out\");\n\t\treturn -1;\n\t}",
        "\tif (data->tls_out && wpabuf_len(data->tls_out) > 0 && in_len > 0) {\n\t\tesp8266_hal_eap_trace_code('f');\n\t\tesp8266_hal_eap_trace_hex32((unsigned int)wpabuf_len(data->tls_out));\n\t\twpa_printf(MSG_DEBUG, \"SSL: Received non-ACK when output \"\n\t\t\t   \"fragments are waiting to be sent out\");\n\t\tif (esp8266_hal_eap_input_reserved) {\n\t\t\textern void esp8266_hal_eap_rsa_release(void);\n\t\t\tesp8266_hal_eap_rsa_release();\n\t\t\tesp8266_hal_eap_trace_code('}');\n\t\t}\n\t\treturn -1;\n\t}",
        "eap_tls_common.c pending output trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\t*out_data = NULL;\n\tesp8266_hal_eap_trace_code('h');\n\tesp8266_hal_eap_trace_hex32((unsigned int)in_len);\n\tif (in_len > 0) {\n\t\textern void esp8266_hal_eap_rsa_reserve(void);\n\t\tesp8266_hal_eap_trace_code('{');\n\t\tesp8266_hal_eap_rsa_reserve();\n\t\tesp8266_hal_eap_input_reserved = 1;\n\t}\n\n\tif (data->tls_out && wpabuf_len(data->tls_out) > 0 && in_len > 0) {",
        "\t*out_data = NULL;\n\tesp8266_hal_eap_trace_code('h');\n\tesp8266_hal_eap_trace_hex32((unsigned int)in_len);\n\n\tif (data->tls_out && wpabuf_len(data->tls_out) > 0 && in_len > 0) {",
        "eap_tls_common.c defer input reserve until output guard trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\t\treturn -1;\n\t}\n\n\tif (data->tls_out == NULL || wpabuf_len(data->tls_out) == 0) {",
        "\t\treturn -1;\n\t}\n\n\tif (in_len > 0) {\n\t\textern int esp8266_hal_tls_restore_deferred_input(struct tls_connection *conn);\n\t\textern void esp8266_hal_eap_rsa_reserve(void);\n\t\tesp8266_hal_eap_trace_code('?');\n\t\tret = esp8266_hal_tls_restore_deferred_input(data->conn);\n\t\tif (ret != 0) {\n\t\t\tesp8266_hal_eap_trace_code('q');\n\t\t\tesp8266_hal_eap_trace_hex32((unsigned int)ret);\n\t\t\treturn -1;\n\t\t}\n\t\tesp8266_hal_eap_trace_code('{');\n\t\tesp8266_hal_eap_rsa_reserve();\n\t\tesp8266_hal_eap_input_reserved = 1;\n\t}\n\n\tif (data->tls_out == NULL || wpabuf_len(data->tls_out) == 0) {",
        "eap_tls_common.c restore deferred input before reserve trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\t\tint res = eap_tls_process_input(sm, data, in_data, in_len,\n\t\t\t\t\t\tout_data);\n\t\tif (res) {",
        "\t\tint res = eap_tls_process_input(sm, data, in_data, in_len,\n\t\t\t\t\t\tout_data);\n\t\tesp8266_hal_eap_trace_code('R');\n\t\tesp8266_hal_eap_trace_hex32((unsigned int)res);\n\t\tesp8266_hal_eap_trace_code(*out_data ? '1' : '0');\n\t\tif (res) {",
        "eap_tls_common.c process input result trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\t\tesp8266_hal_eap_trace_code(*out_data ? '1' : '0');\n\t\tif (res) {\n\t\t\t/*\n\t\t\t * Input processing failed (res = -1) or more data is\n\t\t\t * needed (res = 1).\n\t\t\t */\n\t\t\treturn res;\n\t\t}",
        "\t\tesp8266_hal_eap_trace_code(*out_data ? '1' : '0');\n\t\tif (res) {\n\t\t\t/*\n\t\t\t * Input processing failed (res = -1) or more data is\n\t\t\t * needed (res = 1).\n\t\t\t */\n\t\t\tif (esp8266_hal_eap_input_reserved) {\n\t\t\t\textern void esp8266_hal_eap_rsa_release(void);\n\t\t\t\tesp8266_hal_eap_rsa_release();\n\t\t\t\tesp8266_hal_eap_trace_code('}');\n\t\t\t}\n\t\t\treturn res;\n\t\t}",
        "eap_tls_common.c process input result release trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tif (data->tls_out == NULL) {\n\t\t/*\n\t\t * No outgoing fragments remaining from the previous message\n\t\t * and no new message generated. This indicates an error in TLS\n\t\t * processing.\n\t\t */\n\t\teap_peer_tls_reset_output(data);\n\t\treturn -1;\n\t}",
        "\tif (data->tls_out == NULL) {\n\t\tesp8266_hal_eap_trace_code('x');\n\t\t/*\n\t\t * No outgoing fragments remaining from the previous message\n\t\t * and no new message generated. This indicates an error in TLS\n\t\t * processing.\n\t\t */\n\t\teap_peer_tls_reset_output(data);\n\t\tif (esp8266_hal_eap_input_reserved) {\n\t\t\textern void esp8266_hal_eap_rsa_release(void);\n\t\t\tesp8266_hal_eap_rsa_release();\n\t\t\tesp8266_hal_eap_trace_code('}');\n\t\t}\n\t\treturn -1;\n\t}",
        "eap_tls_common.c no output trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tif (tls_connection_get_failed(data->ssl_ctx, data->conn)) {\n\t\t/* TLS processing has failed - return error */\n\t\twpa_printf(MSG_DEBUG, \"SSL: Failed - tls_out available to \"\n\t\t\t   \"report error\");\n\t\tret = -1;\n\t\t/* TODO: clean pin if engine used? */\n\t}",
        "\tif (tls_connection_get_failed(data->ssl_ctx, data->conn)) {\n\t\tesp8266_hal_eap_trace_code('F');\n\t\t/* TLS processing has failed - return error */\n\t\twpa_printf(MSG_DEBUG, \"SSL: Failed - tls_out available to \"\n\t\t\t   \"report error\");\n\t\tret = -1;\n\t\t/* TODO: clean pin if engine used? */\n\t}",
        "eap_tls_common.c tls failed trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tif (data->tls_out == NULL || wpabuf_len(data->tls_out) == 0) {\n\t\t/*\n\t\t * TLS negotiation should now be complete since all other cases\n\t\t * needing more data should have been caught above based on\n\t\t * the TLS Message Length field.\n\t\t */\n\t\twpa_printf(MSG_DEBUG, \"SSL: No data to be sent out\");\n\t\twpabuf_free(data->tls_out);\n\t\tdata->tls_out = NULL;\n\t\treturn 1;\n\t}",
        "\tif (data->tls_out == NULL || wpabuf_len(data->tls_out) == 0) {\n\t\tesp8266_hal_eap_trace_code('e');\n\t\t/*\n\t\t * TLS negotiation should now be complete since all other cases\n\t\t * needing more data should have been caught above based on\n\t\t * the TLS Message Length field.\n\t\t */\n\t\twpa_printf(MSG_DEBUG, \"SSL: No data to be sent out\");\n\t\twpabuf_free(data->tls_out);\n\t\tdata->tls_out = NULL;\n\t\tif (esp8266_hal_eap_input_reserved) {\n\t\t\textern void esp8266_hal_eap_rsa_release(void);\n\t\t\tesp8266_hal_eap_rsa_release();\n\t\t\tesp8266_hal_eap_trace_code('}');\n\t\t}\n\t\treturn 1;\n\t}",
        "eap_tls_common.c empty output trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\t/* Send the pending message (in fragments, if needed). */\n\treturn eap_tls_process_output(data, eap_type, peap_version, id, ret,\n\t\t\t\t      out_data);",
        "\tesp8266_hal_eap_trace_code('P');\n\tesp8266_hal_eap_trace_hex32((unsigned int)(wpabuf_len(data->tls_out) - data->tls_out_pos));\n\t/* Send the pending message (in fragments, if needed). */\n\tret = eap_tls_process_output(data, eap_type, peap_version, id, ret,\n\t\t\t\t      out_data);\n\tif (ret >= 0 && in_len == 0 && *out_data != NULL) {\n\t\textern void esp8266_hal_eap_rsa_boost(void);\n\t\tesp8266_hal_eap_trace_code('^');\n\t\tesp8266_hal_eap_rsa_boost();\n\t}\n\tif (esp8266_hal_eap_input_reserved) {\n\t\textern void esp8266_hal_eap_rsa_release(void);\n\t\tesp8266_hal_eap_rsa_release();\n\t\tesp8266_hal_eap_trace_code('}');\n\t}\n\treturn ret;",
        "eap_tls_common.c output call trace",
    );
    fs::write(&patched, patched_text).expect("failed to write patched eap_tls_common.c");
    patched
}

fn patch_tls_mbedtls_source(source: &Path, out_dir: &Path) -> PathBuf {
    let patched = out_dir.join("tls_mbedtls_patched.c");
    let source_text = fs::read_to_string(source).expect("failed to read tls_mbedtls.c");
    let patched_text = replace_required(
        &source_text,
        "struct tls_data {\n\t/* Data for mbedlts */\n\tstruct wpabuf *in_data;\n\t/* Data from mbedtls */\n\tstruct wpabuf *out_data;\n};",
        "struct tls_data {\n\t/* Data for mbedlts */\n\tstruct wpabuf *in_data;\n\tconst u8 *in_ext_data;\n\tsize_t in_ext_len;\n\tsize_t in_ext_pos;\n\tstruct wpabuf *in_ext_owner;\n\tconst u8 *in_ext_extra_data;\n\tsize_t in_ext_extra_len;\n\tsize_t in_ext_extra_pos;\n\t/* Data from mbedtls */\n\tstruct wpabuf *out_data;\n};",
        "tls_mbedtls.c external input cursor fields",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tsize_t in_ext_extra_pos;\n\t/* Data from mbedtls */",
        "\tsize_t in_ext_extra_pos;\n\tint in_buf_restore_pending;\n\t/* Data from mbedtls */",
        "tls_mbedtls.c deferred input-buffer restore flag",
    );
    let patched_text = replace_required(
        &patched_text,
        "struct tls_connection {\n\ttls_context_t *tls;\n\tstruct tls_data tls_io_data;\n\tunsigned char randbytes[2 * TLS_RANDOM_LEN];\n};",
        "struct tls_connection {\n\ttls_context_t *tls;\n\tstruct tls_data tls_io_data;\n\tunsigned char randbytes[2 * TLS_RANDOM_LEN];\n};\n\nvoid esp8266_hal_tls_set_extra_input(struct tls_connection *conn,\n\t\t\t\t\t     const u8 *data, size_t len)\n{\n\tif (conn == NULL)\n\t\treturn;\n\tconn->tls_io_data.in_ext_extra_data = data;\n\tconn->tls_io_data.in_ext_extra_len = len;\n\tconn->tls_io_data.in_ext_extra_pos = 0;\n}\n\nvoid esp8266_hal_tls_set_input_owner(struct tls_connection *conn,\n\t\t\t\t\t     struct wpabuf *owner)\n{\n\tif (conn == NULL)\n\t\treturn;\n\twpabuf_free(conn->tls_io_data.in_ext_owner);\n\tconn->tls_io_data.in_ext_owner = owner;\n}",
        "tls_mbedtls.c external input extra fragment setter",
    );
    let patched_text = replace_required(
        &patched_text,
        "static int set_pki_context(tls_context_t *tls, const struct tls_connection_params *cfg)",
        "static int esp8266_hal_x509_blob_is_pem(const unsigned char *blob, size_t len)\n{\n\treturn len >= 10 && os_memcmp(blob, \"-----BEGIN\", 10) == 0;\n}\n\nstatic int esp8266_hal_x509_crt_parse_blob(mbedtls_x509_crt *crt,\n\t\t\t\t\t    const unsigned char *blob,\n\t\t\t\t\t    size_t len)\n{\n\tif (esp8266_hal_x509_blob_is_pem(blob, len))\n\t\treturn mbedtls_x509_crt_parse(crt, blob, len);\n\n\treturn mbedtls_x509_crt_parse_der(crt, blob, len);\n}\n\nstatic void esp8266_hal_tls_drop_verified_ca(tls_context_t *tls)\n{\n\tif (tls == NULL || tls->cacert_ptr == NULL)\n\t\treturn;\n\tif (tls->ssl.conf == NULL || tls->ssl.conf->endpoint != MBEDTLS_SSL_IS_CLIENT)\n\t\treturn;\n\tif (tls->ssl.state <= MBEDTLS_SSL_SERVER_CERTIFICATE)\n\t\treturn;\n\n\tmbedtls_ssl_conf_ca_chain(&tls->conf, NULL, NULL);\n\tmbedtls_x509_crt_free(&tls->cacert);\n\ttls->cacert_ptr = NULL;\n}\n\nstatic int set_pki_context(tls_context_t *tls, const struct tls_connection_params *cfg)",
        "tls_mbedtls.c DER/PEM certificate parser",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tret = mbedtls_x509_crt_parse(&tls->clientcert,\n\t\t\t\t     cfg->client_cert_blob, cfg->client_cert_blob_len);",
        "\tret = esp8266_hal_x509_crt_parse_blob(&tls->clientcert,\n\t\t\t\t\t\t cfg->client_cert_blob,\n\t\t\t\t\t\t cfg->client_cert_blob_len);",
        "tls_mbedtls.c parse client cert DER/PEM blob",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tint ret = mbedtls_x509_crt_parse(tls->cacert_ptr, cacert, cacert_len);",
        "\tint ret = esp8266_hal_x509_crt_parse_blob(tls->cacert_ptr,\n\t\t\t\t\t\t  cacert, cacert_len);",
        "tls_mbedtls.c parse CA cert DER/PEM blob",
    );
    let patched_text = replace_required(
        &patched_text,
        "static int tls_mbedtls_read(void *ctx, unsigned char *buf, size_t len)\n{\n\tstruct tls_connection *conn = (struct tls_connection *)ctx;\n\tstruct tls_data *data = &conn->tls_io_data;\n\tstruct wpabuf *local_buf;\n\tsize_t data_len = len;\n\n\tif (len > wpabuf_len(data->in_data)) {\n\t\twpa_printf(MSG_ERROR, \"don't have suffient data\\n\");\n\t\tdata_len = wpabuf_len(data->in_data);\n\t}\n\n\tos_memcpy(buf, wpabuf_head(data->in_data), data_len);\n\t/* adjust buffer */\n\tif (len < wpabuf_len(data->in_data)) {\n\t\tlocal_buf = wpabuf_alloc_copy(wpabuf_head(data->in_data) + len,\n\t\t\t\t\t      wpabuf_len(data->in_data) - len);\n\t\twpabuf_free(data->in_data);\n\t\tdata->in_data = local_buf;\n\t} else {\n\t\twpabuf_free(data->in_data);\n\t\tdata->in_data = NULL;\n\t}\n\n\treturn data_len;\n}",
        r#"static int tls_mbedtls_read(void *ctx, unsigned char *buf, size_t len)
{
	struct tls_connection *conn = (struct tls_connection *)ctx;
	struct tls_data *data = &conn->tls_io_data;
	struct wpabuf *local_buf;
	size_t copied = 0;

	while (copied < len) {
		size_t available;
		size_t chunk;

		if (data->in_ext_data) {
			available = data->in_ext_len - data->in_ext_pos;
			chunk = len - copied;
			if (chunk > available)
				chunk = available;
			os_memcpy(buf + copied, data->in_ext_data + data->in_ext_pos,
				  chunk);
			copied += chunk;
			data->in_ext_pos += chunk;
			if (data->in_ext_pos >= data->in_ext_len) {
				data->in_ext_data = NULL;
				data->in_ext_len = 0;
				data->in_ext_pos = 0;
				wpabuf_free(data->in_ext_owner);
				data->in_ext_owner = NULL;
			}
			continue;
		}

		if (data->in_ext_extra_data) {
			available = data->in_ext_extra_len -
				    data->in_ext_extra_pos;
			chunk = len - copied;
			if (chunk > available)
				chunk = available;
			os_memcpy(buf + copied,
				  data->in_ext_extra_data +
				  data->in_ext_extra_pos,
				  chunk);
			copied += chunk;
			data->in_ext_extra_pos += chunk;
			if (data->in_ext_extra_pos >= data->in_ext_extra_len) {
				data->in_ext_extra_data = NULL;
				data->in_ext_extra_len = 0;
				data->in_ext_extra_pos = 0;
			}
			continue;
		}

		break;
	}

	if (copied == len)
		return copied;

	if (!data->in_data) {
		if (copied > 0) {
			wpa_printf(MSG_ERROR, "don't have suffient data\n");
			return copied;
		}
		return MBEDTLS_ERR_SSL_WANT_READ;
	}

	if (len - copied > wpabuf_len(data->in_data)) {
		wpa_printf(MSG_ERROR, "don't have suffient data\n");
		len = copied + wpabuf_len(data->in_data);
	}

	if (len > copied) {
		size_t data_len = len - copied;
		const u8 *head = wpabuf_head_u8(data->in_data);

		if (data_len < wpabuf_len(data->in_data)) {
			local_buf = wpabuf_alloc_copy(head + data_len,
						      wpabuf_len(data->in_data) -
						      data_len);
			if (local_buf == NULL)
				return copied > 0 ? copied : MBEDTLS_ERR_SSL_ALLOC_FAILED;
			os_memcpy(buf + copied, head, data_len);
			wpabuf_free(data->in_data);
			data->in_data = local_buf;
		} else {
			os_memcpy(buf + copied, head, data_len);
			wpabuf_free(data->in_data);
			data->in_data = NULL;
		}
		copied += data_len;
	}

	return copied;
}"#,
        "tls_mbedtls.c external input cursor read",
    );
    let patched_text = replace_required(
        &patched_text,
        "static int tls_mbedtls_write(void *ctx, const unsigned char *buf, size_t len)\n{\n\tstruct tls_connection *conn = (struct tls_connection *)ctx;\n\tstruct tls_data *data = &conn->tls_io_data;\n\n\tif (data->out_data) {\n\t\twpabuf_resize(&data->out_data, len);\n\t} else {\n\t\tdata->out_data = wpabuf_alloc(len);\n\t}\n\n\twpabuf_put_data(data->out_data, buf, len);\n\n\treturn len;\n}",
        "#define ESP8266_HAL_TLS_CLIENT_FLIGHT_OUT_PREALLOC_BYTES 704\n\nstatic int esp8266_hal_tls_prealloc_client_flight_out(struct tls_connection *conn)\n{\n\tstruct tls_data *data = &conn->tls_io_data;\n\textern void esp8266_hal_alloc_context_set(unsigned int context);\n\textern void esp8266_hal_alloc_context_clear(void);\n\n\tif (data->out_data != NULL)\n\t\treturn 0;\n\n\tesp8266_hal_alloc_context_set(0x2006u);\n\tdata->out_data = wpabuf_alloc(ESP8266_HAL_TLS_CLIENT_FLIGHT_OUT_PREALLOC_BYTES);\n\tesp8266_hal_alloc_context_clear();\n\n\treturn data->out_data == NULL ? MBEDTLS_ERR_SSL_ALLOC_FAILED : 0;\n}\n\nstatic int tls_mbedtls_write(void *ctx, const unsigned char *buf, size_t len)\n{\n\tstruct tls_connection *conn = (struct tls_connection *)ctx;\n\tstruct tls_data *data = &conn->tls_io_data;\n\textern void esp8266_hal_alloc_context_set(unsigned int context);\n\textern void esp8266_hal_alloc_context_clear(void);\n\n\tif (data->out_data) {\n\t\tint resize_ret;\n\t\tesp8266_hal_alloc_context_set(0x2004u);\n\t\tresize_ret = wpabuf_resize(&data->out_data, len);\n\t\tesp8266_hal_alloc_context_clear();\n\t\tif (resize_ret < 0) {\n\t\t\treturn MBEDTLS_ERR_SSL_ALLOC_FAILED;\n\t\t}\n\t} else {\n\t\tesp8266_hal_alloc_context_set(0x2004u);\n\t\tdata->out_data = wpabuf_alloc(len);\n\t\tesp8266_hal_alloc_context_clear();\n\t}\n\tif (!data->out_data) {\n\t\treturn MBEDTLS_ERR_SSL_ALLOC_FAILED;\n\t}\n\n\twpabuf_put_data(data->out_data, buf, len);\n\n\treturn len;\n}",
        "tls_mbedtls.c write allocation guard",
    );
    let patched_text = replace_required(
        &patched_text,
        "struct wpabuf * tls_connection_handshake(void *tls_ctx,",
        "static int esp8266_hal_tls_release_in_buf_for_client_flight(mbedtls_ssl_context *ssl)\n{\n\tif (ssl->conf == NULL || ssl->conf->endpoint != MBEDTLS_SSL_IS_CLIENT)\n\t\treturn 0;\n\n\tif (ssl->in_buf == NULL || ssl->in_offt != NULL || ssl->in_left != 0)\n\t\treturn 0;\n\n\tif (ssl->in_hslen != 0 && ssl->in_hslen < ssl->in_msglen)\n\t\treturn 0;\n\n\tos_memset(ssl->in_buf, 0, MBEDTLS_SSL_IN_BUFFER_LEN);\n\tmbedtls_free(ssl->in_buf);\n\tssl->in_buf = NULL;\n\tssl->in_hdr = NULL;\n\tssl->in_ctr = NULL;\n\tssl->in_len = NULL;\n\tssl->in_iv = NULL;\n\tssl->in_msg = NULL;\n\tssl->in_msglen = 0;\n\tssl->in_hslen = 0;\n\n\treturn 1;\n}\n\nstatic int esp8266_hal_tls_restore_in_buf_after_client_flight(mbedtls_ssl_context *ssl,\n\t\t\t\t\t\t\t      int released)\n{\n\tif (!released)\n\t\treturn 0;\n\n\tssl->in_buf = mbedtls_calloc(1, MBEDTLS_SSL_IN_BUFFER_LEN);\n\tif (ssl->in_buf == NULL)\n\t\treturn MBEDTLS_ERR_SSL_ALLOC_FAILED;\n\n\tssl->in_left = 0;\n#if defined(MBEDTLS_SSL_PROTO_DTLS)\n\tssl->next_record_offset = 0;\n\tif (ssl->conf->transport == MBEDTLS_SSL_TRANSPORT_DATAGRAM) {\n\t\tssl->in_hdr = ssl->in_buf;\n\t\tssl->in_ctr = ssl->in_hdr + 3;\n\t\tssl->in_len = ssl->in_hdr + 11;\n\t\tssl->in_iv = ssl->in_hdr + 13;\n\t\tssl->in_msg = ssl->in_iv;\n\t} else\n#endif\n\t{\n\t\tssl->in_hdr = ssl->in_buf + 8;\n\t\tssl->in_ctr = ssl->in_hdr - 8;\n\t\tssl->in_len = ssl->in_hdr + 3;\n\t\tssl->in_iv = ssl->in_hdr + 5;\n\t\tssl->in_msg = ssl->in_iv;\n\t}\n\tssl->in_msglen = 0;\n\tssl->in_hslen = 0;\n\n\treturn 0;\n}\n\nstruct wpabuf * tls_connection_handshake(void *tls_ctx,",
        "tls_mbedtls.c temporary input-buffer release helper",
    );
    let patched_text = replace_required(
        &patched_text,
        "\n\treturn 0;\n}\n\nstruct wpabuf * tls_connection_handshake(void *tls_ctx,",
        "\n\treturn 0;\n}\n\nint esp8266_hal_tls_restore_deferred_input(struct tls_connection *conn)\n{\n\tint ret;\n\n\tif (conn == NULL || conn->tls == NULL)\n\t\treturn 0;\n\tif (!conn->tls_io_data.in_buf_restore_pending)\n\t\treturn 0;\n\n\tret = esp8266_hal_tls_restore_in_buf_after_client_flight(&conn->tls->ssl, 1);\n\tif (ret == 0)\n\t\tconn->tls_io_data.in_buf_restore_pending = 0;\n\n\treturn ret;\n}\n\nstruct wpabuf * tls_connection_handshake(void *tls_ctx,",
        "tls_mbedtls.c exported deferred input restore",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tssl->in_buf = mbedtls_calloc(1, MBEDTLS_SSL_IN_BUFFER_LEN);\n\tif (ssl->in_buf == NULL)\n\t\treturn MBEDTLS_ERR_SSL_ALLOC_FAILED;",
        "\t{\n\t\textern void esp8266_hal_eap_rsa_release(void);\n\t\tesp8266_hal_eap_rsa_release();\n\t}\n\tssl->in_buf = mbedtls_calloc(1, MBEDTLS_SSL_IN_BUFFER_LEN);\n\tif (ssl->in_buf == NULL)\n\t\treturn MBEDTLS_ERR_SSL_ALLOC_FAILED;",
        "tls_mbedtls.c release EAP reserve before deferred input restore",
    );
    let patched_text = replace_required(
        &patched_text,
        "\t/* Enable SHA1 support since it's not enabled by default in mbedtls */\n\ttls_enable_sha1_config(tls);",
        "\t/* Enable SHA1 support since it's not enabled by default in mbedtls */\n\ttls_enable_sha1_config(tls);\n#if defined(MBEDTLS_SSL_CBC_RECORD_SPLITTING)\n\tmbedtls_ssl_conf_cbc_record_splitting(&tls->conf,\n\t\t\t\t\tMBEDTLS_SSL_CBC_RECORD_SPLITTING_DISABLED);\n#endif",
        "tls_mbedtls.c disable EAP CBC record splitting",
    );
    let patched_text = replace_required(
        &patched_text,
        "\ttls_context_t *tls = conn->tls;\n\tint ret = 0;\n\n\t/* data freed by sender */",
        "\ttls_context_t *tls = conn->tls;\n\tint ret = 0;\n\tint esp8266_hal_eap_server_input_reserved = 0;\n\n\t/* data freed by sender */",
        "tls_mbedtls.c server input reserve flag",
    );
    let patched_text = replace_required(
        &patched_text,
        "\t/* data freed by sender */\n\tconn->tls_io_data.out_data = NULL;\n\tif (wpabuf_len(in_data)) {\n\t\tconn->tls_io_data.in_data = wpabuf_dup(in_data);\n\t}\n\tret = mbedtls_ssl_handshake_step(&tls->ssl);",
        "\t/* data freed by sender */\n\tconn->tls_io_data.out_data = NULL;\n\twpabuf_free(conn->tls_io_data.in_data);\n\tconn->tls_io_data.in_data = NULL;\n\tconn->tls_io_data.in_ext_data = NULL;\n\tconn->tls_io_data.in_ext_len = 0;\n\tconn->tls_io_data.in_ext_pos = 0;\n\tif (wpabuf_len(in_data)) {\n\t\textern void esp8266_hal_eap_rsa_reserve(void);\n\t\tesp8266_hal_eap_rsa_reserve();\n\t\tesp8266_hal_eap_server_input_reserved = 1;\n\t\tconn->tls_io_data.in_ext_data = wpabuf_head_u8(in_data);\n\t\tconn->tls_io_data.in_ext_len = wpabuf_len(in_data);\n\t\tconn->tls_io_data.in_ext_pos = 0;\n\t}\n\t{\n\t\textern void esp8266_hal_alloc_context_set(unsigned int context);\n\t\textern void esp8266_hal_alloc_context_clear(void);\n\t\tesp8266_hal_alloc_context_set(0x2001u);\n\t\tret = mbedtls_ssl_handshake_step(&tls->ssl);\n\t\tesp8266_hal_alloc_context_clear();\n\t\tif (ret >= 0)\n\t\t\tesp8266_hal_tls_drop_verified_ca(tls);\n\t}",
        "tls_mbedtls.c handshake external input",
    );
    let patched_text = replace_required(
        &patched_text,
        "\twhile (conn->tls_io_data.in_data) {\n\t\tret = mbedtls_ssl_handshake_step(&tls->ssl);\n\t\tif (ret < 0)\n\t\t\tbreak;\n\t}",
        "\twhile (conn->tls_io_data.in_data || conn->tls_io_data.in_ext_data) {\n\t\textern void esp8266_hal_alloc_context_set(unsigned int context);\n\t\textern void esp8266_hal_alloc_context_clear(void);\n\t\tesp8266_hal_alloc_context_set(0x2002u);\n\t\tret = mbedtls_ssl_handshake_step(&tls->ssl);\n\t\tesp8266_hal_alloc_context_clear();\n\t\tif (ret < 0)\n\t\t\tbreak;\n\t\tesp8266_hal_tls_drop_verified_ca(tls);\n\t}",
        "tls_mbedtls.c input drain external input",
    );
    let patched_text = replace_required(
        &patched_text,
        "\twhile (conn->tls_io_data.in_data || conn->tls_io_data.in_ext_data) {",
        "\twhile (conn->tls_io_data.in_data || conn->tls_io_data.in_ext_data ||\n\t       conn->tls_io_data.in_ext_extra_data) {",
        "tls_mbedtls.c input drain extra input",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tif (tls->ssl.state == MBEDTLS_SSL_CLIENT_CERTIFICATE) {",
        "\tif (tls->ssl.state == MBEDTLS_SSL_SERVER_HELLO_DONE) {\n\t\tret = mbedtls_ssl_handshake_step(&tls->ssl);\n\t\tif (ret >= 0)\n\t\t\tesp8266_hal_tls_drop_verified_ca(tls);\n\t\tif (ret < 0)\n\t\t\tgoto end;\n\t}\n\n\tif (tls->ssl.state == MBEDTLS_SSL_CLIENT_CERTIFICATE) {",
        "tls_mbedtls.c drive client flight from server hello done",
    );
    let patched_text = replace_required(
        &patched_text,
        "\t\twhile (tls->ssl.state <= MBEDTLS_SSL_CLIENT_FINISHED) {\n\t\t\tret = mbedtls_ssl_handshake_step(&tls->ssl);\n\t\t\tif (ret < 0) {\n\t\t\t\tbreak;\n\t\t\t}\n\t\t}",
        "\t\textern void esp8266_hal_eap_rsa_reserve(void);\n\t\textern void esp8266_hal_eap_rsa_release(void);\n\t\textern void esp8266_hal_alloc_context_set(unsigned int context);\n\t\textern void esp8266_hal_alloc_context_clear(void);\n\t\tint esp8266_hal_client_flight_in_buf_released;\n\t\tint esp8266_hal_client_flight_restore_ret;\n\t\tesp8266_hal_client_flight_in_buf_released =\n\t\t\tesp8266_hal_tls_release_in_buf_for_client_flight(&tls->ssl);\n\t\tif (tls->clientcert.raw.p != NULL) {\n\t\t\tret = esp8266_hal_tls_prealloc_client_flight_out(conn);\n\t\t\tif (ret != 0)\n\t\t\t\tgoto esp8266_hal_client_flight_done;\n\t\t}\n\t\tesp8266_hal_eap_rsa_reserve();\n\t\twhile (tls->ssl.state <= MBEDTLS_SSL_CLIENT_FINISHED) {\n\t\t\tesp8266_hal_alloc_context_set(0x2003u);\n\t\t\tret = mbedtls_ssl_handshake_step(&tls->ssl);\n\t\t\tesp8266_hal_alloc_context_clear();\n\t\t\tif (ret < 0) {\n\t\t\t\tbreak;\n\t\t\t}\n\t\t}\n\t\tesp8266_hal_eap_rsa_release();\n\tesp8266_hal_client_flight_done:\n\t\tesp8266_hal_client_flight_restore_ret =\n\t\t\tesp8266_hal_tls_restore_in_buf_after_client_flight(\n\t\t\t\t&tls->ssl, esp8266_hal_client_flight_in_buf_released);\n\t\tif (esp8266_hal_client_flight_restore_ret != 0 && ret == 0)\n\t\t\tret = esp8266_hal_client_flight_restore_ret;",
        "tls_mbedtls.c client flight reserve",
    );
    let patched_text = replace_required(
        &patched_text,
        "\t\tint esp8266_hal_client_flight_in_buf_released;\n\t\tint esp8266_hal_client_flight_restore_ret;\n\t\tesp8266_hal_client_flight_in_buf_released =\n\t\t\tesp8266_hal_tls_release_in_buf_for_client_flight(&tls->ssl);\n",
        "\t\tint esp8266_hal_client_flight_in_buf_released;\n\t\tesp8266_hal_client_flight_in_buf_released =\n\t\t\tesp8266_hal_tls_release_in_buf_for_client_flight(&tls->ssl);\n\t\tif (esp8266_hal_client_flight_in_buf_released)\n\t\t\tconn->tls_io_data.in_buf_restore_pending = 1;\n",
        "tls_mbedtls.c mark deferred input restore",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tesp8266_hal_client_flight_done:\n\t\tesp8266_hal_client_flight_restore_ret =\n\t\t\tesp8266_hal_tls_restore_in_buf_after_client_flight(\n\t\t\t\t&tls->ssl, esp8266_hal_client_flight_in_buf_released);\n\t\tif (esp8266_hal_client_flight_restore_ret != 0 && ret == 0)\n\t\t\tret = esp8266_hal_client_flight_restore_ret;",
        "\tesp8266_hal_client_flight_done:\n\t\t(void)esp8266_hal_client_flight_in_buf_released;",
        "tls_mbedtls.c defer client-flight input restore",
    );
    let patched_text = replace_required(
        &patched_text,
        "\t\twhile (tls->ssl.state <= MBEDTLS_SSL_HANDSHAKE_OVER) {\n\t\t\tret = mbedtls_ssl_handshake_step(&tls->ssl);\n\t\t\tif (ret < 0) {\n\t\t\t\tbreak;\n\t\t\t}\n\t\t}",
        "\t\twhile (tls->ssl.state < MBEDTLS_SSL_HANDSHAKE_OVER) {\n\t\t\textern void esp8266_hal_alloc_context_set(unsigned int context);\n\t\t\textern void esp8266_hal_alloc_context_clear(void);\n\t\t\tesp8266_hal_alloc_context_set(0x2005u);\n\t\t\tret = mbedtls_ssl_handshake_step(&tls->ssl);\n\t\t\tesp8266_hal_alloc_context_clear();\n\t\t\tif (ret < 0) {\n\t\t\t\tbreak;\n\t\t\t}\n\t\t}",
        "tls_mbedtls.c flush handshake without overstep",
    );
    let patched_text = replace_required(
        &patched_text,
        "end:\n\treturn conn->tls_io_data.out_data;\n}",
        "end:\n\tif (esp8266_hal_eap_server_input_reserved) {\n\t\textern void esp8266_hal_eap_rsa_release(void);\n\t\tesp8266_hal_eap_rsa_release();\n\t}\n\tconn->tls_io_data.in_ext_data = NULL;\n\tconn->tls_io_data.in_ext_len = 0;\n\tconn->tls_io_data.in_ext_pos = 0;\n\twpabuf_free(conn->tls_io_data.in_ext_owner);\n\tconn->tls_io_data.in_ext_owner = NULL;\n\treturn conn->tls_io_data.out_data;\n}",
        "tls_mbedtls.c handshake clear external input",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tconn->tls_io_data.in_ext_owner = NULL;\n\treturn conn->tls_io_data.out_data;",
        "\tconn->tls_io_data.in_ext_owner = NULL;\n\tconn->tls_io_data.in_ext_extra_data = NULL;\n\tconn->tls_io_data.in_ext_extra_len = 0;\n\tconn->tls_io_data.in_ext_extra_pos = 0;\n\treturn conn->tls_io_data.out_data;",
        "tls_mbedtls.c handshake clear extra input",
    );
    let patched_text = replace_required(
        &patched_text,
        "struct wpabuf * tls_connection_decrypt(void *tls_ctx,\n\t\tstruct tls_connection *conn,\n\t\tconst struct wpabuf *in_data)\n{\n\tunsigned char buf[1200];\n\tint ret;\n\tconn->tls_io_data.in_data = wpabuf_dup(in_data);\n\tret = mbedtls_ssl_read(&conn->tls->ssl, buf, 1200);\n\tif (ret < 0) {\n\t\twpa_printf(MSG_ERROR, \"%s:%d, not able to write whole data\",\n\t\t\t\t__func__, __LINE__);\n\t\treturn NULL;\n\t}\n\n\tstruct wpabuf *out = wpabuf_alloc_copy(buf, ret);\n\n\treturn out;\n}",
        "struct wpabuf * tls_connection_decrypt(void *tls_ctx,\n\t\tstruct tls_connection *conn,\n\t\tconst struct wpabuf *in_data)\n{\n\tunsigned char buf[1200];\n\tstruct wpabuf *out;\n\tint ret;\n\twpabuf_free(conn->tls_io_data.in_data);\n\tconn->tls_io_data.in_data = wpabuf_dup(in_data);\n\tconn->tls_io_data.in_ext_data = NULL;\n\tconn->tls_io_data.in_ext_len = 0;\n\tconn->tls_io_data.in_ext_pos = 0;\n\tif (!conn->tls_io_data.in_data) {\n\t\treturn NULL;\n\t}\n\tret = mbedtls_ssl_read(&conn->tls->ssl, buf, 1200);\n\tif (ret < 0) {\n\t\twpa_printf(MSG_ERROR, \"%s:%d, not able to write whole data\",\n\t\t\t\t__func__, __LINE__);\n\t\treturn NULL;\n\t}\n\n\tout = wpabuf_alloc_copy(buf, (size_t)ret);\n\n\treturn out;\n}",
        "tls_mbedtls.c decrypt input ownership",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tconn->tls_io_data.in_ext_pos = 0;\n\tif (!conn->tls_io_data.in_data) {",
        "\tconn->tls_io_data.in_ext_pos = 0;\n\tconn->tls_io_data.in_ext_extra_data = NULL;\n\tconn->tls_io_data.in_ext_extra_len = 0;\n\tconn->tls_io_data.in_ext_extra_pos = 0;\n\tif (!conn->tls_io_data.in_data) {",
        "tls_mbedtls.c decrypt clear extra input",
    );
    fs::write(&patched, patched_text).expect("failed to write patched tls_mbedtls.c");
    patched
}

fn patch_tls_mbedtls_eap_trace_source(source: &Path, out_dir: &Path) -> PathBuf {
    let patched = out_dir.join("tls_mbedtls_eap_trace.c");
    let source_text = fs::read_to_string(source).expect("failed to read tls_mbedtls.c");
    let patched_text = replace_required(
        &source_text,
        "#define MAX_CIPHERSUITE 32",
        &format!("#define MAX_CIPHERSUITE 32\n{}", eap_trace_hook()),
        "tls_mbedtls.c EAP trace hook",
    );
    let patched_text = replace_required(
        &patched_text,
        "struct tls_data {\n\t/* Data for mbedlts */\n\tstruct wpabuf *in_data;\n\t/* Data from mbedtls */\n\tstruct wpabuf *out_data;\n};",
        "struct tls_data {\n\t/* Data for mbedlts */\n\tstruct wpabuf *in_data;\n\tconst u8 *in_ext_data;\n\tsize_t in_ext_len;\n\tsize_t in_ext_pos;\n\tstruct wpabuf *in_ext_owner;\n\tconst u8 *in_ext_extra_data;\n\tsize_t in_ext_extra_len;\n\tsize_t in_ext_extra_pos;\n\t/* Data from mbedtls */\n\tstruct wpabuf *out_data;\n};",
        "tls_mbedtls.c external input cursor fields",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tsize_t in_ext_extra_pos;\n\t/* Data from mbedtls */",
        "\tsize_t in_ext_extra_pos;\n\tint in_buf_restore_pending;\n\t/* Data from mbedtls */",
        "tls_mbedtls.c deferred input-buffer restore flag trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "struct tls_connection {\n\ttls_context_t *tls;\n\tstruct tls_data tls_io_data;\n\tunsigned char randbytes[2 * TLS_RANDOM_LEN];\n};",
        "struct tls_connection {\n\ttls_context_t *tls;\n\tstruct tls_data tls_io_data;\n\tunsigned char randbytes[2 * TLS_RANDOM_LEN];\n};\n\nvoid esp8266_hal_tls_set_extra_input(struct tls_connection *conn,\n\t\t\t\t\t     const u8 *data, size_t len)\n{\n\tif (conn == NULL)\n\t\treturn;\n\tconn->tls_io_data.in_ext_extra_data = data;\n\tconn->tls_io_data.in_ext_extra_len = len;\n\tconn->tls_io_data.in_ext_extra_pos = 0;\n}\n\nvoid esp8266_hal_tls_set_input_owner(struct tls_connection *conn,\n\t\t\t\t\t     struct wpabuf *owner)\n{\n\tif (conn == NULL)\n\t\treturn;\n\twpabuf_free(conn->tls_io_data.in_ext_owner);\n\tconn->tls_io_data.in_ext_owner = owner;\n}",
        "tls_mbedtls.c external input extra fragment setter trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "static int set_pki_context(tls_context_t *tls, const struct tls_connection_params *cfg)",
        "static int esp8266_hal_x509_blob_is_pem(const unsigned char *blob, size_t len)\n{\n\treturn len >= 10 && os_memcmp(blob, \"-----BEGIN\", 10) == 0;\n}\n\nstatic int esp8266_hal_x509_crt_parse_blob(mbedtls_x509_crt *crt,\n\t\t\t\t\t    const unsigned char *blob,\n\t\t\t\t\t    size_t len)\n{\n\tif (esp8266_hal_x509_blob_is_pem(blob, len))\n\t\treturn mbedtls_x509_crt_parse(crt, blob, len);\n\n\treturn mbedtls_x509_crt_parse_der(crt, blob, len);\n}\n\nstatic void esp8266_hal_tls_drop_verified_ca(tls_context_t *tls)\n{\n\tif (tls == NULL || tls->cacert_ptr == NULL)\n\t\treturn;\n\tif (tls->ssl.conf == NULL || tls->ssl.conf->endpoint != MBEDTLS_SSL_IS_CLIENT)\n\t\treturn;\n\tif (tls->ssl.state <= MBEDTLS_SSL_SERVER_CERTIFICATE)\n\t\treturn;\n\n\tmbedtls_ssl_conf_ca_chain(&tls->conf, NULL, NULL);\n\tmbedtls_x509_crt_free(&tls->cacert);\n\ttls->cacert_ptr = NULL;\n}\n\nstatic int set_pki_context(tls_context_t *tls, const struct tls_connection_params *cfg)",
        "tls_mbedtls.c DER/PEM certificate parser",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tret = mbedtls_x509_crt_parse(&tls->clientcert,\n\t\t\t\t     cfg->client_cert_blob, cfg->client_cert_blob_len);",
        "\tret = esp8266_hal_x509_crt_parse_blob(&tls->clientcert,\n\t\t\t\t\t\t cfg->client_cert_blob,\n\t\t\t\t\t\t cfg->client_cert_blob_len);",
        "tls_mbedtls.c parse client cert DER/PEM blob",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tint ret = mbedtls_x509_crt_parse(tls->cacert_ptr, cacert, cacert_len);",
        "\tint ret = esp8266_hal_x509_crt_parse_blob(tls->cacert_ptr,\n\t\t\t\t\t\t  cacert, cacert_len);",
        "tls_mbedtls.c parse CA cert DER/PEM blob",
    );
    let patched_text = replace_required(
        &patched_text,
        "static int tls_mbedtls_read(void *ctx, unsigned char *buf, size_t len)\n{\n\tstruct tls_connection *conn = (struct tls_connection *)ctx;\n\tstruct tls_data *data = &conn->tls_io_data;\n\tstruct wpabuf *local_buf;\n\tsize_t data_len = len;\n\n\tif (len > wpabuf_len(data->in_data)) {\n\t\twpa_printf(MSG_ERROR, \"don't have suffient data\\n\");\n\t\tdata_len = wpabuf_len(data->in_data);\n\t}\n\n\tos_memcpy(buf, wpabuf_head(data->in_data), data_len);\n\t/* adjust buffer */\n\tif (len < wpabuf_len(data->in_data)) {\n\t\tlocal_buf = wpabuf_alloc_copy(wpabuf_head(data->in_data) + len,\n\t\t\t\t\t      wpabuf_len(data->in_data) - len);\n\t\twpabuf_free(data->in_data);\n\t\tdata->in_data = local_buf;\n\t} else {\n\t\twpabuf_free(data->in_data);\n\t\tdata->in_data = NULL;\n\t}\n\n\treturn data_len;\n}",
        r#"static int tls_mbedtls_read(void *ctx, unsigned char *buf, size_t len)
{
	struct tls_connection *conn = (struct tls_connection *)ctx;
	struct tls_data *data = &conn->tls_io_data;
	struct wpabuf *local_buf;
	size_t copied = 0;

	while (copied < len) {
		size_t available;
		size_t chunk;

		if (data->in_ext_data) {
			available = data->in_ext_len - data->in_ext_pos;
			chunk = len - copied;
			if (chunk > available)
				chunk = available;
			os_memcpy(buf + copied, data->in_ext_data + data->in_ext_pos,
				  chunk);
			copied += chunk;
			data->in_ext_pos += chunk;
			if (data->in_ext_pos >= data->in_ext_len) {
				data->in_ext_data = NULL;
				data->in_ext_len = 0;
				data->in_ext_pos = 0;
				wpabuf_free(data->in_ext_owner);
				data->in_ext_owner = NULL;
			}
			continue;
		}

		if (data->in_ext_extra_data) {
			available = data->in_ext_extra_len -
				    data->in_ext_extra_pos;
			chunk = len - copied;
			if (chunk > available)
				chunk = available;
			os_memcpy(buf + copied,
				  data->in_ext_extra_data +
				  data->in_ext_extra_pos,
				  chunk);
			copied += chunk;
			data->in_ext_extra_pos += chunk;
			if (data->in_ext_extra_pos >= data->in_ext_extra_len) {
				data->in_ext_extra_data = NULL;
				data->in_ext_extra_len = 0;
				data->in_ext_extra_pos = 0;
			}
			continue;
		}

		break;
	}

	if (copied == len)
		return copied;

	if (!data->in_data) {
		if (copied > 0) {
			wpa_printf(MSG_ERROR, "don't have suffient data\n");
			return copied;
		}
		return MBEDTLS_ERR_SSL_WANT_READ;
	}

	if (len - copied > wpabuf_len(data->in_data)) {
		wpa_printf(MSG_ERROR, "don't have suffient data\n");
		len = copied + wpabuf_len(data->in_data);
	}

	if (len > copied) {
		size_t data_len = len - copied;
		const u8 *head = wpabuf_head_u8(data->in_data);

		if (data_len < wpabuf_len(data->in_data)) {
			local_buf = wpabuf_alloc_copy(head + data_len,
						      wpabuf_len(data->in_data) -
						      data_len);
			if (local_buf == NULL)
				return copied > 0 ? copied : MBEDTLS_ERR_SSL_ALLOC_FAILED;
			os_memcpy(buf + copied, head, data_len);
			wpabuf_free(data->in_data);
			data->in_data = local_buf;
		} else {
			os_memcpy(buf + copied, head, data_len);
			wpabuf_free(data->in_data);
			data->in_data = NULL;
		}
		copied += data_len;
	}

	return copied;
}"#,
        "tls_mbedtls.c external input cursor read",
    );
    let patched_text = replace_required(
        &patched_text,
        "static int tls_mbedtls_write(void *ctx, const unsigned char *buf, size_t len)\n{\n\tstruct tls_connection *conn = (struct tls_connection *)ctx;\n\tstruct tls_data *data = &conn->tls_io_data;\n\n\tif (data->out_data) {\n\t\twpabuf_resize(&data->out_data, len);\n\t} else {\n\t\tdata->out_data = wpabuf_alloc(len);\n\t}\n\n\twpabuf_put_data(data->out_data, buf, len);\n\n\treturn len;\n}",
        "#define ESP8266_HAL_TLS_CLIENT_FLIGHT_OUT_PREALLOC_BYTES 704\n\nstatic int esp8266_hal_tls_prealloc_client_flight_out(struct tls_connection *conn)\n{\n\tstruct tls_data *data = &conn->tls_io_data;\n\textern void esp8266_hal_alloc_context_set(unsigned int context);\n\textern void esp8266_hal_alloc_context_clear(void);\n\n\tif (data->out_data != NULL)\n\t\treturn 0;\n\n\tesp8266_hal_eap_trace_code('~');\n\tesp8266_hal_eap_trace_code('y');\n\tesp8266_hal_alloc_context_set(0x2006u);\n\tdata->out_data = wpabuf_alloc(ESP8266_HAL_TLS_CLIENT_FLIGHT_OUT_PREALLOC_BYTES);\n\tesp8266_hal_alloc_context_clear();\n\tesp8266_hal_eap_trace_code(data->out_data ? 'Y' : 'y');\n\n\treturn data->out_data == NULL ? MBEDTLS_ERR_SSL_ALLOC_FAILED : 0;\n}\n\nstatic int tls_mbedtls_write(void *ctx, const unsigned char *buf, size_t len)\n{\n\tstruct tls_connection *conn = (struct tls_connection *)ctx;\n\tstruct tls_data *data = &conn->tls_io_data;\n\tesp8266_hal_eap_trace_code('w');\n\tesp8266_hal_eap_trace_hex32((unsigned int)len);\n\n\tif (data->out_data) {\n\t\tif (wpabuf_resize(&data->out_data, len) < 0) {\n\t\t\tesp8266_hal_eap_trace_code('W');\n\t\t\treturn MBEDTLS_ERR_SSL_ALLOC_FAILED;\n\t\t}\n\t} else {\n\t\tdata->out_data = wpabuf_alloc(len);\n\t}\n\tif (!data->out_data) {\n\t\tesp8266_hal_eap_trace_code('W');\n\t\treturn MBEDTLS_ERR_SSL_ALLOC_FAILED;\n\t}\n\n\twpabuf_put_data(data->out_data, buf, len);\n\n\treturn len;\n}",
        "tls_mbedtls.c write allocation trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "struct wpabuf * tls_connection_handshake(void *tls_ctx,",
        "static int esp8266_hal_tls_release_in_buf_for_client_flight(mbedtls_ssl_context *ssl)\n{\n\tif (ssl->conf == NULL || ssl->conf->endpoint != MBEDTLS_SSL_IS_CLIENT)\n\t\treturn 0;\n\n\tif (ssl->in_buf == NULL || ssl->in_offt != NULL || ssl->in_left != 0)\n\t\treturn 0;\n\n\tif (ssl->in_hslen != 0 && ssl->in_hslen < ssl->in_msglen)\n\t\treturn 0;\n\n\tos_memset(ssl->in_buf, 0, MBEDTLS_SSL_IN_BUFFER_LEN);\n\tmbedtls_free(ssl->in_buf);\n\tssl->in_buf = NULL;\n\tssl->in_hdr = NULL;\n\tssl->in_ctr = NULL;\n\tssl->in_len = NULL;\n\tssl->in_iv = NULL;\n\tssl->in_msg = NULL;\n\tssl->in_msglen = 0;\n\tssl->in_hslen = 0;\n\n\treturn 1;\n}\n\nstatic int esp8266_hal_tls_restore_in_buf_after_client_flight(mbedtls_ssl_context *ssl,\n\t\t\t\t\t\t\t      int released)\n{\n\tif (!released)\n\t\treturn 0;\n\n\tssl->in_buf = mbedtls_calloc(1, MBEDTLS_SSL_IN_BUFFER_LEN);\n\tif (ssl->in_buf == NULL)\n\t\treturn MBEDTLS_ERR_SSL_ALLOC_FAILED;\n\n\tssl->in_left = 0;\n#if defined(MBEDTLS_SSL_PROTO_DTLS)\n\tssl->next_record_offset = 0;\n\tif (ssl->conf->transport == MBEDTLS_SSL_TRANSPORT_DATAGRAM) {\n\t\tssl->in_hdr = ssl->in_buf;\n\t\tssl->in_ctr = ssl->in_hdr + 3;\n\t\tssl->in_len = ssl->in_hdr + 11;\n\t\tssl->in_iv = ssl->in_hdr + 13;\n\t\tssl->in_msg = ssl->in_iv;\n\t} else\n#endif\n\t{\n\t\tssl->in_hdr = ssl->in_buf + 8;\n\t\tssl->in_ctr = ssl->in_hdr - 8;\n\t\tssl->in_len = ssl->in_hdr + 3;\n\t\tssl->in_iv = ssl->in_hdr + 5;\n\t\tssl->in_msg = ssl->in_iv;\n\t}\n\tssl->in_msglen = 0;\n\tssl->in_hslen = 0;\n\n\treturn 0;\n}\n\nstruct wpabuf * tls_connection_handshake(void *tls_ctx,",
        "tls_mbedtls.c temporary input-buffer release helper trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\n\treturn 0;\n}\n\nstruct wpabuf * tls_connection_handshake(void *tls_ctx,",
        "\n\treturn 0;\n}\n\nint esp8266_hal_tls_restore_deferred_input(struct tls_connection *conn)\n{\n\tint ret;\n\n\tif (conn == NULL || conn->tls == NULL)\n\t\treturn 0;\n\tif (!conn->tls_io_data.in_buf_restore_pending)\n\t\treturn 0;\n\n\tesp8266_hal_eap_trace_code('?');\n\tret = esp8266_hal_tls_restore_in_buf_after_client_flight(&conn->tls->ssl, 1);\n\tif (ret == 0)\n\t\tconn->tls_io_data.in_buf_restore_pending = 0;\n\tesp8266_hal_eap_trace_code(ret == 0 ? 'I' : 'i');\n\n\treturn ret;\n}\n\nstruct wpabuf * tls_connection_handshake(void *tls_ctx,",
        "tls_mbedtls.c exported deferred input restore trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tssl->in_buf = mbedtls_calloc(1, MBEDTLS_SSL_IN_BUFFER_LEN);\n\tif (ssl->in_buf == NULL)\n\t\treturn MBEDTLS_ERR_SSL_ALLOC_FAILED;",
        "\t{\n\t\textern void esp8266_hal_eap_rsa_release(void);\n\t\tesp8266_hal_eap_rsa_release();\n\t}\n\tssl->in_buf = mbedtls_calloc(1, MBEDTLS_SSL_IN_BUFFER_LEN);\n\tif (ssl->in_buf == NULL)\n\t\treturn MBEDTLS_ERR_SSL_ALLOC_FAILED;",
        "tls_mbedtls.c release EAP reserve before deferred input restore trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tret = mbedtls_ssl_config_defaults(&tls->conf,\n\t\t\t\t\tMBEDTLS_SSL_IS_CLIENT,\n\t\t\t\t\tMBEDTLS_SSL_TRANSPORT_STREAM,\n\t\t\t\t\tMBEDTLS_SSL_PRESET_DEFAULT);\n\tif (ret != 0) {\n\t\twpa_printf(MSG_ERROR, \"mbedtls_ssl_config_defaults returned -0x%x\", -ret);\n\t\treturn ret;\n\t}",
        "\tesp8266_hal_eap_trace_code('D');\n\tret = mbedtls_ssl_config_defaults(&tls->conf,\n\t\t\t\t\tMBEDTLS_SSL_IS_CLIENT,\n\t\t\t\t\tMBEDTLS_SSL_TRANSPORT_STREAM,\n\t\t\t\t\tMBEDTLS_SSL_PRESET_DEFAULT);\n\tif (ret != 0) {\n\t\tesp8266_hal_eap_trace_code('d');\n\t\tesp8266_hal_eap_trace_hex32((unsigned int)ret);\n\t\twpa_printf(MSG_ERROR, \"mbedtls_ssl_config_defaults returned -0x%x\", -ret);\n\t\treturn ret;\n\t}\n\tesp8266_hal_eap_trace_code('P');",
        "tls_mbedtls.c config defaults trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\t/* Enable SHA1 support since it's not enabled by default in mbedtls */\n\ttls_enable_sha1_config(tls);",
        "\t/* Enable SHA1 support since it's not enabled by default in mbedtls */\n\ttls_enable_sha1_config(tls);\n#if defined(MBEDTLS_SSL_CBC_RECORD_SPLITTING)\n\tesp8266_hal_eap_trace_code('m');\n\tmbedtls_ssl_conf_cbc_record_splitting(&tls->conf,\n\t\t\t\t\tMBEDTLS_SSL_CBC_RECORD_SPLITTING_DISABLED);\n#endif",
        "tls_mbedtls.c disable EAP CBC record splitting",
    );
    let patched_text = replace_required(
        &patched_text,
        "\ttls_set_ciphersuite(tls);\n\n\treturn 0;\n}\n\nstatic int tls_create_mbedtls_handle",
        "\ttls_set_ciphersuite(tls);\n\tesp8266_hal_eap_trace_code('Q');\n\n\treturn 0;\n}\n\nstatic int tls_create_mbedtls_handle",
        "tls_mbedtls.c client config success trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tmbedtls_ssl_init(&tls->ssl);\n\tmbedtls_ctr_drbg_init(&tls->ctr_drbg);\n\tmbedtls_ssl_config_init(&tls->conf);\n\tmbedtls_entropy_init(&tls->entropy);\n\n\tret = set_client_config(params, tls);\n\tif (ret != 0) {\n\t\twpa_printf(MSG_ERROR, \"Failed to set client configurations\");\n\t\tgoto exit;\n\t}\n\n\tret = mbedtls_ctr_drbg_seed(&tls->ctr_drbg, mbedtls_entropy_func,\n\t\t\t\t    &tls->entropy, NULL, 0);\n\tif (ret != 0) {\n\t\twpa_printf(MSG_ERROR, \"mbedtls_ctr_drbg_seed returned -0x%x\", -ret);\n\t\tgoto exit;\n\t}\n\n\tmbedtls_ssl_conf_rng(&tls->conf, mbedtls_ctr_drbg_random, &tls->ctr_drbg);\n\n\tret = mbedtls_ssl_setup(&tls->ssl, &tls->conf);\n\tif (ret != 0) {\n\t\twpa_printf(MSG_ERROR, \"mbedtls_ssl_setup returned -0x%x\", -ret);\n\t\tgoto exit;\n\t}",
        "\tmbedtls_ssl_init(&tls->ssl);\n\tmbedtls_ctr_drbg_init(&tls->ctr_drbg);\n\tmbedtls_ssl_config_init(&tls->conf);\n\tmbedtls_entropy_init(&tls->entropy);\n\tesp8266_hal_eap_trace_code('G');\n\n\tesp8266_hal_eap_trace_code('A');\n\tret = set_client_config(params, tls);\n\tif (ret != 0) {\n\t\tesp8266_hal_eap_trace_code('a');\n\t\tesp8266_hal_eap_trace_hex32((unsigned int)ret);\n\t\twpa_printf(MSG_ERROR, \"Failed to set client configurations\");\n\t\tgoto exit;\n\t}\n\tesp8266_hal_eap_trace_code('B');\n\n\tesp8266_hal_eap_trace_code('E');\n\tret = mbedtls_ctr_drbg_seed(&tls->ctr_drbg, mbedtls_entropy_func,\n\t\t\t\t    &tls->entropy, NULL, 0);\n\tif (ret != 0) {\n\t\tesp8266_hal_eap_trace_code('e');\n\t\tesp8266_hal_eap_trace_hex32((unsigned int)ret);\n\t\twpa_printf(MSG_ERROR, \"mbedtls_ctr_drbg_seed returned -0x%x\", -ret);\n\t\tgoto exit;\n\t}\n\tesp8266_hal_eap_trace_code('F');\n\n\tmbedtls_ssl_conf_rng(&tls->conf, mbedtls_ctr_drbg_random, &tls->ctr_drbg);\n\n\tesp8266_hal_eap_trace_code('U');\n\tret = mbedtls_ssl_setup(&tls->ssl, &tls->conf);\n\tif (ret != 0) {\n\t\tesp8266_hal_eap_trace_code('u');\n\t\tesp8266_hal_eap_trace_hex32((unsigned int)ret);\n\t\twpa_printf(MSG_ERROR, \"mbedtls_ssl_setup returned -0x%x\", -ret);\n\t\tgoto exit;\n\t}\n\tesp8266_hal_eap_trace_code('V');",
        "tls_mbedtls.c handle creation trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\ttls_context_t *tls = (tls_context_t *)os_zalloc(sizeof(tls_context_t));\n\n\tif (!tls) {\n\t\twpa_printf(MSG_ERROR, \"failed to allocate tls context\");\n\t\treturn -1;\n\t}\n\tif (!params) {\n\t\twpa_printf(MSG_ERROR, \"configuration is null\");\n\t\tret = -1;\n\t\tgoto err;\n\t}\n\n\tret = tls_create_mbedtls_handle(params, tls);\n\tif (ret < 0) {\n\t\twpa_printf(MSG_ERROR, \"failed to create ssl handle\");\n\t\tgoto err;\n\t}\n\tmbedtls_ssl_set_bio(&tls->ssl, conn, tls_mbedtls_write, tls_mbedtls_read, NULL);\n\tconn->tls = (tls_context_t *)tls;\n\n\treturn ret;\nerr:\n\tos_free(tls);\n\treturn ret;",
        "\tesp8266_hal_eap_trace_code('X');\n\ttls_context_t *tls = (tls_context_t *)os_zalloc(sizeof(tls_context_t));\n\n\tif (!tls) {\n\t\tesp8266_hal_eap_trace_code('x');\n\t\twpa_printf(MSG_ERROR, \"failed to allocate tls context\");\n\t\treturn -1;\n\t}\n\tif (!params) {\n\t\tesp8266_hal_eap_trace_code('p');\n\t\twpa_printf(MSG_ERROR, \"configuration is null\");\n\t\tret = -1;\n\t\tgoto err;\n\t}\n\n\tesp8266_hal_eap_trace_code('Y');\n\tret = tls_create_mbedtls_handle(params, tls);\n\tif (ret < 0) {\n\t\tesp8266_hal_eap_trace_code('y');\n\t\tesp8266_hal_eap_trace_hex32((unsigned int)ret);\n\t\twpa_printf(MSG_ERROR, \"failed to create ssl handle\");\n\t\tgoto err;\n\t}\n\tesp8266_hal_eap_trace_code('Z');\n\tmbedtls_ssl_set_bio(&tls->ssl, conn, tls_mbedtls_write, tls_mbedtls_read, NULL);\n\tconn->tls = (tls_context_t *)tls;\n\n\treturn ret;\nerr:\n\tos_free(tls);\n\treturn ret;",
        "tls_mbedtls.c set params trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\ttls_context_t *tls = conn->tls;\n\tint ret = 0;\n\n\t/* data freed by sender */",
        "\ttls_context_t *tls = conn->tls;\n\tint ret = 0;\n\tint esp8266_hal_eap_server_input_reserved = 0;\n\n\t/* data freed by sender */",
        "tls_mbedtls.c server input reserve flag trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\t/* data freed by sender */\n\tconn->tls_io_data.out_data = NULL;\n\tif (wpabuf_len(in_data)) {\n\t\tconn->tls_io_data.in_data = wpabuf_dup(in_data);\n\t}\n\tret = mbedtls_ssl_handshake_step(&tls->ssl);",
        "\t/* data freed by sender */\n\tconn->tls_io_data.out_data = NULL;\n\twpabuf_free(conn->tls_io_data.in_data);\n\tconn->tls_io_data.in_data = NULL;\n\tconn->tls_io_data.in_ext_data = NULL;\n\tconn->tls_io_data.in_ext_len = 0;\n\tconn->tls_io_data.in_ext_pos = 0;\n\tif (wpabuf_len(in_data)) {\n\t\textern void esp8266_hal_eap_rsa_reserve(void);\n\t\tesp8266_hal_eap_trace_code('[');\n\t\tesp8266_hal_eap_rsa_reserve();\n\t\tesp8266_hal_eap_server_input_reserved = 1;\n\t\tesp8266_hal_eap_trace_code('i');\n\t\tesp8266_hal_eap_trace_hex32((unsigned int)wpabuf_len(in_data));\n\t\tconn->tls_io_data.in_ext_data = wpabuf_head_u8(in_data);\n\t\tconn->tls_io_data.in_ext_len = wpabuf_len(in_data);\n\t\tconn->tls_io_data.in_ext_pos = 0;\n\t}\n\tesp8266_hal_eap_trace_code('h');\n\tret = mbedtls_ssl_handshake_step(&tls->ssl);",
        "tls_mbedtls.c handshake external input",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tesp8266_hal_eap_trace_code('h');\n\tret = mbedtls_ssl_handshake_step(&tls->ssl);",
        "\tesp8266_hal_eap_trace_code('h');\n\tret = mbedtls_ssl_handshake_step(&tls->ssl);\n\tif (ret >= 0)\n\t\tesp8266_hal_tls_drop_verified_ca(tls);\n\tesp8266_hal_eap_trace_code('r');\n\tesp8266_hal_eap_trace_hex32((unsigned int)ret);\n\tesp8266_hal_eap_trace_hex32((unsigned int)tls->ssl.state);",
        "tls_mbedtls.c first handshake step trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\twhile (conn->tls_io_data.in_data) {\n\t\tret = mbedtls_ssl_handshake_step(&tls->ssl);\n\t\tif (ret < 0)\n\t\t\tbreak;\n\t}",
        "\twhile (conn->tls_io_data.in_data || conn->tls_io_data.in_ext_data) {\n\t\tret = mbedtls_ssl_handshake_step(&tls->ssl);\n\t\tif (ret >= 0)\n\t\t\tesp8266_hal_tls_drop_verified_ca(tls);\n\t\tesp8266_hal_eap_trace_code('r');\n\t\tesp8266_hal_eap_trace_hex32((unsigned int)ret);\n\t\tesp8266_hal_eap_trace_hex32((unsigned int)tls->ssl.state);\n\t\tif (ret < 0)\n\t\t\tbreak;\n\t}",
        "tls_mbedtls.c input drain handshake trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\twhile (conn->tls_io_data.in_data || conn->tls_io_data.in_ext_data) {\n\t\tret = mbedtls_ssl_handshake_step(&tls->ssl);",
        "\twhile (conn->tls_io_data.in_data || conn->tls_io_data.in_ext_data ||\n\t       conn->tls_io_data.in_ext_extra_data) {\n\t\tret = mbedtls_ssl_handshake_step(&tls->ssl);",
        "tls_mbedtls.c input drain extra input trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tif (tls->ssl.state == MBEDTLS_SSL_CLIENT_HELLO) {\n\t\tret = mbedtls_ssl_handshake_step(&tls->ssl);\n\t}",
        "\tif (tls->ssl.state == MBEDTLS_SSL_CLIENT_HELLO) {\n\t\tret = mbedtls_ssl_handshake_step(&tls->ssl);\n\t\tesp8266_hal_eap_trace_code('r');\n\t\tesp8266_hal_eap_trace_hex32((unsigned int)ret);\n\t\tesp8266_hal_eap_trace_hex32((unsigned int)tls->ssl.state);\n\t}",
        "tls_mbedtls.c client hello handshake trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tif (tls->ssl.state == MBEDTLS_SSL_CLIENT_CERTIFICATE) {",
        "\tif (tls->ssl.state == MBEDTLS_SSL_SERVER_HELLO_DONE) {\n\t\tret = mbedtls_ssl_handshake_step(&tls->ssl);\n\t\tesp8266_hal_eap_trace_code('r');\n\t\tesp8266_hal_eap_trace_hex32((unsigned int)ret);\n\t\tesp8266_hal_eap_trace_hex32((unsigned int)tls->ssl.state);\n\t\tif (ret >= 0)\n\t\t\tesp8266_hal_tls_drop_verified_ca(tls);\n\t\tif (ret < 0)\n\t\t\tgoto end;\n\t}\n\n\tif (tls->ssl.state == MBEDTLS_SSL_CLIENT_CERTIFICATE) {",
        "tls_mbedtls.c drive client flight from server hello done",
    );
    let patched_text = replace_required(
        &patched_text,
        "\t\twhile (tls->ssl.state <= MBEDTLS_SSL_CLIENT_FINISHED) {\n\t\t\tret = mbedtls_ssl_handshake_step(&tls->ssl);\n\t\t\tif (ret < 0) {\n\t\t\t\tbreak;\n\t\t\t}\n\t\t}",
        "\t\textern void esp8266_hal_eap_rsa_reserve(void);\n\t\textern void esp8266_hal_eap_rsa_release(void);\n\t\tint esp8266_hal_client_flight_in_buf_released;\n\t\tint esp8266_hal_client_flight_restore_ret;\n\t\tesp8266_hal_eap_trace_code('j');\n\t\tesp8266_hal_client_flight_in_buf_released =\n\t\t\tesp8266_hal_tls_release_in_buf_for_client_flight(&tls->ssl);\n\t\tif (tls->clientcert.raw.p != NULL) {\n\t\t\tret = esp8266_hal_tls_prealloc_client_flight_out(conn);\n\t\t\tif (ret != 0)\n\t\t\t\tgoto esp8266_hal_client_flight_done;\n\t\t}\n\t\tesp8266_hal_eap_rsa_reserve();\n\t\twhile (tls->ssl.state <= MBEDTLS_SSL_CLIENT_FINISHED) {\n\t\t\tret = mbedtls_ssl_handshake_step(&tls->ssl);\n\t\t\tesp8266_hal_eap_trace_code('r');\n\t\t\tesp8266_hal_eap_trace_hex32((unsigned int)ret);\n\t\t\tesp8266_hal_eap_trace_hex32((unsigned int)tls->ssl.state);\n\t\t\tif (ret < 0) {\n\t\t\t\tbreak;\n\t\t\t}\n\t\t}\n\t\tesp8266_hal_eap_rsa_release();\n\tesp8266_hal_client_flight_done:\n\t\tesp8266_hal_client_flight_restore_ret =\n\t\t\tesp8266_hal_tls_restore_in_buf_after_client_flight(\n\t\t\t\t&tls->ssl, esp8266_hal_client_flight_in_buf_released);\n\t\tif (esp8266_hal_client_flight_restore_ret != 0 && ret == 0)\n\t\t\tret = esp8266_hal_client_flight_restore_ret;\n\t\tesp8266_hal_eap_trace_code('J');",
        "tls_mbedtls.c client finished handshake trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\t\tint esp8266_hal_client_flight_in_buf_released;\n\t\tint esp8266_hal_client_flight_restore_ret;\n\t\tesp8266_hal_eap_trace_code('j');\n\t\tesp8266_hal_client_flight_in_buf_released =\n\t\t\tesp8266_hal_tls_release_in_buf_for_client_flight(&tls->ssl);\n",
        "\t\tint esp8266_hal_client_flight_in_buf_released;\n\t\tesp8266_hal_eap_trace_code('j');\n\t\tesp8266_hal_client_flight_in_buf_released =\n\t\t\tesp8266_hal_tls_release_in_buf_for_client_flight(&tls->ssl);\n\t\tif (esp8266_hal_client_flight_in_buf_released) {\n\t\t\tconn->tls_io_data.in_buf_restore_pending = 1;\n\t\t\tesp8266_hal_eap_trace_code('k');\n\t\t}\n",
        "tls_mbedtls.c mark deferred input restore trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tesp8266_hal_client_flight_done:\n\t\tesp8266_hal_client_flight_restore_ret =\n\t\t\tesp8266_hal_tls_restore_in_buf_after_client_flight(\n\t\t\t\t&tls->ssl, esp8266_hal_client_flight_in_buf_released);\n\t\tif (esp8266_hal_client_flight_restore_ret != 0 && ret == 0)\n\t\t\tret = esp8266_hal_client_flight_restore_ret;\n\t\tesp8266_hal_eap_trace_code('J');",
        "\tesp8266_hal_client_flight_done:\n\t\t(void)esp8266_hal_client_flight_in_buf_released;\n\t\tesp8266_hal_eap_trace_code('J');",
        "tls_mbedtls.c defer client-flight input restore trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\t\twhile (tls->ssl.state <= MBEDTLS_SSL_HANDSHAKE_OVER) {\n\t\t\tret = mbedtls_ssl_handshake_step(&tls->ssl);\n\t\t\tif (ret < 0) {\n\t\t\t\tbreak;\n\t\t\t}\n\t\t}",
        "\t\twhile (tls->ssl.state < MBEDTLS_SSL_HANDSHAKE_OVER) {\n\t\t\tret = mbedtls_ssl_handshake_step(&tls->ssl);\n\t\t\tesp8266_hal_eap_trace_code('r');\n\t\t\tesp8266_hal_eap_trace_hex32((unsigned int)ret);\n\t\t\tesp8266_hal_eap_trace_hex32((unsigned int)tls->ssl.state);\n\t\t\tif (ret < 0) {\n\t\t\t\tbreak;\n\t\t\t}\n\t\t}",
        "tls_mbedtls.c flush handshake trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "end:\n\treturn conn->tls_io_data.out_data;\n}",
        "end:\n\tif (esp8266_hal_eap_server_input_reserved) {\n\t\textern void esp8266_hal_eap_rsa_release(void);\n\t\tesp8266_hal_eap_rsa_release();\n\t\tesp8266_hal_eap_trace_code(']');\n\t}\n\tconn->tls_io_data.in_ext_data = NULL;\n\tconn->tls_io_data.in_ext_len = 0;\n\tconn->tls_io_data.in_ext_pos = 0;\n\twpabuf_free(conn->tls_io_data.in_ext_owner);\n\tconn->tls_io_data.in_ext_owner = NULL;\n\tesp8266_hal_eap_trace_code(conn->tls_io_data.out_data ? 'o' : 'O');\n\treturn conn->tls_io_data.out_data;\n}",
        "tls_mbedtls.c handshake output trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tconn->tls_io_data.in_ext_owner = NULL;\n\tesp8266_hal_eap_trace_code(conn->tls_io_data.out_data ? 'o' : 'O');",
        "\tconn->tls_io_data.in_ext_owner = NULL;\n\tconn->tls_io_data.in_ext_extra_data = NULL;\n\tconn->tls_io_data.in_ext_extra_len = 0;\n\tconn->tls_io_data.in_ext_extra_pos = 0;\n\tesp8266_hal_eap_trace_code(conn->tls_io_data.out_data ? 'o' : 'O');",
        "tls_mbedtls.c handshake clear extra input trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "struct wpabuf * tls_connection_encrypt(void *tls_ctx,\n\t\t\t\t       struct tls_connection *conn,\n\t\t\t\t       const struct wpabuf *in_data)\n{\n\t/* Reset dangling pointer */\n\tconn->tls_io_data.out_data = NULL;\n\n\tssize_t ret = mbedtls_ssl_write(&conn->tls->ssl,\n\t\t\t(unsigned char*) wpabuf_head(in_data),  wpabuf_len(in_data));\n\n\tif (ret < wpabuf_len(in_data)) {\n\t\twpa_printf(MSG_ERROR, \"%s:%d, not able to write whole data\",\n\t\t\t   __func__, __LINE__);\n\t}\n\n\treturn conn->tls_io_data.out_data;\n}",
        "struct wpabuf * tls_connection_encrypt(void *tls_ctx,\n\t\t\t\t       struct tls_connection *conn,\n\t\t\t\t       const struct wpabuf *in_data)\n{\n\tsize_t out_len;\n\t/* Reset dangling pointer */\n\tconn->tls_io_data.out_data = NULL;\n\tesp8266_hal_eap_trace_code('e');\n\tesp8266_hal_eap_trace_hex32((unsigned int)wpabuf_len(in_data));\n\tif (wpabuf_len(in_data) > 0) {\n\t\tesp8266_hal_eap_trace_hex8((unsigned int)wpabuf_head_u8(in_data)[0]);\n\t}\n\n\tssize_t ret = mbedtls_ssl_write(&conn->tls->ssl,\n\t\t\t(unsigned char*) wpabuf_head(in_data),  wpabuf_len(in_data));\n\tout_len = conn->tls_io_data.out_data ? wpabuf_len(conn->tls_io_data.out_data) : 0;\n\tesp8266_hal_eap_trace_code('v');\n\tesp8266_hal_eap_trace_hex32((unsigned int)ret);\n\tesp8266_hal_eap_trace_hex32((unsigned int)out_len);\n\n\tif (ret < 0 || (size_t)ret < wpabuf_len(in_data)) {\n\t\tesp8266_hal_eap_trace_code('V');\n\t\twpa_printf(MSG_ERROR, \"%s:%d, not able to write whole data\",\n\t\t\t   __func__, __LINE__);\n\t}\n\n\treturn conn->tls_io_data.out_data;\n}",
        "tls_mbedtls.c encrypt trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "struct wpabuf * tls_connection_decrypt(void *tls_ctx,\n\t\tstruct tls_connection *conn,\n\t\tconst struct wpabuf *in_data)\n{\n\tunsigned char buf[1200];\n\tint ret;\n\tconn->tls_io_data.in_data = wpabuf_dup(in_data);\n\tret = mbedtls_ssl_read(&conn->tls->ssl, buf, 1200);\n\tif (ret < 0) {\n\t\twpa_printf(MSG_ERROR, \"%s:%d, not able to write whole data\",\n\t\t\t\t__func__, __LINE__);\n\t\treturn NULL;\n\t}\n\n\tstruct wpabuf *out = wpabuf_alloc_copy(buf, ret);\n\n\treturn out;\n}",
        "struct wpabuf * tls_connection_decrypt(void *tls_ctx,\n\t\tstruct tls_connection *conn,\n\t\tconst struct wpabuf *in_data)\n{\n\tunsigned char buf[1200];\n\tstruct wpabuf *out;\n\tint ret;\n\tesp8266_hal_eap_trace_code('d');\n\tesp8266_hal_eap_trace_hex32((unsigned int)wpabuf_len(in_data));\n\twpabuf_free(conn->tls_io_data.in_data);\n\tconn->tls_io_data.in_data = wpabuf_dup(in_data);\n\tconn->tls_io_data.in_ext_data = NULL;\n\tconn->tls_io_data.in_ext_len = 0;\n\tconn->tls_io_data.in_ext_pos = 0;\n\tret = mbedtls_ssl_read(&conn->tls->ssl, buf, 1200);\n\tesp8266_hal_eap_trace_code('D');\n\tesp8266_hal_eap_trace_hex32((unsigned int)ret);\n\tif (ret < 0) {\n\t\twpa_printf(MSG_ERROR, \"%s:%d, not able to write whole data\",\n\t\t\t\t__func__, __LINE__);\n\t\treturn NULL;\n\t}\n\n\tout = wpabuf_alloc_copy(buf, (size_t)ret);\n\tesp8266_hal_eap_trace_code(out ? 'O' : 'Q');\n\tesp8266_hal_eap_trace_hex32((unsigned int)ret);\n\treturn out;\n}",
        "tls_mbedtls.c decrypt trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tconn->tls_io_data.in_ext_pos = 0;\n\tret = mbedtls_ssl_read(&conn->tls->ssl, buf, 1200);",
        "\tconn->tls_io_data.in_ext_pos = 0;\n\tconn->tls_io_data.in_ext_extra_data = NULL;\n\tconn->tls_io_data.in_ext_extra_len = 0;\n\tconn->tls_io_data.in_ext_extra_pos = 0;\n\tret = mbedtls_ssl_read(&conn->tls->ssl, buf, 1200);",
        "tls_mbedtls.c decrypt clear extra input trace",
    );
    fs::write(&patched, patched_text).expect("failed to write patched tls_mbedtls.c");
    patched
}

fn patch_aes_internal_source(source: &Path, out_dir: &Path) -> PathBuf {
    patch_aes_i_header(source, out_dir);
    let patched = out_dir.join("aes_internal_aligned_irom_tables.c");
    let source_text = fs::read_to_string(source).expect("failed to read aes-internal.c");
    let patched_text = replace_required(
        &source_text,
        "#define AES_SMALL_TABLES",
        "/* ESP8266 IROM supports aligned 32-bit loads, but the small-table AES\n * mode emits byte loads from Te0/Td0 and faults when ESP-NOW encryption sets up\n * peer keys. Keep the full tables in flash instead. */\n/* #define AES_SMALL_TABLES */",
        "aes-internal.c aligned IROM tables",
    );
    fs::write(&patched, patched_text).expect("failed to write patched aes-internal.c");
    patched
}

fn patch_aes_internal_user_source(source: &Path, out_dir: &Path) -> PathBuf {
    patch_aes_i_header(source, out_dir);
    let file_name = source
        .file_name()
        .and_then(|name| name.to_str())
        .expect("AES source file has no UTF-8 name");
    let patched = out_dir.join(format!("{}_aligned_irom.c", file_name.replace('-', "_")));
    let source_text = fs::read_to_string(source).expect("failed to read AES helper source");
    fs::write(&patched, source_text).expect("failed to write patched AES helper source");
    patched
}

fn patch_aes_i_header(source: &Path, out_dir: &Path) {
    let header = source
        .parent()
        .expect("AES source has no parent directory")
        .join("aes_i.h");
    let source_text = fs::read_to_string(header).expect("failed to read aes_i.h");
    let patched_text = replace_required(
        &source_text,
        "#define AES_SMALL_TABLES",
        "/* ESP8266 IROM supports aligned 32-bit loads, but byte loads from\n * flash-mapped AES tables fault. Keep the full-table macros. */\n/* #define AES_SMALL_TABLES */",
        "aes_i.h aligned IROM table macros",
    );
    let patched_text = replace_required(
        &patched_text,
        r#"#ifndef AES_SMALL_TABLES

#define RCON(i) rcon[(i)]

#define TE0(i) Te0[((i) >> 24) & 0xff]
#define TE1(i) Te1[((i) >> 16) & 0xff]
#define TE2(i) Te2[((i) >> 8) & 0xff]
#define TE3(i) Te3[(i) & 0xff]
#define TE41(i) (Te4[((i) >> 24) & 0xff] & 0xff000000)
#define TE42(i) (Te4[((i) >> 16) & 0xff] & 0x00ff0000)
#define TE43(i) (Te4[((i) >> 8) & 0xff] & 0x0000ff00)
#define TE44(i) (Te4[(i) & 0xff] & 0x000000ff)
#define TE421(i) (Te4[((i) >> 16) & 0xff] & 0xff000000)
#define TE432(i) (Te4[((i) >> 8) & 0xff] & 0x00ff0000)
#define TE443(i) (Te4[(i) & 0xff] & 0x0000ff00)
#define TE414(i) (Te4[((i) >> 24) & 0xff] & 0x000000ff)
#define TE411(i) (Te4[((i) >> 24) & 0xff] & 0xff000000)
#define TE422(i) (Te4[((i) >> 16) & 0xff] & 0x00ff0000)
#define TE433(i) (Te4[((i) >> 8) & 0xff] & 0x0000ff00)
#define TE444(i) (Te4[(i) & 0xff] & 0x000000ff)
#define TE4(i) (Te4[(i)] & 0x000000ff)

#define TD0(i) Td0[((i) >> 24) & 0xff]
#define TD1(i) Td1[((i) >> 16) & 0xff]
#define TD2(i) Td2[((i) >> 8) & 0xff]
#define TD3(i) Td3[(i) & 0xff]
#define TD41(i) (Td4[((i) >> 24) & 0xff] & 0xff000000)
#define TD42(i) (Td4[((i) >> 16) & 0xff] & 0x00ff0000)
#define TD43(i) (Td4[((i) >> 8) & 0xff] & 0x0000ff00)
#define TD44(i) (Td4[(i) & 0xff] & 0x000000ff)
#define TD0_(i) Td0[(i) & 0xff]
#define TD1_(i) Td1[(i) & 0xff]
#define TD2_(i) Td2[(i) & 0xff]
#define TD3_(i) Td3[(i) & 0xff]

#else /* AES_SMALL_TABLES */"#,
        r#"#ifndef AES_SMALL_TABLES

#define AES_TABLE_LOAD(table, index) (((const volatile u32 *)(table))[(index)])

#define RCON(i) AES_TABLE_LOAD(rcon, (i))

#define TE0(i) AES_TABLE_LOAD(Te0, (((i) >> 24) & 0xff))
#define TE1(i) AES_TABLE_LOAD(Te1, (((i) >> 16) & 0xff))
#define TE2(i) AES_TABLE_LOAD(Te2, (((i) >> 8) & 0xff))
#define TE3(i) AES_TABLE_LOAD(Te3, ((i) & 0xff))
#define TE41(i) (AES_TABLE_LOAD(Te4, (((i) >> 24) & 0xff)) & 0xff000000)
#define TE42(i) (AES_TABLE_LOAD(Te4, (((i) >> 16) & 0xff)) & 0x00ff0000)
#define TE43(i) (AES_TABLE_LOAD(Te4, (((i) >> 8) & 0xff)) & 0x0000ff00)
#define TE44(i) (AES_TABLE_LOAD(Te4, ((i) & 0xff)) & 0x000000ff)
#define TE421(i) (AES_TABLE_LOAD(Te4, (((i) >> 16) & 0xff)) & 0xff000000)
#define TE432(i) (AES_TABLE_LOAD(Te4, (((i) >> 8) & 0xff)) & 0x00ff0000)
#define TE443(i) (AES_TABLE_LOAD(Te4, ((i) & 0xff)) & 0x0000ff00)
#define TE414(i) (AES_TABLE_LOAD(Te4, (((i) >> 24) & 0xff)) & 0x000000ff)
#define TE411(i) (AES_TABLE_LOAD(Te4, (((i) >> 24) & 0xff)) & 0xff000000)
#define TE422(i) (AES_TABLE_LOAD(Te4, (((i) >> 16) & 0xff)) & 0x00ff0000)
#define TE433(i) (AES_TABLE_LOAD(Te4, (((i) >> 8) & 0xff)) & 0x0000ff00)
#define TE444(i) (AES_TABLE_LOAD(Te4, ((i) & 0xff)) & 0x000000ff)
#define TE4(i) (AES_TABLE_LOAD(Te4, (i)) & 0x000000ff)

#define TD0(i) AES_TABLE_LOAD(Td0, (((i) >> 24) & 0xff))
#define TD1(i) AES_TABLE_LOAD(Td1, (((i) >> 16) & 0xff))
#define TD2(i) AES_TABLE_LOAD(Td2, (((i) >> 8) & 0xff))
#define TD3(i) AES_TABLE_LOAD(Td3, ((i) & 0xff))
#define TD41(i) (AES_TABLE_LOAD(Td4, (((i) >> 24) & 0xff)) & 0xff000000)
#define TD42(i) (AES_TABLE_LOAD(Td4, (((i) >> 16) & 0xff)) & 0x00ff0000)
#define TD43(i) (AES_TABLE_LOAD(Td4, (((i) >> 8) & 0xff)) & 0x0000ff00)
#define TD44(i) (AES_TABLE_LOAD(Td4, ((i) & 0xff)) & 0x000000ff)
#define TD0_(i) AES_TABLE_LOAD(Td0, ((i) & 0xff))
#define TD1_(i) AES_TABLE_LOAD(Td1, ((i) & 0xff))
#define TD2_(i) AES_TABLE_LOAD(Td2, ((i) & 0xff))
#define TD3_(i) AES_TABLE_LOAD(Td3, ((i) & 0xff))

#else /* AES_SMALL_TABLES */"#,
        "aes_i.h volatile 32-bit table loads",
    );
    fs::write(out_dir.join("aes_i.h"), patched_text).expect("failed to write patched aes_i.h");
}

fn patch_esp_wps_source(source: &Path, out_dir: &Path) -> PathBuf {
    let patched = out_dir.join("esp_wps_patched.c");
    let source_text = fs::read_to_string(source).expect("failed to read esp_wps.c");
    let include_hook = concat!(
        "#include \"esp_wifi_driver.h\"\n",
        "#undef WPS_TASK_STACK_SIZE\n",
        "#define WPS_TASK_STACK_SIZE (3584 + TASK_STACK_SIZE_ADD)\n",
        "static unsigned char esp8266_hal_wps_pin_override[9];\n",
        "static unsigned char esp8266_hal_wps_target_ssid[32];\n",
        "static unsigned char esp8266_hal_wps_target_ssid_len;\n",
        "static void esp8266_hal_wps_trace_char(char c) {\n",
        "    volatile unsigned int *status = (volatile unsigned int *)(0x60000000 + 0x1c);\n",
        "    volatile unsigned int *fifo = (volatile unsigned int *)0x60000000;\n",
        "    if ((((*status) >> 16) & 0xffu) < 126u) {\n",
        "        *fifo = (unsigned int)c;\n",
        "    }\n",
        "}\n",
        "static void esp8266_hal_wps_trace_hex_nibble(unsigned int value) {\n",
        "    value &= 0xFu;\n",
        "    esp8266_hal_wps_trace_char((char)(value < 10u ? ('0' + value) : ('A' + (value - 10u))));\n",
        "}\n",
        "static void esp8266_hal_wps_trace_hex8(unsigned int value) {\n",
        "    esp8266_hal_wps_trace_hex_nibble(value >> 4);\n",
        "    esp8266_hal_wps_trace_hex_nibble(value);\n",
        "}\n",
        "static void esp8266_hal_wps_trace_hex32(unsigned int value) {\n",
        "    for (int shift = 28; shift >= 0; shift -= 4) {\n",
        "        esp8266_hal_wps_trace_hex_nibble(value >> (unsigned int)shift);\n",
        "    }\n",
        "}\n",
        "void esp8266_hal_wps_set_pin(const unsigned char *pin) {\n",
        "    if (pin) {\n",
        "        memcpy(esp8266_hal_wps_pin_override, pin, 8);\n",
        "        esp8266_hal_wps_pin_override[8] = 0;\n",
        "    } else {\n",
        "        memset(esp8266_hal_wps_pin_override, 0, sizeof(esp8266_hal_wps_pin_override));\n",
        "    }\n",
        "}\n",
        "void esp8266_hal_wps_set_target_ssid(const unsigned char *ssid, unsigned int len) {\n",
        "    if (ssid && len <= sizeof(esp8266_hal_wps_target_ssid)) {\n",
        "        memcpy(esp8266_hal_wps_target_ssid, ssid, len);\n",
        "        esp8266_hal_wps_target_ssid_len = len;\n",
        "    } else {\n",
        "        memset(esp8266_hal_wps_target_ssid, 0, sizeof(esp8266_hal_wps_target_ssid));\n",
        "        esp8266_hal_wps_target_ssid_len = 0;\n",
        "    }\n",
        "}"
    );
    let patched_text = replace_required(
        &source_text,
        "#include \"esp_wifi_driver.h\"",
        include_hook,
        "esp_wps.c include hook",
    );
    let patched_text = replace_required(
        &patched_text,
        "        spin = wps_generate_pin();\n        sprintf((char *)data->dev_password, \"%08d\", spin);",
        "        if (esp8266_hal_wps_pin_override[0] != 0) {\n            memcpy(data->dev_password, esp8266_hal_wps_pin_override, 8);\n            data->dev_password[8] = 0;\n        } else {\n            spin = wps_generate_pin();\n            sprintf((char *)data->dev_password, \"%08d\", spin);\n        }",
        "esp_wps.c PIN override hook",
    );
    let patched_text = replace_required(
        &patched_text,
        "        wps_build_public_key(sm->wps, NULL, WPS_CALC_KEY_PRE_CALC);\n\n        wifi_wps_scan();",
        "        if (wps_build_public_key(sm->wps, NULL, WPS_CALC_KEY_PRE_CALC) != 0) {\n            esp8266_hal_wps_trace_char('~');\n            esp8266_hal_wps_trace_char('K');\n            return ESP_ERR_NO_MEM;\n        }\n\n        wifi_wps_scan();",
        "esp_wps.c checked WPS public-key precompute",
    );
    let patched_text = replace_required(
        &patched_text,
        "        if (wps_is_selected_pbc_registrar(buf, scan->bssid)\n                || wps_is_selected_pin_registrar(buf, scan->bssid)) {",
        "        bool esp8266_hal_wps_selected = wps_is_selected_pbc_registrar(buf, scan->bssid)\n                || wps_is_selected_pin_registrar(buf, scan->bssid);\n        if (!esp8266_hal_wps_selected && esp8266_hal_wps_target_ssid_len != 0 &&\n                scan->ssid[1] == esp8266_hal_wps_target_ssid_len &&\n                memcmp(&scan->ssid[2], esp8266_hal_wps_target_ssid, esp8266_hal_wps_target_ssid_len) == 0) {\n            esp8266_hal_wps_trace_char('~');\n            esp8266_hal_wps_trace_char('t');\n            esp8266_hal_wps_trace_hex8(scan->chan);\n            esp8266_hal_wps_selected = true;\n        }\n        if (esp8266_hal_wps_selected) {\n            esp8266_hal_wps_trace_char('~');\n            esp8266_hal_wps_trace_char('S');\n            esp8266_hal_wps_trace_hex8(scan->chan);",
        "esp_wps.c target SSID scan selection",
    );
    let patched_text = replace_required(
        &patched_text,
        "    if (sm->discover_ssid_cnt == 1) {\n        wps_set_status(WPS_STATUS_PENDING);",
        "    esp8266_hal_wps_trace_char('~');\n    esp8266_hal_wps_trace_char('D');\n    esp8266_hal_wps_trace_hex8(sm->discover_ssid_cnt);\n    esp8266_hal_wps_trace_hex8(wps_get_status());\n\n    if (sm->discover_ssid_cnt == 1) {\n        wps_set_status(WPS_STATUS_PENDING);",
        "esp_wps.c scan-done trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "    ets_timer_arm(&sm->wps_timeout_timer, 120000, 0); /* 120s total */",
        "    esp8266_hal_wps_trace_char('~');\n    esp8266_hal_wps_trace_char('B');\n    esp8266_hal_wps_trace_hex8(wps_get_type());\n    esp8266_hal_wps_trace_hex8(wps_get_status());\n\n    ets_timer_arm(&sm->wps_timeout_timer, 120000, 0); /* 120s total */",
        "esp_wps.c start trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "    ret = esp_wifi_get_assoc_bssid_internal(bssid);\n    if (ret != 0) {\n        wpa_printf(MSG_ERROR, \"bssid is empty!\");\n        return ret;\n    }\n\n    if (!sm) {",
        "    ret = esp_wifi_get_assoc_bssid_internal(bssid);\n    if (ret != 0) {\n        esp8266_hal_wps_trace_char('~');\n        esp8266_hal_wps_trace_char('e');\n        esp8266_hal_wps_trace_hex8((unsigned int)(-ret));\n        wpa_printf(MSG_ERROR, \"bssid is empty!\");\n        return ret;\n    }\n\n    esp8266_hal_wps_trace_char('~');\n    esp8266_hal_wps_trace_char('E');\n\n    if (!sm) {",
        "esp_wps.c eapol-start trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "        case EAP_TYPE_IDENTITY:\n            wpa_printf(MSG_DEBUG, \"=========identity===========\");",
        "        case EAP_TYPE_IDENTITY:\n            esp8266_hal_wps_trace_char('~');\n            esp8266_hal_wps_trace_char('I');\n            wpa_printf(MSG_DEBUG, \"=========identity===========\");",
        "esp_wps.c identity trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "        case EAP_TYPE_EXPANDED:\n            wpa_printf(MSG_DEBUG, \"=========expanded plen[%d], %d===========\", plen, sizeof(*ehdr));",
        "        case EAP_TYPE_EXPANDED:\n            esp8266_hal_wps_trace_char('~');\n            esp8266_hal_wps_trace_char('X');\n            esp8266_hal_wps_trace_hex8(sm->wps->state);\n            wpa_printf(MSG_DEBUG, \"=========expanded plen[%d], %d===========\", plen, sizeof(*ehdr));",
        "esp_wps.c expanded trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "            ret = wps_process_wps_mX_req(tmp, plen - sizeof(*ehdr) - 1, &res);\n            if (ret == 0 && res != WPS_FAILURE && res != WPS_IGNORE && res != WPS_FRAGMENT) {",
        "            ret = wps_process_wps_mX_req(tmp, plen - sizeof(*ehdr) - 1, &res);\n            esp8266_hal_wps_trace_char('~');\n            esp8266_hal_wps_trace_char('x');\n            esp8266_hal_wps_trace_hex8(sm->wps->state);\n            esp8266_hal_wps_trace_hex8(res);\n            esp8266_hal_wps_trace_hex8((unsigned int)(-ret));\n            if (ret == 0 && res != WPS_FAILURE && res != WPS_IGNORE && res != WPS_FRAGMENT) {",
        "esp_wps.c process-result trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "        ets_timer_disarm(&sm->wps_success_cb_timer);\n        ets_timer_arm(&sm->wps_success_cb_timer, 1000, 0);\n\n        ret = 0;",
        "        ets_timer_disarm(&sm->wps_success_cb_timer);\n        wifi_station_wps_success_internal();\n\n        ret = 0;",
        "esp_wps.c immediate WPS success callback",
    );
    let patched_text = replace_required(
        &patched_text,
        "    esp_wifi_disarm_sta_connection_timer_internal();\n\n    wps_set_status(WPS_STATUS_DISABLE);",
        "    esp8266_hal_wps_trace_char('~');\n    esp8266_hal_wps_trace_char('T');\n    esp8266_hal_wps_trace_hex8(sm->wps->state);\n    esp8266_hal_wps_trace_hex8(wps_get_status());\n\n    esp_wifi_disarm_sta_connection_timer_internal();\n\n    wps_set_status(WPS_STATUS_DISABLE);",
        "esp_wps.c timeout trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "    if (sm->wps->state == WPS_FINISHED) {\n        wpa_printf(MSG_DEBUG, \"wps msg timeout WPS_FINISHED\");",
        "    esp8266_hal_wps_trace_char('~');\n    esp8266_hal_wps_trace_char('M');\n    esp8266_hal_wps_trace_hex8(sm->wps->state);\n\n    if (sm->wps->state == WPS_FINISHED) {\n        wpa_printf(MSG_DEBUG, \"wps msg timeout WPS_FINISHED\");",
        "esp_wps.c message-timeout trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "        system_event_t evt;\n        evt.event_id = SYSTEM_EVENT_STA_WPS_ER_FAILED;\n        esp_event_send(&evt);\n\n        return ret;",
        "        system_event_t evt;\n        evt.event_id = SYSTEM_EVENT_STA_WPS_ER_FAILED;\n        evt.event_info.sta_er_fail_reason = (((u32)sm->wps->state & 0xff) << 16) |\n                                            (((u32)res & 0xff) << 8) |\n                                            ((u32)(-ret) & 0xff);\n        esp_event_send(&evt);\n\n        return ret;",
        "esp_wps.c WPS failure diagnostic reason",
    );
    fs::write(&patched, patched_text).expect("failed to write patched esp_wps.c");
    patched
}

fn patch_wps_attr_build_source(source: &Path, out_dir: &Path) -> PathBuf {
    let patched = out_dir.join("wps_attr_build_patched.c");
    let source_text = fs::read_to_string(source).expect("failed to read wps_attr_build.c");
    let patched_text = replace_required(
        &source_text,
        "\t\t\twps->dh_ctx = dh5_init(&wps->dh_privkey, &pubkey);\n\n\t\t\twpa_printf(MSG_DEBUG, \"build public key finish\\n\");\n\n\t\t\tpubkey = wpabuf_zeropad(pubkey, 192);",
        "\t\t\twps->dh_ctx = dh5_init(&wps->dh_privkey, &pubkey);\n\n\t\t\twpa_printf(MSG_DEBUG, \"build public key finish\\n\");\n\n\t\t\tif (pubkey == NULL) {\n\t\t\t\twpa_printf(MSG_DEBUG, \"WPS: Public key allocation failed\");\n\t\t\t\treturn -1;\n\t\t\t}\n\n\t\t\tpubkey = wpabuf_zeropad(pubkey, 192);",
        "wps_attr_build.c precompute public-key null guard",
    );
    let patched_text = replace_required(
        &patched_text,
        "\t\tif (wps->registrar)\n\t\t\tpubkey = wps->dh_pubkey_r;\n\t\telse\n\t\t\tpubkey = wps->dh_pubkey_e;\n\n\t\twpabuf_put_be16(msg, ATTR_PUBLIC_KEY);",
        "\t\tif (wps->registrar)\n\t\t\tpubkey = wps->dh_pubkey_r;\n\t\telse\n\t\t\tpubkey = wps->dh_pubkey_e;\n\n\t\tif (pubkey == NULL) {\n\t\t\twpa_printf(MSG_DEBUG, \"WPS: Public key missing\");\n\t\t\treturn -1;\n\t\t}\n\n\t\twpabuf_put_be16(msg, ATTR_PUBLIC_KEY);",
        "wps_attr_build.c public-key null guard",
    );
    fs::write(&patched, patched_text).expect("failed to write patched wps_attr_build.c");
    patched
}

fn patch_wps_enrollee_source(source: &Path, out_dir: &Path) -> PathBuf {
    let patched = out_dir.join("wps_enrollee_patched.c");
    let source_text = fs::read_to_string(source).expect("failed to read wps_enrollee.c");
    let include_hook = concat!(
        "#include \"wps/wps_dev_attr.h\"\n",
        "static void esp8266_hal_wps_trace_char(char c) {\n",
        "    volatile unsigned int *status = (volatile unsigned int *)(0x60000000 + 0x1c);\n",
        "    volatile unsigned int *fifo = (volatile unsigned int *)0x60000000;\n",
        "    if ((((*status) >> 16) & 0xffu) < 126u) {\n",
        "        *fifo = (unsigned int)c;\n",
        "    }\n",
        "}\n"
    );
    let patched_text = replace_required(
        &source_text,
        "#include \"wps/wps_dev_attr.h\"\n",
        include_hook,
        "wps_enrollee.c trace hook",
    );
    let patched_text = replace_required(
        &patched_text,
        "        if (eattr == NULL) {\n\t\twps->state = SEND_WSC_NACK;\n\t\tres = WPS_CONTINUE;\n\t\tgoto _out;\n\t}",
        "        if (eattr == NULL) {\n\t\tesp8266_hal_wps_trace_char('~');\n\t\tesp8266_hal_wps_trace_char('8');\n\t\tesp8266_hal_wps_trace_char('a');\n\t\twps->state = SEND_WSC_NACK;\n\t\tres = WPS_CONTINUE;\n\t\tgoto _out;\n\t}",
        "wps_enrollee.c M8 eattr trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tif (wps_process_enrollee_nonce(wps, attr->enrollee_nonce) ||\n\t    wps_process_authenticator(wps, attr->authenticator, msg)) {\n\t\twps->state = SEND_WSC_NACK;\n\t\tres = WPS_CONTINUE;\n\t\tgoto _out;\n\t}",
        "\tif (wps_process_enrollee_nonce(wps, attr->enrollee_nonce)) {\n\t\tesp8266_hal_wps_trace_char('~');\n\t\tesp8266_hal_wps_trace_char('8');\n\t\tesp8266_hal_wps_trace_char('n');\n\t\twps->state = SEND_WSC_NACK;\n\t\tres = WPS_CONTINUE;\n\t\tgoto _out;\n\t}\n\tif (wps_process_authenticator(wps, attr->authenticator, msg)) {\n\t\tesp8266_hal_wps_trace_char('~');\n\t\tesp8266_hal_wps_trace_char('8');\n\t\tesp8266_hal_wps_trace_char('a');\n\t\twps->state = SEND_WSC_NACK;\n\t\tres = WPS_CONTINUE;\n\t\tgoto _out;\n\t}",
        "wps_enrollee.c M8 nonce/auth trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tif (decrypted == NULL) {\n\t\twpa_printf(MSG_DEBUG,  \"WPS: Failed to decrypted Encrypted \"\n\t\t\t   \"Settings attribute\");\n\t\twps->state = SEND_WSC_NACK;\n\t\tres = WPS_CONTINUE;\n\t\tgoto _out;\n\t}",
        "\tif (decrypted == NULL) {\n\t\tesp8266_hal_wps_trace_char('~');\n\t\tesp8266_hal_wps_trace_char('8');\n\t\tesp8266_hal_wps_trace_char('d');\n\t\twpa_printf(MSG_DEBUG,  \"WPS: Failed to decrypted Encrypted \"\n\t\t\t   \"Settings attribute\");\n\t\twps->state = SEND_WSC_NACK;\n\t\tres = WPS_CONTINUE;\n\t\tgoto _out;\n\t}",
        "wps_enrollee.c M8 decrypt trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tif (wps_validate_m8_encr(decrypted, wps->wps->ap,\n\t\t\t\t attr->version2 != NULL) < 0) {\n\t\twpabuf_free(decrypted);\n\t\twps->state = SEND_WSC_NACK;\n\t\tres = WPS_CONTINUE;\n\t\tgoto _out;\n\t}",
        "\tif (wps_validate_m8_encr(decrypted, wps->wps->ap,\n\t\t\t\t attr->version2 != NULL) < 0) {\n\t\tesp8266_hal_wps_trace_char('~');\n\t\tesp8266_hal_wps_trace_char('8');\n\t\tesp8266_hal_wps_trace_char('v');\n\t\twpabuf_free(decrypted);\n\t\twps->state = SEND_WSC_NACK;\n\t\tres = WPS_CONTINUE;\n\t\tgoto _out;\n\t}",
        "wps_enrollee.c M8 validate trace",
    );
    let patched_text = replace_required(
        &patched_text,
        "\tif (wps_parse_msg(decrypted, eattr) < 0 ||\n\t    wps_process_key_wrap_auth(wps, decrypted, eattr->key_wrap_auth) ||\n\t    wps_process_creds(wps, eattr->cred, eattr->cred_len,\n\t\t\t      eattr->num_cred, attr->version2 != NULL) ||\n\t    wps_process_ap_settings_e(wps, eattr, decrypted,\n\t\t\t\t      attr->version2 != NULL)) {\n\t\twpabuf_free(decrypted);\n\t\twps->state = SEND_WSC_NACK;\n\t\tres = WPS_CONTINUE;\n\t\tgoto _out;\n\t}",
        "\tif (wps_parse_msg(decrypted, eattr) < 0) {\n\t\tesp8266_hal_wps_trace_char('~');\n\t\tesp8266_hal_wps_trace_char('8');\n\t\tesp8266_hal_wps_trace_char('p');\n\t\twpabuf_free(decrypted);\n\t\twps->state = SEND_WSC_NACK;\n\t\tres = WPS_CONTINUE;\n\t\tgoto _out;\n\t}\n\tif (wps_process_key_wrap_auth(wps, decrypted, eattr->key_wrap_auth)) {\n\t\tesp8266_hal_wps_trace_char('~');\n\t\tesp8266_hal_wps_trace_char('8');\n\t\tesp8266_hal_wps_trace_char('k');\n\t\twpabuf_free(decrypted);\n\t\twps->state = SEND_WSC_NACK;\n\t\tres = WPS_CONTINUE;\n\t\tgoto _out;\n\t}\n\tif (wps_process_creds(wps, eattr->cred, eattr->cred_len,\n\t\t\t      eattr->num_cred, attr->version2 != NULL)) {\n\t\tesp8266_hal_wps_trace_char('~');\n\t\tesp8266_hal_wps_trace_char('8');\n\t\tesp8266_hal_wps_trace_char('c');\n\t\twpabuf_free(decrypted);\n\t\twps->state = SEND_WSC_NACK;\n\t\tres = WPS_CONTINUE;\n\t\tgoto _out;\n\t}\n\tif (wps_process_ap_settings_e(wps, eattr, decrypted,\n\t\t\t\t      attr->version2 != NULL)) {\n\t\tesp8266_hal_wps_trace_char('~');\n\t\tesp8266_hal_wps_trace_char('8');\n\t\tesp8266_hal_wps_trace_char('s');\n\t\twpabuf_free(decrypted);\n\t\twps->state = SEND_WSC_NACK;\n\t\tres = WPS_CONTINUE;\n\t\tgoto _out;\n\t}",
        "wps_enrollee.c M8 final branch trace",
    );
    fs::write(&patched, patched_text).expect("failed to write patched wps_enrollee.c");
    patched
}

fn patch_ssl_tls_source(source: &Path, out_dir: &Path) -> PathBuf {
    let patched = out_dir.join("ssl_tls_patched.c");
    let source_text = fs::read_to_string(source).expect("failed to read ssl_tls.c");
    let patched_text = replace_required(
        &source_text,
        "int mbedtls_ssl_write_certificate( mbedtls_ssl_context *ssl )",
        "static void esp8266_hal_ssl_drop_sent_client_cert( mbedtls_ssl_context *ssl )\n{\n#if defined(MBEDTLS_SSL_CLI_C) && defined(MBEDTLS_X509_CRT_PARSE_C)\n    mbedtls_ssl_key_cert *key_cert;\n\n    if( ssl->conf == NULL || ssl->conf->endpoint != MBEDTLS_SSL_IS_CLIENT )\n        return;\n\n    if( ssl->handshake != NULL && ssl->handshake->key_cert != NULL )\n        key_cert = ssl->handshake->key_cert;\n    else\n        key_cert = ssl->conf->key_cert;\n\n    if( key_cert == NULL || key_cert->cert == NULL )\n        return;\n\n    mbedtls_x509_crt_free( key_cert->cert );\n#else\n    (void) ssl;\n#endif\n}\n\nint mbedtls_ssl_write_certificate( mbedtls_ssl_context *ssl )",
        "ssl_tls.c drop sent client certificate helper",
    );
    let patched_text = replace_required(
        &patched_text,
        "    if( ( ret = mbedtls_ssl_write_handshake_msg( ssl ) ) != 0 )\n    {\n        MBEDTLS_SSL_DEBUG_RET( 1, \"mbedtls_ssl_write_handshake_msg\", ret );\n        return( ret );\n    }\n\n    MBEDTLS_SSL_DEBUG_MSG( 2, ( \"<= write certificate\" ) );",
        "    if( ( ret = mbedtls_ssl_write_handshake_msg( ssl ) ) != 0 )\n    {\n        MBEDTLS_SSL_DEBUG_RET( 1, \"mbedtls_ssl_write_handshake_msg\", ret );\n        return( ret );\n    }\n\n    esp8266_hal_ssl_drop_sent_client_cert( ssl );\n\n    MBEDTLS_SSL_DEBUG_MSG( 2, ( \"<= write certificate\" ) );",
        "ssl_tls.c drop sent client certificate after write",
    );
    let patched_text = replace_required(
        &patched_text,
        "int mbedtls_ssl_parse_certificate( mbedtls_ssl_context *ssl )",
        "static int esp8266_hal_ssl_release_in_buf_for_verify( mbedtls_ssl_context *ssl )\n{\n    if( ssl->conf == NULL || ssl->conf->endpoint != MBEDTLS_SSL_IS_CLIENT )\n        return( 0 );\n\n    if( ssl->in_buf == NULL || ssl->in_offt != NULL || ssl->in_left != 0 )\n        return( 0 );\n\n    if( ssl->in_hslen != 0 && ssl->in_hslen < ssl->in_msglen )\n        return( 0 );\n\n    mbedtls_platform_zeroize( ssl->in_buf, MBEDTLS_SSL_IN_BUFFER_LEN );\n    mbedtls_free( ssl->in_buf );\n    ssl->in_buf = NULL;\n    ssl->in_hdr = NULL;\n    ssl->in_ctr = NULL;\n    ssl->in_len = NULL;\n    ssl->in_iv = NULL;\n    ssl->in_msg = NULL;\n    ssl->in_msglen = 0;\n    ssl->in_hslen = 0;\n\n    return( 1 );\n}\n\nstatic int esp8266_hal_ssl_restore_in_buf_after_verify( mbedtls_ssl_context *ssl,\n                                                        int released )\n{\n    if( ! released )\n        return( 0 );\n\n    ssl->in_buf = mbedtls_calloc( 1, MBEDTLS_SSL_IN_BUFFER_LEN );\n    if( ssl->in_buf == NULL )\n        return( MBEDTLS_ERR_SSL_ALLOC_FAILED );\n\n    ssl->in_left = 0;\n#if defined(MBEDTLS_SSL_PROTO_DTLS)\n    ssl->next_record_offset = 0;\n#endif\n    ssl_reset_in_out_pointers( ssl );\n\n    return( 0 );\n}\n\nint mbedtls_ssl_parse_certificate( mbedtls_ssl_context *ssl )",
        "ssl_tls.c temporary input-buffer release helper",
    );
    let patched_text = replace_required(
        &patched_text,
        "    if( ( ret = ssl_parse_certificate_chain( ssl ) ) != 0 )\n    {\n#if defined(MBEDTLS_SSL_SRV_C)\n        if( ret == MBEDTLS_ERR_SSL_NO_CLIENT_CERTIFICATE &&\n            authmode == MBEDTLS_SSL_VERIFY_OPTIONAL )\n        {\n            ret = 0;\n        }\n#endif\n\n        ssl->state++;\n        return( ret );\n    }",
        "    {\n        extern void esp8266_hal_alloc_context_set(unsigned int context);\n        extern void esp8266_hal_alloc_context_clear(void);\n        esp8266_hal_alloc_context_set(0x3000u);\n        ret = ssl_parse_certificate_chain( ssl );\n        esp8266_hal_alloc_context_clear();\n    }\n    if( ret != 0 )\n    {\n#if defined(MBEDTLS_SSL_SRV_C)\n        if( ret == MBEDTLS_ERR_SSL_NO_CLIENT_CERTIFICATE &&\n            authmode == MBEDTLS_SSL_VERIFY_OPTIONAL )\n        {\n            ret = 0;\n        }\n#endif\n\n        ssl->state++;\n        return( ret );\n    }",
        "ssl_tls.c certificate parse allocation context",
    );
    let patched_text = replace_required(
        &patched_text,
        "        ret = mbedtls_x509_crt_verify_restartable(\n                                ssl->session_negotiate->peer_cert,\n                                ca_chain, ca_crl,\n                                ssl->conf->cert_profile,\n                                ssl->hostname,\n                               &ssl->session_negotiate->verify_result,\n                                ssl->conf->f_vrfy, ssl->conf->p_vrfy, rs_ctx );",
        "        {\n            extern void esp8266_hal_alloc_context_set(unsigned int context);\n            extern void esp8266_hal_alloc_context_clear(void);\n            int esp8266_hal_in_buf_released;\n            int esp8266_hal_restore_ret;\n\n            esp8266_hal_in_buf_released = esp8266_hal_ssl_release_in_buf_for_verify( ssl );\n            esp8266_hal_alloc_context_set(0x3001u);\n            ret = mbedtls_x509_crt_verify_restartable(\n                                    ssl->session_negotiate->peer_cert,\n                                    ca_chain, ca_crl,\n                                    ssl->conf->cert_profile,\n                                    ssl->hostname,\n                                   &ssl->session_negotiate->verify_result,\n                                    ssl->conf->f_vrfy, ssl->conf->p_vrfy, rs_ctx );\n            esp8266_hal_alloc_context_clear();\n\n            esp8266_hal_restore_ret = esp8266_hal_ssl_restore_in_buf_after_verify(\n                                                    ssl, esp8266_hal_in_buf_released );\n            if( esp8266_hal_restore_ret != 0 && ret == 0 )\n                ret = esp8266_hal_restore_ret;\n        }",
        "ssl_tls.c certificate verify allocation context",
    );
    let patched_text = replace_required(
        &patched_text,
        "    if( src->peer_cert != NULL )\n    {\n        int ret;\n\n        dst->peer_cert = mbedtls_calloc( 1, sizeof(mbedtls_x509_crt) );\n        if( dst->peer_cert == NULL )\n            return( MBEDTLS_ERR_SSL_ALLOC_FAILED );\n\n        mbedtls_x509_crt_init( dst->peer_cert );\n\n        if( ( ret = mbedtls_x509_crt_parse_der( dst->peer_cert, src->peer_cert->raw.p,\n                                        src->peer_cert->raw.len ) ) != 0 )\n        {\n            mbedtls_free( dst->peer_cert );\n            dst->peer_cert = NULL;\n            return( ret );\n        }\n    }",
        "    if( src->peer_cert != NULL &&\n        src->peer_cert->raw.p != NULL && src->peer_cert->raw.len != 0 )\n    {\n        int ret;\n\n        dst->peer_cert = mbedtls_calloc( 1, sizeof(mbedtls_x509_crt) );\n        if( dst->peer_cert == NULL )\n            return( MBEDTLS_ERR_SSL_ALLOC_FAILED );\n\n        mbedtls_x509_crt_init( dst->peer_cert );\n\n        if( ( ret = mbedtls_x509_crt_parse_der( dst->peer_cert, src->peer_cert->raw.p,\n                                        src->peer_cert->raw.len ) ) != 0 )\n        {\n            mbedtls_free( dst->peer_cert );\n            dst->peer_cert = NULL;\n            return( ret );\n        }\n    }\n    else\n    {\n        dst->peer_cert = NULL;\n    }",
        "ssl_tls.c rawless peer cert session copy",
    );
    let patched_text = replace_required(
        &patched_text,
        "    ssl->state++;\n\n    MBEDTLS_SSL_DEBUG_MSG( 2, ( \"<= parse certificate\" ) );\n\n    return( ret );",
        "    if( ret == 0 && ssl->conf->endpoint == MBEDTLS_SSL_IS_CLIENT &&\n        ssl->session_negotiate != NULL &&\n        ssl->session_negotiate->peer_cert != NULL )\n    {\n        mbedtls_x509_crt *cert = ssl->session_negotiate->peer_cert;\n        while( cert != NULL )\n        {\n            if( cert->raw.p != NULL )\n            {\n                mbedtls_platform_zeroize( cert->raw.p, cert->raw.len );\n                mbedtls_free( cert->raw.p );\n                cert->raw.p = NULL;\n                cert->raw.len = 0;\n                cert->tbs.p = NULL;\n                cert->tbs.len = 0;\n                cert->issuer_raw.p = NULL;\n                cert->issuer_raw.len = 0;\n                cert->subject_raw.p = NULL;\n                cert->subject_raw.len = 0;\n                cert->sig_oid.p = NULL;\n                cert->sig_oid.len = 0;\n                cert->sig.p = NULL;\n                cert->sig.len = 0;\n            }\n            cert = cert->next;\n        }\n    }\n\n    ssl->state++;\n\n    MBEDTLS_SSL_DEBUG_MSG( 2, ( \"<= parse certificate\" ) );\n\n    return( ret );",
        "ssl_tls.c drop verified peer cert raw der",
    );
    fs::write(&patched, patched_text).expect("failed to write patched ssl_tls.c");
    patched
}

fn patch_ssl_cli_source(source: &Path, out_dir: &Path) -> PathBuf {
    let patched = out_dir.join("ssl_cli_patched.c");
    let source_text = fs::read_to_string(source).expect("failed to read ssl_cli.c");
    let patched_text = replace_required(
        &source_text,
        "static int ssl_parse_server_key_exchange( mbedtls_ssl_context *ssl )",
        "static int esp8266_hal_ssl_release_in_buf_for_ske_verify( mbedtls_ssl_context *ssl )\n{\n    if( ssl->conf == NULL || ssl->conf->endpoint != MBEDTLS_SSL_IS_CLIENT )\n        return( 0 );\n\n    if( ssl->in_buf == NULL || ssl->in_offt != NULL || ssl->in_left != 0 )\n        return( 0 );\n\n    if( ssl->in_hslen != 0 && ssl->in_hslen < ssl->in_msglen )\n        return( 0 );\n\n    mbedtls_platform_zeroize( ssl->in_buf, MBEDTLS_SSL_IN_BUFFER_LEN );\n    mbedtls_free( ssl->in_buf );\n    ssl->in_buf = NULL;\n    ssl->in_hdr = NULL;\n    ssl->in_ctr = NULL;\n    ssl->in_len = NULL;\n    ssl->in_iv = NULL;\n    ssl->in_msg = NULL;\n    ssl->in_msglen = 0;\n    ssl->in_hslen = 0;\n\n    return( 1 );\n}\n\nstatic int esp8266_hal_ssl_restore_in_buf_after_ske_verify( mbedtls_ssl_context *ssl,\n                                                            int released )\n{\n    if( ! released )\n        return( 0 );\n\n    ssl->in_buf = mbedtls_calloc( 1, MBEDTLS_SSL_IN_BUFFER_LEN );\n    if( ssl->in_buf == NULL )\n        return( MBEDTLS_ERR_SSL_ALLOC_FAILED );\n\n    ssl->in_left = 0;\n#if defined(MBEDTLS_SSL_PROTO_DTLS)\n    ssl->next_record_offset = 0;\n    if( ssl->conf->transport == MBEDTLS_SSL_TRANSPORT_DATAGRAM )\n    {\n        ssl->in_hdr = ssl->in_buf;\n        ssl->in_ctr = ssl->in_hdr + 3;\n        ssl->in_len = ssl->in_hdr + 11;\n        ssl->in_iv = ssl->in_hdr + 13;\n        ssl->in_msg = ssl->in_iv;\n    }\n    else\n#endif\n    {\n        ssl->in_hdr = ssl->in_buf + 8;\n        ssl->in_ctr = ssl->in_hdr - 8;\n        ssl->in_len = ssl->in_hdr + 3;\n        ssl->in_iv = ssl->in_hdr + 5;\n        ssl->in_msg = ssl->in_iv;\n    }\n    ssl->in_msglen = 0;\n    ssl->in_hslen = 0;\n\n    return( 0 );\n}\n\nstatic int ssl_parse_server_key_exchange( mbedtls_ssl_context *ssl )",
        "ssl_cli.c temporary input-buffer release helper",
    );
    let patched_text = replace_required(
        &patched_text,
        "static int ssl_parse_server_key_exchange( mbedtls_ssl_context *ssl )",
        "static int esp8266_hal_ssl_release_out_buf_for_cert_verify( mbedtls_ssl_context *ssl )\n{\n    if( ssl->conf == NULL || ssl->conf->endpoint != MBEDTLS_SSL_IS_CLIENT )\n        return( 0 );\n\n    if( ssl->out_buf == NULL || ssl->out_left != 0 )\n        return( 0 );\n\n    mbedtls_platform_zeroize( ssl->out_buf, MBEDTLS_SSL_OUT_BUFFER_LEN );\n    mbedtls_free( ssl->out_buf );\n    ssl->out_buf = NULL;\n    ssl->out_hdr = NULL;\n    ssl->out_ctr = NULL;\n    ssl->out_len = NULL;\n    ssl->out_iv = NULL;\n    ssl->out_msg = NULL;\n    ssl->out_msglen = 0;\n    ssl->out_msgtype = 0;\n\n    return( 1 );\n}\n\nstatic int esp8266_hal_ssl_restore_out_buf_after_cert_verify( mbedtls_ssl_context *ssl,\n                                                             int released )\n{\n    if( ! released )\n        return( 0 );\n\n    ssl->out_buf = mbedtls_calloc( 1, MBEDTLS_SSL_OUT_BUFFER_LEN );\n    if( ssl->out_buf == NULL )\n        return( MBEDTLS_ERR_SSL_ALLOC_FAILED );\n\n    ssl->out_left = 0;\n#if defined(MBEDTLS_SSL_PROTO_DTLS)\n    if( ssl->conf->transport == MBEDTLS_SSL_TRANSPORT_DATAGRAM )\n    {\n        ssl->out_hdr = ssl->out_buf;\n        ssl->out_ctr = ssl->out_hdr + 3;\n        ssl->out_len = ssl->out_hdr + 11;\n        ssl->out_iv = ssl->out_hdr + 13;\n    }\n    else\n#endif\n    {\n        ssl->out_hdr = ssl->out_buf + 8;\n        ssl->out_ctr = ssl->out_hdr - 8;\n        ssl->out_len = ssl->out_hdr + 3;\n        ssl->out_iv = ssl->out_hdr + 5;\n    }\n\n    if( ssl->transform_out != NULL &&\n        ssl->minor_ver >= MBEDTLS_SSL_MINOR_VERSION_2 )\n    {\n        ssl->out_msg = ssl->out_iv + ssl->transform_out->ivlen -\n                       ssl->transform_out->fixed_ivlen;\n    }\n    else\n        ssl->out_msg = ssl->out_iv;\n\n    ssl->out_msglen = 0;\n    ssl->out_msgtype = 0;\n\n    return( 0 );\n}\n\nstatic int ssl_parse_server_key_exchange( mbedtls_ssl_context *ssl )",
        "ssl_cli.c temporary output-buffer release helper",
    );
    let patched_text = replace_required(
        &patched_text,
        "    if( ( ret = mbedtls_pk_encrypt( &ssl->session_negotiate->peer_cert->pk,\n                            p, ssl->handshake->pmslen,\n                            ssl->out_msg + offset + len_bytes, olen,\n                            MBEDTLS_SSL_OUT_CONTENT_LEN - offset - len_bytes,\n                            ssl->conf->f_rng, ssl->conf->p_rng ) ) != 0 )\n    {\n        MBEDTLS_SSL_DEBUG_RET( 1, \"mbedtls_rsa_pkcs1_encrypt\", ret );\n        return( ret );\n    }",
        "    extern void esp8266_hal_eap_rsa_reserve(void);\n    extern void esp8266_hal_eap_rsa_release(void);\n\n    esp8266_hal_eap_rsa_reserve();\n    if( ( ret = mbedtls_pk_encrypt( &ssl->session_negotiate->peer_cert->pk,\n                            p, ssl->handshake->pmslen,\n                            ssl->out_msg + offset + len_bytes, olen,\n                            MBEDTLS_SSL_OUT_CONTENT_LEN - offset - len_bytes,\n                            ssl->conf->f_rng, ssl->conf->p_rng ) ) != 0 )\n    {\n        esp8266_hal_eap_rsa_release();\n        MBEDTLS_SSL_DEBUG_RET( 1, \"mbedtls_rsa_pkcs1_encrypt\", ret );\n        return( ret );\n    }\n    esp8266_hal_eap_rsa_release();",
        "ssl_cli.c RSA premaster reserve",
    );
    let patched_text = replace_required(
        &patched_text,
        "static int ssl_parse_server_hello_done( mbedtls_ssl_context *ssl )",
        "extern void esp8266_hal_eap_rsa_reserve(void);\nextern void esp8266_hal_eap_rsa_release(void);\n\nstatic void esp8266_hal_free_peer_cert_after_ecdhe( mbedtls_ssl_context *ssl )\n{\n#if defined(MBEDTLS_KEY_EXCHANGE_ECDHE_RSA_ENABLED) || \\\n    defined(MBEDTLS_KEY_EXCHANGE_ECDHE_ECDSA_ENABLED)\n    const mbedtls_ssl_ciphersuite_t *ciphersuite_info;\n\n    if( ssl->transform_negotiate == NULL ||\n        ssl->transform_negotiate->ciphersuite_info == NULL ||\n        ssl->session_negotiate == NULL ||\n        ssl->session_negotiate->peer_cert == NULL )\n    {\n        return;\n    }\n\n    ciphersuite_info = ssl->transform_negotiate->ciphersuite_info;\n    if( ciphersuite_info->key_exchange != MBEDTLS_KEY_EXCHANGE_ECDHE_RSA &&\n        ciphersuite_info->key_exchange != MBEDTLS_KEY_EXCHANGE_ECDHE_ECDSA )\n    {\n        return;\n    }\n\n    mbedtls_x509_crt_free( ssl->session_negotiate->peer_cert );\n    mbedtls_free( ssl->session_negotiate->peer_cert );\n    ssl->session_negotiate->peer_cert = NULL;\n#else\n    (void) ssl;\n#endif\n}\n\nstatic int ssl_parse_server_hello_done( mbedtls_ssl_context *ssl )",
        "ssl_cli.c free ECDHE peer cert helper",
    );
    let patched_text = replace_required(
        &patched_text,
        "    ssl->state++;\n\n#if defined(MBEDTLS_SSL_PROTO_DTLS)\n    if( ssl->conf->transport == MBEDTLS_SSL_TRANSPORT_DATAGRAM )\n        mbedtls_ssl_recv_flight_completed( ssl );",
        "    esp8266_hal_free_peer_cert_after_ecdhe( ssl );\n    ssl->state++;\n\n#if defined(MBEDTLS_SSL_PROTO_DTLS)\n    if( ssl->conf->transport == MBEDTLS_SSL_TRANSPORT_DATAGRAM )\n        mbedtls_ssl_recv_flight_completed( ssl );",
        "ssl_cli.c free ECDHE peer cert at ServerHelloDone",
    );
    let patched_text = replace_required(
        &patched_text,
        "        if( ( ret = mbedtls_pk_verify_restartable(\n                        &ssl->session_negotiate->peer_cert->pk,\n                        md_alg, hash, hashlen, p, sig_len, rs_ctx ) ) != 0 )",
        "        {\n            unsigned char esp8266_hal_sig_copy[96];\n            const unsigned char *esp8266_hal_sig = p;\n            int esp8266_hal_in_buf_released = 0;\n            int esp8266_hal_restore_ret;\n            extern void esp8266_hal_alloc_context_set(unsigned int context);\n            extern void esp8266_hal_alloc_context_clear(void);\n\n            if( sig_len <= sizeof( esp8266_hal_sig_copy ) )\n            {\n                memcpy( esp8266_hal_sig_copy, p, sig_len );\n                esp8266_hal_sig = esp8266_hal_sig_copy;\n                esp8266_hal_in_buf_released =\n                    esp8266_hal_ssl_release_in_buf_for_ske_verify( ssl );\n            }\n\n            esp8266_hal_alloc_context_set(0x3002u);\n            ret = mbedtls_pk_verify_restartable(\n                        &ssl->session_negotiate->peer_cert->pk,\n                        md_alg, hash, hashlen, esp8266_hal_sig, sig_len, rs_ctx );\n            esp8266_hal_alloc_context_clear();\n\n            esp8266_hal_restore_ret =\n                esp8266_hal_ssl_restore_in_buf_after_ske_verify(\n                                                    ssl, esp8266_hal_in_buf_released );\n            if( esp8266_hal_restore_ret != 0 && ret == 0 )\n                ret = esp8266_hal_restore_ret;\n        }\n        if( ret != 0 )",
        "ssl_cli.c ServerKeyExchange verify allocation context",
    );
    let patched_text = replace_required(
        &patched_text,
        "    if( ( ret = mbedtls_pk_sign_restartable( mbedtls_ssl_own_key( ssl ),\n                         md_alg, hash_start, hashlen,\n                         ssl->out_msg + 6 + offset, &n,\n                         ssl->conf->f_rng, ssl->conf->p_rng, rs_ctx ) ) != 0 )\n    {\n        MBEDTLS_SSL_DEBUG_RET( 1, \"mbedtls_pk_sign\", ret );\n#if defined(MBEDTLS_SSL__ECP_RESTARTABLE)\n        if( ret == MBEDTLS_ERR_ECP_IN_PROGRESS )\n            ret = MBEDTLS_ERR_SSL_CRYPTO_IN_PROGRESS;\n#endif\n        return( ret );\n    }",
        "    {\n        unsigned char esp8266_hal_sig[96];\n        unsigned char *esp8266_hal_sig_out = ssl->out_msg + 6 + offset;\n        int esp8266_hal_in_buf_released = 0;\n        int esp8266_hal_out_buf_released = 0;\n        int esp8266_hal_in_restore_ret;\n        int esp8266_hal_restore_ret;\n        int esp8266_hal_use_stack_sig = 0;\n        unsigned char esp8266_hal_hash_sig_alg = ssl->out_msg[4];\n        unsigned char esp8266_hal_sig_alg = ssl->out_msg[5];\n        extern void esp8266_hal_alloc_context_set(unsigned int context);\n        extern void esp8266_hal_alloc_context_clear(void);\n\n        if( mbedtls_pk_can_do( mbedtls_ssl_own_key( ssl ), MBEDTLS_PK_ECDSA ) )\n        {\n            esp8266_hal_use_stack_sig = 1;\n#if defined(MBEDTLS_SSL__ECP_RESTARTABLE)\n            if( ssl->handshake->ecrs_enabled )\n                esp8266_hal_use_stack_sig = 0;\n#endif\n        }\n\n        if( esp8266_hal_use_stack_sig )\n        {\n            esp8266_hal_sig_out = esp8266_hal_sig;\n            esp8266_hal_in_buf_released =\n                esp8266_hal_ssl_release_in_buf_for_ske_verify( ssl );\n            esp8266_hal_out_buf_released =\n                esp8266_hal_ssl_release_out_buf_for_cert_verify( ssl );\n        }\n\n        esp8266_hal_eap_rsa_reserve();\n        esp8266_hal_alloc_context_set(0x4001u);\n        ret = mbedtls_pk_sign_restartable( mbedtls_ssl_own_key( ssl ),\n                         md_alg, hash_start, hashlen,\n                         esp8266_hal_sig_out, &n,\n                         ssl->conf->f_rng, ssl->conf->p_rng, rs_ctx );\n        esp8266_hal_alloc_context_clear();\n\n        if( esp8266_hal_use_stack_sig )\n        {\n            esp8266_hal_in_restore_ret =\n                esp8266_hal_ssl_restore_in_buf_after_ske_verify(\n                                            ssl, esp8266_hal_in_buf_released );\n            if( esp8266_hal_in_restore_ret != 0 && ret == 0 )\n                ret = esp8266_hal_in_restore_ret;\n\n            esp8266_hal_restore_ret =\n                esp8266_hal_ssl_restore_out_buf_after_cert_verify(\n                                            ssl, esp8266_hal_out_buf_released );\n            if( esp8266_hal_restore_ret != 0 && ret == 0 )\n                ret = esp8266_hal_restore_ret;\n\n            if( ret == 0 )\n            {\n                if( n > sizeof( esp8266_hal_sig ) ||\n                    6 + offset + n > MBEDTLS_SSL_OUT_CONTENT_LEN )\n                {\n                    ret = MBEDTLS_ERR_SSL_ALLOC_FAILED;\n                }\n                else\n                {\n                    ssl->out_msg[4] = esp8266_hal_hash_sig_alg;\n                    ssl->out_msg[5] = esp8266_hal_sig_alg;\n                    memcpy( ssl->out_msg + 6 + offset, esp8266_hal_sig, n );\n                }\n            }\n        }\n    }\n    if( ret != 0 )\n    {\n        esp8266_hal_eap_rsa_release();\n        MBEDTLS_SSL_DEBUG_RET( 1, \"mbedtls_pk_sign\", ret );\n#if defined(MBEDTLS_SSL__ECP_RESTARTABLE)\n        if( ret == MBEDTLS_ERR_ECP_IN_PROGRESS )\n            ret = MBEDTLS_ERR_SSL_CRYPTO_IN_PROGRESS;\n#endif\n        return( ret );\n    }\n    esp8266_hal_eap_rsa_release();",
        "ssl_cli.c certificate verify sign reserve",
    );
    fs::write(&patched, patched_text).expect("failed to write patched ssl_cli.c");
    patched
}

fn replace_required(source: &str, from: &str, to: &str, label: &str) -> String {
    let patched = source.replace(from, to);
    assert_ne!(patched, source, "failed to patch {label}");
    patched
}

/// Compile mbedTLS sources needed by wpa_supplicant and the TLS stack.
fn compile_mbedtls(sdk: &Path, include_paths: &[PathBuf], out_dir: &Path, gcc: Option<&str>) {
    let mut build = cc::Build::new();
    init_cc_build(&mut build, gcc, include_paths);
    let config_file = if env::var_os(MBEDTLS_SOFTWARE_AES_ENV).is_some() {
        let shim = out_dir.join("esp_config_software_aes.h");
        fs::write(
            &shim,
            "#include \"mbedtls/esp_config.h\"\n#undef MBEDTLS_AES_ALT\n",
        )
        .expect("failed to write software-AES mbedTLS config shim");
        "esp_config_software_aes.h"
    } else {
        "mbedtls/esp_config.h"
    };
    build
        .define("MBEDTLS_CONFIG_FILE", format!("\"{config_file}\"").as_str())
        .define("CONFIG_SSL_USING_MBEDTLS", None)
        .define("USING_IBUS_FASTER_GET", None);

    // mbedTLS library sources from the upstream mbedtls/library/ directory.
    let mbed_lib = sdk.join("components/mbedtls/mbedtls/library");
    let mbed_srcs = [
        "aes.c",
        "aesni.c",
        "arc4.c",
        "asn1parse.c",
        "asn1write.c",
        "base64.c",
        "bignum.c",
        "blowfish.c",
        "camellia.c",
        "ccm.c",
        "certs.c",
        "cipher.c",
        "cipher_wrap.c",
        "cmac.c",
        "ctr_drbg.c",
        "debug.c",
        "des.c",
        "dhm.c",
        "ecdh.c",
        "ecdsa.c",
        "ecjpake.c",
        "ecp.c",
        "ecp_curves.c",
        "entropy.c",
        "entropy_poll.c",
        "error.c",
        "gcm.c",
        "havege.c",
        "hmac_drbg.c",
        "md.c",
        "md2.c",
        "md4.c",
        "md5.c",
        "md_wrap.c",
        "memory_buffer_alloc.c",
        "oid.c",
        "padlock.c",
        "pem.c",
        "pk.c",
        "pk_wrap.c",
        "pkcs5.c",
        "pkcs11.c",
        "pkcs12.c",
        "pkparse.c",
        "pkwrite.c",
        "platform.c",
        "platform_util.c",
        "ripemd160.c",
        "rsa.c",
        "rsa_internal.c",
        "sha1.c",
        "sha256.c",
        "sha512.c",
        "ssl_cache.c",
        "ssl_ciphersuites.c",
        "ssl_cli.c",
        "ssl_cookie.c",
        "ssl_srv.c",
        "ssl_ticket.c",
        "ssl_tls.c",
        "threading.c",
        "timing.c",
        "version.c",
        "version_features.c",
        "x509.c",
        "x509_create.c",
        "x509_crl.c",
        "x509_crt.c",
        "x509_csr.c",
        "x509write_crt.c",
        "x509write_csr.c",
        "xtea.c",
    ];
    for src in &mbed_srcs {
        let path = mbed_lib.join(src);
        if path.exists() {
            match *src {
                "ssl_cli.c" => {
                    build.file(patch_ssl_cli_source(&path, out_dir));
                }
                "ssl_tls.c" => {
                    build.file(patch_ssl_tls_source(&path, out_dir));
                }
                _ => {
                    build.file(&path);
                }
            }
        }
    }

    // Port files
    let port_srcs = [
        "components/mbedtls/port/mbedtls_debug.c",
        "components/mbedtls/port/net_sockets.c",
        "components/mbedtls/port/esp_hardware.c",
        "components/mbedtls/port/esp_mem.c",
        "components/mbedtls/port/esp_timing.c",
        "components/mbedtls/port/esp_aes.c",
        "components/mbedtls/port/esp_sha1.c",
        "components/mbedtls/port/esp_sha256.c",
        "components/mbedtls/port/esp_sha512.c",
        "components/mbedtls/port/esp8266/aes.c",
        "components/mbedtls/port/esp8266/arc4.c",
        "components/mbedtls/port/esp8266/base64.c",
        "components/mbedtls/port/esp8266/md5.c",
        "components/mbedtls/port/esp8266/sha1.c",
        "components/mbedtls/port/esp8266/sha256.c",
        "components/mbedtls/port/esp8266/sha512.c",
    ];
    for src in &port_srcs {
        let path = sdk.join(src);
        if path.exists() {
            build.file(&path);
        }
    }

    build.compile("mbedtls_all");
}

// ===========================================================================
// Step 4: Link directives
// ===========================================================================

fn emit_link_directives(sdk: &Path) {
    let lib_dir = sdk.join("components/esp8266/lib");
    if lib_dir.exists() {
        println!("cargo:rustc-link-search=native={}", lib_dir.display());

        // Core WiFi blob libraries (always needed).
        for lib in &[
            "core", "net80211", "phy", "pp", "hal", "clk", "rtc", "gcc", "espnow",
        ] {
            println!("cargo:rustc-link-lib=static={lib}");
        }

        // Feature-gated libraries.
        if env::var("CARGO_FEATURE_SMARTCONFIG").is_ok() {
            println!("cargo:rustc-link-lib=static=smartconfig");
        }
    } else {
        println!(
            "cargo:warning=esp8266-wifi-sys: SDK lib directory not found: {}",
            lib_dir.display()
        );
    }

    // Emit linker script paths and include ROM/peripheral symbol definitions.
    let ld_dir = sdk.join("components/esp8266/ld");
    if ld_dir.exists() {
        println!("cargo:rustc-link-search=native={}", ld_dir.display());
        // ROM function addresses (Cache_Read_Disable, lldesc_build_chain, etc.)
        println!("cargo:rustc-link-arg=-Tesp8266.rom.ld");
        // Peripheral register addresses (GPIO, uart0, uart1, rtc_sys_info, etc.)
        println!("cargo:rustc-link-arg=-Tesp8266.peripherals.ld");
    }

    // Provide the toolchain libc.a
    if let Some(libc_dir) = find_toolchain_libc_dir() {
        println!("cargo:rustc-link-search=native={}", libc_dir.display());
        println!("cargo:rustc-link-lib=static=c");
        println!("cargo:rustc-link-lib=static=m");
    }

    // Export paths as DEP_ variables for downstream crates (examples-bin).
    println!(
        "cargo:ld-scripts={}",
        sdk.join("components/esp8266/ld").display()
    );
    println!("cargo:sdk-path={}", sdk.display());
    //     println!("cargo:rustc-link-arg=-Wl,--wrap=esp_phy_load_cal_and_init");
    //     println!("cargo:rustc-link-arg=-Wl,--wrap=phy_enable_agc");
    //     println!("cargo:rustc-link-arg=-Wl,--wrap=ieee80211_phy_init");
    //     println!("cargo:rustc-link-arg=-Wl,--wrap=lmacInit");
    //     println!("cargo:rustc-link-arg=-Wl,--wrap=wDev_Initialize");
    //     println!("cargo:rustc-link-arg=-Wl,--wrap=pp_attach");
    //     println!("cargo:rustc-link-arg=-Wl,--wrap=ieee80211_ifattach");
    //     println!("cargo:rustc-link-arg=-Wl,--wrap=pm_attach");
    //     println!("cargo:rustc-link-arg=-Wl,--wrap=wifi_nvs_get_sta_listen_interval");
    //     println!("cargo:rustc-link-arg=-Wl,--wrap=pm_reset_param");
    //     println!("cargo:rustc-link-arg=-Wl,--wrap=ppRegisterTxCallback");
    //     println!("cargo:rustc-link-arg=-Wl,--wrap=pm_goto_rf_off");
    //     println!("cargo:rustc-link-arg=-Wl,--wrap=_xt_isr_mask");
    //     println!("cargo:rustc-link-arg=-Wl,--wrap=cnx_attach");
    //     println!("cargo:rustc-link-arg=-Wl,--wrap=wifi_timer_init");
    //     println!("cargo:rustc-link-arg=-Wl,--wrap=esf_buf_setup");
    //     println!("cargo:rustc-link-arg=-Wl,--wrap=os_timer_setfn");
    // Keep only the functional wraps here as well. The connect/scan hot-path
    // tracing wraps (`scan_start`, `pm_scan_start`, `os_timer_*`) were useful
    // for earlier diagnosis, but they sit directly on the active discovery
    // path we are now trying to validate and can perturb that timing-sensitive
    // path themselves.
    // Re-enable only the smallest PP/timer stage hooks while investigating the
    // remaining zero-PC fault. The corresponding wrappers already exist in
    // `pp_stage.rs` and let the probes distinguish PP TX processing from ROM
    // timer dispatch without introducing new example-local instrumentation.
    println!("cargo:rustc-link-arg=-Wl,--wrap=esp_event_send"); // event bridge
    println!("cargo:rustc-link-arg=-Wl,--wrap=nvs_open"); // NVS RAM stub
    println!("cargo:rustc-link-arg=-Wl,--wrap=nvs_get_blob"); // NVS RAM stub
    println!("cargo:rustc-link-arg=-Wl,--wrap=ets_printf"); // no-op (ppTask stack overflow prevention)

    // Diagnostic timer/connect wrappers removed. The current goal is to match
    // the SDK station/tcp_server runtime shape as closely as possible, and the
    // remaining active investigation wraps were still changing that path.
    // REMOVED: --wrap=uart_tx_wait_idle (trace_char disrupts PHY calibration timing)
}

// ===========================================================================
// Toolchain discovery helpers
// ===========================================================================

fn sdk_looks_valid(path: &Path) -> bool {
    path.join("components/esp8266/include/esp_wifi.h").exists()
        && path.join("components/lwip/lwip/src/include").exists()
        && path.join("components/mbedtls/mbedtls/include").exists()
}

fn discover_sdk_path() -> PathBuf {
    // 1. Explicit environment variable override.
    if let Ok(path) = env::var("ESP8266_RTOS_SDK") {
        let p = PathBuf::from(path);
        if sdk_looks_valid(&p) {
            return p;
        }
        println!("cargo:warning=ESP8266_RTOS_SDK env var set but SDK not valid there");
    }

    // 2. Clone / open via embuild into ~/.espressif/ESP8266_RTOS_SDK
    let home = env::var("HOME").expect("HOME not set");
    let install_dir = PathBuf::from(home).join(".espressif");
    fs::create_dir_all(&install_dir).expect("failed to create ~/.espressif/");

    let repo_dir = install_dir.join("ESP8266_RTOS_SDK");

    println!("cargo:warning=Obtaining ESP8266 RTOS SDK ({SDK_COMMIT})...");

    let mut repo = git::Repository::new(&repo_dir);

    if !repo_dir.join(".git").exists() {
        let clone_opts = git::CloneOptions::new()
            .force_ref(git::Ref::Commit(SDK_COMMIT.to_string()))
            .depth(1);
        repo.clone_ext(SDK_REPO_URL, clone_opts)
            .expect("failed to clone ESP8266 RTOS SDK");
    }

    let path = repo.worktree().to_path_buf();

    assert!(
        sdk_looks_valid(&path),
        "Cloned SDK at {} is missing required files",
        path.display()
    );

    path
}

/// Find newlib include path from the xtensa-lx106-elf-gcc sysroot.
///
/// Resolves the GCC binary location and derives the sysroot as
/// `{gcc_prefix}/xtensa-lx106-elf/include` (since `-print-sysroot` returns
/// empty for this toolchain).
fn find_newlib_include() -> Option<PathBuf> {
    let gcc = find_xtensa_gcc()?;
    let gcc_path = PathBuf::from(&gcc);

    // Resolve symlinks to get the real binary path.
    let gcc_real = fs::canonicalize(&gcc_path).unwrap_or(gcc_path);
    // .../xtensa-lx106-elf/bin/xtensa-lx106-elf-gcc → .../xtensa-lx106-elf/
    let prefix = gcc_real.parent()?.parent()?;
    let inc = prefix.join("xtensa-lx106-elf/include");
    if inc.join("stdio.h").exists() {
        return Some(inc);
    }

    None
}

fn find_esp_clang_lib() -> Option<String> {
    let rustup_home = env::var("RUSTUP_HOME").unwrap_or_else(|_| {
        let home = env::var("HOME").unwrap_or_default();
        format!("{home}/.rustup")
    });

    let toolchain_dir = PathBuf::from(&rustup_home).join("toolchains/esp");
    if !toolchain_dir.exists() {
        return None;
    }

    let clang_base = toolchain_dir.join("xtensa-esp32-elf-clang");
    if !clang_base.exists() {
        return None;
    }

    if let Ok(entries) = std::fs::read_dir(&clang_base) {
        for entry in entries.flatten() {
            let lib_dir = entry.path().join("esp-clang/lib");
            if lib_dir.join("libclang.so").exists() {
                return Some(lib_dir.to_string_lossy().into_owned());
            }
        }
    }

    None
}

/// Find the Xtensa LX106 GCC cross-compiler for ESP8266.
///
/// Checks `XTENSA_LX106_GCC` env var first, then falls back to PATH.
fn find_xtensa_gcc() -> Option<String> {
    if let Ok(gcc) = env::var("XTENSA_LX106_GCC") {
        if PathBuf::from(&gcc).exists() {
            return Some(gcc);
        }
    }

    if let Ok(output) = Command::new("which").arg("xtensa-lx106-elf-gcc").output() {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Some(path);
            }
        }
    }

    None
}

/// Find the directory containing the toolchain's libc.a for linking.
///
/// Derives from the GCC binary location, same approach as `find_newlib_include()`.
fn find_toolchain_libc_dir() -> Option<PathBuf> {
    let gcc = find_xtensa_gcc()?;
    let gcc_path = PathBuf::from(&gcc);
    let gcc_real = fs::canonicalize(&gcc_path).unwrap_or(gcc_path);
    let prefix = gcc_real.parent()?.parent()?;
    let lib_dir = prefix.join("xtensa-lx106-elf/lib");
    if lib_dir.join("libc.a").exists() {
        return Some(lib_dir);
    }
    None
}
