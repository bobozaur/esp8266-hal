use std::{
    env,
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, SocketAddrV4, TcpListener, TcpStream, UdpSocket},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, bail, Context, Result};
use regex::bytes::Regex;

use crate::esp32_companion;

const TARGET: &str = "xtensa-esp8266-none-elf";
const BUILD_STD: &str = "-Zbuild-std=core";
const DEFAULT_UART_BAUD: u32 = 115_200;
const DEFAULT_FLASH_BAUD: u32 = 460_800;
const DEFAULT_SERIAL_PORT: &str = "/dev/ttyUSB0";
const DEFAULT_COMPANION_PORT: &str = esp32_companion::DEFAULT_COMPANION_PORT;
const DEFAULT_ROUTER_HOST: &str = "10.42.0.131";
const DEFAULT_ROUTER_USER: &str = "root";
const DEFAULT_ROUTER_IDENTITY: &str = "target/wifi-hw-router/id_rsa";
const DEFAULT_ROUTER_KNOWN_HOSTS: &str = "/tmp/esp8266-hal-openwrt-known-hosts";
const DEFAULT_ROUTER_HOSTAPD_IFACE: &str = "phy0-ap0";
const DEFAULT_MATRIX_ATTEMPTS: usize = 3;
const MATRIX_RETRY_BACKOFF: Duration = Duration::from_secs(2);
const COMPANION_TCP_CONNECT_TIMEOUT_MS: u32 = 20_000;
const WPS_ROUTER_SSID: &str = "esp8266-hal-wps";
const WPS_ROUTER_PASSPHRASE: &str = "esp8266wps";
const WPS_PIN: &str = "12345670";
const WPS_HARNESS_MAX_ATTEMPTS: &str = "3";
const DEFAULT_WPS_DUT_MAC: &str = "48:3f:da:68:e3:04";
const DEFAULT_WPS_DUT_UUID: &str = "df2d477b-b793-51e8-a89b-5aec91edcbf9";
const SMARTCONFIG_ROUTER_SSID: &str = "esp8266-hal-smart";
const SMARTCONFIG_ROUTER_PASSPHRASE: &str = "esp8266smart";
const DEFAULT_SMARTCONFIG_CHANNEL: u8 = 6;
const SKIP_UNCHANGED_FLASH_ENV: &str = "ESP8266_HAL_WIFI_HW_SKIP_UNCHANGED_FLASH";
const FLASH_FINGERPRINT_OFFSET_A: u64 = 0xcbf2_9ce4_8422_2325;
const FLASH_FINGERPRINT_OFFSET_B: u64 = 0x8422_2325_cbf2_9ce4;
const FLASH_FINGERPRINT_PRIME: u64 = 0x0000_0100_0000_01b3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Text,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScenarioState {
    Implemented,
    Manual,
    RigBlocked { reason: &'static str },
    PendingExample,
}

impl ScenarioState {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Implemented => "implemented",
            Self::Manual => "manual",
            Self::RigBlocked { .. } => "rig-blocked",
            Self::PendingExample => "pending-example",
        }
    }

    pub const fn rig_blocked_reason(self) -> Option<&'static str> {
        match self {
            Self::RigBlocked { reason } => Some(reason),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scenario {
    pub name: &'static str,
    pub description: &'static str,
    pub example: Option<&'static str>,
    pub features: &'static [&'static str],
    pub profile: &'static str,
    pub state: ScenarioState,
    pub runtime: RuntimePlan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FlashImage {
    address: u32,
    path: PathBuf,
}

#[derive(Debug, Clone, Copy)]
struct FlashFingerprint {
    a: u64,
    b: u64,
}

impl FlashFingerprint {
    const fn new() -> Self {
        Self {
            a: FLASH_FINGERPRINT_OFFSET_A,
            b: FLASH_FINGERPRINT_OFFSET_B,
        }
    }

    fn update(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.a ^= u64::from(*byte);
            self.a = self.a.wrapping_mul(FLASH_FINGERPRINT_PRIME);

            self.b ^= u64::from(byte.reverse_bits());
            self.b = self.b.wrapping_mul(FLASH_FINGERPRINT_PRIME);
        }
    }

    fn finish(self) -> String {
        format!("{:016x}{:016x}", self.a, self.b)
    }
}

#[derive(Debug, Clone)]
struct RouterRig {
    host: String,
    user: String,
    identity_file: Option<PathBuf>,
    known_hosts_file: PathBuf,
    hostapd_iface: String,
}

#[derive(Debug, Clone)]
struct RouterWpsFixture {
    router: RouterRig,
    backup_path: String,
    dut_mac: String,
    dut_uuid: String,
    mode: WpsRouterMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WpsRouterMode {
    Pbc,
    Pin,
}

impl WpsRouterMode {
    fn from_env() -> Result<Self> {
        let value = env::var("ESP8266_HAL_WPS_MODE").unwrap_or_else(|_| "pbc".to_owned());
        Self::from_str(&value)
    }

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "pbc" => Ok(Self::Pbc),
            "pin" => Ok(Self::Pin),
            value => bail!("ESP8266_HAL_WPS_MODE must be `pbc` or `pin`, got `{value}`"),
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Pbc => "pbc",
            Self::Pin => "pin",
        }
    }

    fn build_env(self) -> Option<(&'static str, String)> {
        match self {
            Self::Pbc => None,
            Self::Pin => Some(("ESP8266_HAL_WPS_PIN", WPS_PIN.to_owned())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SmartConfigSenderKind {
    RouterLocal,
    CompanionSta,
    CompanionAp,
}

impl SmartConfigSenderKind {
    fn from_env() -> Result<Self> {
        let value =
            env::var("ESP8266_HAL_SMARTCONFIG_SENDER").unwrap_or_else(|_| "companion-ap".into());
        match value.as_str() {
            "router-local" => Ok(Self::RouterLocal),
            "companion-sta" => Ok(Self::CompanionSta),
            "companion-ap" => Ok(Self::CompanionAp),
            value => bail!(
                "ESP8266_HAL_SMARTCONFIG_SENDER must be `router-local`, `companion-sta`, or `companion-ap`, got `{value}`"
            ),
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::RouterLocal => "router-local",
            Self::CompanionSta => "companion-sta",
            Self::CompanionAp => "companion-ap",
        }
    }

    const fn needs_router_ap(self) -> bool {
        matches!(self, Self::RouterLocal | Self::CompanionSta)
    }

    const fn default_start_after_scan(self) -> bool {
        matches!(self, Self::CompanionAp)
    }

    fn build_env(self) -> Vec<(&'static str, String)> {
        match self {
            Self::CompanionAp => vec![("ESP8266_HAL_SMARTCONFIG_STATIC_ACK", "1".to_owned())],
            Self::RouterLocal | Self::CompanionSta => Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
struct RouterSmartConfigFixture {
    router: RouterRig,
    backup_path: String,
    ssid: String,
    passphrase: String,
    channel: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimePlan {
    UartWatch {
        timeout: Duration,
        success_markers: &'static [&'static str],
        failure_markers: &'static [&'static str],
    },
    TcpEcho {
        timeout: Duration,
        port: u16,
        dhcp_regex: &'static str,
        listen_marker: &'static str,
        failure_markers: &'static [&'static str],
        payload: &'static [u8],
    },
    TcpClient {
        timeout: Duration,
        port: u16,
        success_markers: &'static [&'static str],
        failure_markers: &'static [&'static str],
        response: &'static [u8],
    },
    CompanionRig,
    Pending,
}

impl RuntimePlan {
    pub const fn label(self) -> &'static str {
        match self {
            Self::UartWatch { .. } => "uart-watch",
            Self::TcpEcho { .. } => "tcp-echo",
            Self::TcpClient { .. } => "tcp-client",
            Self::CompanionRig => "companion-rig",
            Self::Pending => "pending",
        }
    }
}

const WIFI_SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "sta_connect",
        description: "associate and report WiFi link-up from the typestate core path",
        example: Some("wifi_core_connect"),
        features: &["interrupt", "rt", "wifi-core", "wifi", "wifi-sta"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::UartWatch {
            timeout: Duration::from_secs(105),
            success_markers: &["wifi core connected"],
            failure_markers: &[
                "wifi core connect err",
                "DISCONNECT reason=",
                "panic",
                "Guru Meditation",
            ],
        },
    },
    Scenario {
        name: "sta_wrong_password",
        description: "fail cleanly when station credentials do not match a secured AP",
        example: Some("wifi_core_connect"),
        features: &["interrupt", "rt", "wifi-core", "wifi", "wifi-sta"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::UartWatch {
            timeout: Duration::from_secs(75),
            success_markers: &["wifi core connect err"],
            failure_markers: &[
                "wifi core associated",
                "wifi core connected",
                "panic",
                "Guru Meditation",
            ],
        },
    },
    Scenario {
        name: "sta_wpa2_connect",
        description: "associate with a visible WPA2-PSK AP through the typestate core path",
        example: Some("wifi_core_connect"),
        features: &["interrupt", "rt", "wifi-core", "wifi", "wifi-sta"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::UartWatch {
            timeout: Duration::from_secs(105),
            success_markers: &["wifi core connected"],
            failure_markers: &[
                "wifi core connect err",
                "DISCONNECT reason=",
                "panic",
                "Guru Meditation",
            ],
        },
    },
    Scenario {
        name: "sta_stale_channel_hint",
        description: "associate when a station channel hint points away from the target AP",
        example: Some("wifi_core_connect"),
        features: &["interrupt", "rt", "wifi-core", "wifi", "wifi-sta"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::UartWatch {
            timeout: Duration::from_secs(105),
            success_markers: &["wifi core connected"],
            failure_markers: &[
                "wifi core connect err",
                "DISCONNECT reason=",
                "panic",
                "Guru Meditation",
            ],
        },
    },
    Scenario {
        name: "sta_hidden_ssid",
        description: "associate with a non-broadcast WPA2 SSID through the typestate core path",
        example: Some("wifi_core_connect"),
        features: &["interrupt", "rt", "wifi-core", "wifi", "wifi-sta"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::UartWatch {
            timeout: Duration::from_secs(105),
            success_markers: &["wifi core connected"],
            failure_markers: &[
                "wifi core connect err",
                "DISCONNECT reason=",
                "panic",
                "Guru Meditation",
            ],
        },
    },
    Scenario {
        name: "sta_scan_connect",
        description: "associate from the typestate core path without a station channel hint",
        example: Some("wifi_core_connect"),
        features: &["interrupt", "rt", "wifi-core", "wifi", "wifi-sta"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::UartWatch {
            timeout: Duration::from_secs(105),
            success_markers: &["wifi core connected"],
            failure_markers: &[
                "wifi core connect err",
                "DISCONNECT reason=",
                "panic",
                "Guru Meditation",
            ],
        },
    },
    Scenario {
        name: "sta_scan",
        description: "perform a station active scan through the typestate core path",
        example: Some("raw_scan"),
        features: &["interrupt", "rt", "wifi-core", "wifi", "wifi-sta"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::UartWatch {
            timeout: Duration::from_secs(30),
            success_markers: &["scan-ok"],
            failure_markers: &["wifi core scan error", "panic", "Guru Meditation"],
        },
    },
    Scenario {
        name: "sta_dhcp",
        description: "bring the station link up and observe DHCP completion",
        example: Some("wifi_core_dhcp_probe"),
        features: &["interrupt", "rt", "wifi-embassy-net", "wifi-sta"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::UartWatch {
            timeout: Duration::from_secs(105),
            success_markers: &["dhcp up Cidr", "dhcp:up", "config up"],
            failure_markers: &[
                "wifi connect err",
                "dhcp timeout",
                "dhcp wake no-v4",
                "DISCONNECT reason=",
                "panic",
                "Guru Meditation",
            ],
        },
    },
    Scenario {
        name: "sta_static_ip",
        description: "bring the station link up with a Rust-owned static IPv4 config",
        example: Some("sta_static_ip"),
        features: &["interrupt", "rt", "wifi-embassy-net", "wifi-sta"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::UartWatch {
            timeout: Duration::from_secs(105),
            success_markers: &["static ip up"],
            failure_markers: &[
                "DISCONNECT reason=",
                "static ip wifi err",
                "static ip config err",
                "static ip config-up timeout",
                "panic",
                "Guru Meditation",
            ],
        },
    },
    Scenario {
        name: "tcp_server",
        description: "run the embassy-net TCP server and verify first-attempt echo",
        example: Some("tcp_server"),
        features: &["interrupt", "rt", "wifi-embassy-net", "wifi-sta"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::TcpEcho {
            timeout: Duration::from_secs(105),
            port: 1234,
            dhcp_regex: r"dhcp up Cidr \{ address: ([0-9]+(?:\.[0-9]+){3}), prefix_len: ([0-9]+) \}",
            listen_marker: "listen 1234",
            failure_markers: &[
                "wifi start err",
                "tcp server wifi err",
                "tcp server dhcp timeout",
                "tcp server dhcp no-v4",
                "accept err",
                "write err",
                "flush err",
                "read err",
                "DISCONNECT reason=",
                "panic",
                "Guru Meditation",
            ],
            payload: b"ping",
        },
    },
    Scenario {
        name: "tcp_client",
        description: "run the embassy-net TCP client and verify outbound connect/send",
        example: Some("tcp_client"),
        features: &["interrupt", "rt", "wifi-embassy-net", "wifi-sta"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::TcpClient {
            timeout: Duration::from_secs(135),
            port: 4321,
            success_markers: &["tcp client connected", "recv:", "done"],
            failure_markers: &[
                "DISCONNECT reason=",
                "tcp client wifi err",
                "tcp client dhcp err",
                "tcp client dhcp timeout",
                "tcp client connect err",
                "tcp client connect timeout",
                "tcp client write err",
                "tcp client flush err",
                "tcp client read err",
                "tcp client closed",
                "panic",
                "Guru Meditation",
            ],
            response: b"HTTP/1.0 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\npong",
        },
    },
    Scenario {
        name: "eap_sta_assoc",
        description: "associate to the OpenWrt WPA2 Enterprise AP through the RADIUS fixture",
        example: Some("wifi_eap"),
        features: &["interrupt", "rt", "wifi-enterprise", "wifi-sta"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::UartWatch {
            timeout: Duration::from_secs(150),
            success_markers: &["eap-ok"],
            failure_markers: &[
                "wifi eap configure err",
                "wifi eap start err",
                "wifi eap connect err",
                "DISCONNECT reason=",
                "panic",
                "Guru Meditation",
            ],
        },
    },
    Scenario {
        name: "eap_wrong_password",
        description: "fail closed when WPA2 Enterprise credentials are rejected by RADIUS",
        example: Some("wifi_eap"),
        features: &["interrupt", "rt", "wifi-enterprise", "wifi-sta"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::UartWatch {
            timeout: Duration::from_secs(150),
            success_markers: &["eap-negative-ok"],
            failure_markers: &[
                "wifi eap configure err",
                "wifi eap start err",
                "wifi eap associated",
                "eap-ok",
                "eap-negative-unexpected-success",
                "panic",
                "Guru Meditation",
            ],
        },
    },
    Scenario {
        name: "ap_station_list_companion",
        description: "join the soft-AP with the ESP32 companion and validate station list/deauth",
        example: Some("wifi_ap"),
        features: &["interrupt", "rt", "wifi-ap"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::CompanionRig,
    },
    Scenario {
        name: "ap_rejoin_companion",
        description: "rejoin the soft-AP with the ESP32 companion after DUT-initiated deauth",
        example: Some("wifi_ap"),
        features: &["interrupt", "rt", "wifi-ap"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::CompanionRig,
    },
    Scenario {
        name: "ap_attach_companion",
        description: "join the soft-AP after attaching the AP RX path without embassy-net",
        example: Some("wifi_ap"),
        features: &["interrupt", "rt", "wifi-ap"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::CompanionRig,
    },
    Scenario {
        name: "ap_net_attach_companion",
        description: "join the soft-AP after creating the AP network runtime but before entering the DHCP loop",
        example: Some("wifi_ap"),
        features: &["interrupt", "rt", "wifi-ap"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::CompanionRig,
    },
    Scenario {
        name: "ap_dhcp_bind_companion",
        description: "join the soft-AP after binding the Rust DHCP socket but before entering the DHCP poll loop",
        example: Some("wifi_ap"),
        features: &["interrupt", "rt", "wifi-ap"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::CompanionRig,
    },
    Scenario {
        name: "ap_dhcp_companion",
        description: "join the soft-AP with the ESP32 companion and validate Rust DHCP service",
        example: Some("wifi_ap"),
        features: &["interrupt", "rt", "wifi-ap"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::CompanionRig,
    },
    Scenario {
        name: "ap_sta_downstream_companion",
        description: "validate AP+STA mixed mode with the ESP32 companion as downstream client",
        example: Some("wifi_ap_sta"),
        features: &["interrupt", "rt", "wifi-ap", "wifi-sta", "wifi-embassy-net"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::CompanionRig,
    },
    Scenario {
        name: "esp_now_unicast_companion",
        description: "exchange ESP-NOW unicast payloads with the ESP32 companion",
        example: Some("wifi_esp_now_rx"),
        features: &["interrupt", "rt", "wifi-sta", "wifi-esp-now"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::CompanionRig,
    },
    Scenario {
        name: "esp_now_encrypted_companion",
        description: "exchange encrypted ESP-NOW payloads with the ESP32 companion",
        example: Some("wifi_esp_now_rx"),
        features: &["interrupt", "rt", "wifi-sta", "wifi-esp-now"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::CompanionRig,
    },
    Scenario {
        name: "esp_now_bad_key_companion",
        description: "validate encrypted ESP-NOW failure cases with the ESP32 companion",
        example: Some("wifi_esp_now_rx"),
        features: &["interrupt", "rt", "wifi-sta", "wifi-esp-now"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::CompanionRig,
    },
    Scenario {
        name: "promisc_data_companion",
        description: "validate promiscuous data capture with repeatable ESP32 companion traffic",
        example: Some("wifi_promisc"),
        features: &["interrupt", "rt", "wifi-sta", "wifi-promiscuous"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::CompanionRig,
    },
    Scenario {
        name: "ap_start",
        description: "start the soft-AP and validate a client join path",
        example: Some("wifi_ap"),
        features: &["interrupt", "rt", "wifi-ap"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::UartWatch {
            timeout: Duration::from_secs(10),
            success_markers: &["ap-start-ok"],
            failure_markers: &["ap start err", "panic", "Guru Meditation"],
        },
    },
    Scenario {
        name: "ap_station_list",
        description: "join the soft-AP with a client, enumerate it, and deauth all stations",
        example: Some("wifi_ap"),
        features: &["interrupt", "rt", "wifi-ap"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::UartWatch {
            timeout: Duration::from_secs(70),
            success_markers: &["ap-station-list-ok"],
            failure_markers: &[
                "ap start err",
                "ap wait client timeout",
                "ap deauth err",
                "panic",
                "Guru Meditation",
            ],
        },
    },
    Scenario {
        name: "ap_dhcp",
        description: "join the soft-AP with a client and validate Rust DHCP lease service",
        example: Some("wifi_ap"),
        features: &["interrupt", "rt", "wifi-ap"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::UartWatch {
            timeout: Duration::from_secs(90),
            success_markers: &["ap-dhcp-ok"],
            failure_markers: &[
                "ap start err",
                "ap-net-timeout",
                "ap-dhcp bind err",
                "panic",
                "Guru Meditation",
            ],
        },
    },
    Scenario {
        name: "ap_sta",
        description: "run AP+STA mixed mode with upstream and downstream connectivity",
        example: Some("wifi_ap_sta"),
        features: &["interrupt", "rt", "wifi-ap", "wifi-sta", "wifi-embassy-net"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::UartWatch {
            timeout: Duration::from_secs(45),
            success_markers: &["ap-sta-ok"],
            failure_markers: &[
                "ap_sta err connect-timeout",
                "ap_sta err connect-disconnected",
                "ap_sta err connect-backend",
                "ap_sta err connect-other",
                "ap_sta err stations",
                "ap_sta err dhcp",
                "ap_sta err dhcp-timeout",
                "DISCONNECT reason=",
                "panic",
                "Guru Meditation",
            ],
        },
    },
    Scenario {
        name: "wps",
        description: "complete WPS PBC enrollment on supported router hardware",
        example: Some("wifi_wps"),
        features: &["interrupt", "rt", "wifi-sta", "wifi-wps"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::UartWatch {
            timeout: Duration::from_secs(180),
            success_markers: &["wps-ok"],
            failure_markers: &[
                "wps sta start err",
                "wps session err",
                "wps start err",
                "wps-failed",
                "wps-timeout",
                "panic",
                "Guru Meditation",
            ],
        },
    },
    Scenario {
        name: "wps_pin",
        description: "complete WPS fixed-PIN enrollment on supported router hardware",
        example: Some("wifi_wps"),
        features: &["interrupt", "rt", "wifi-sta", "wifi-wps"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::UartWatch {
            timeout: Duration::from_secs(180),
            success_markers: &["wps-ok"],
            failure_markers: &[
                "wps sta start err",
                "wps session err",
                "wps start err",
                "wps-failed",
                "wps-timeout",
                "panic",
                "Guru Meditation",
            ],
        },
    },
    Scenario {
        name: "smartconfig",
        description: "complete SmartConfig provisioning from an ESPTouch-compatible sender",
        example: Some("wifi_smartconfig"),
        features: &["interrupt", "rt", "wifi-sta", "wifi-smartconfig"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::UartWatch {
            timeout: Duration::from_secs(180),
            success_markers: &["smartconfig-ok"],
            failure_markers: &[
                "smartconfig sta start err",
                "smartconfig session err",
                "smartconfig start err",
                "smartconfig credentials err",
                "smartconfig configure err",
                "smartconfig connect err",
                "smartconfig stop err",
                "smartconfig net err",
                "smartconfig mac err",
                "smartconfig alloc err",
                "smartconfig dhcp err",
                "smartconfig ack err",
                "smartconfig ack bind err",
                "smartconfig ack send err",
                "panic",
                "Guru Meditation",
            ],
        },
    },
    Scenario {
        name: "esp_now_unicast",
        description: "exchange ESP-NOW unicast payloads between two boards",
        example: Some("wifi_esp_now_rx"),
        features: &["interrupt", "rt", "wifi-sta", "wifi-esp-now"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::UartWatch {
            timeout: Duration::from_secs(30),
            success_markers: &["esp-now-tx-ok"],
            failure_markers: &[
                "esp-now start err",
                "esp-now session err",
                "esp-now channel err",
                "esp-now peer err",
                "esp-now send err",
                "esp-now send status failed",
                "esp-now send status timeout",
                "panic",
                "Guru Meditation",
            ],
        },
    },
    Scenario {
        name: "esp_now_broadcast",
        description: "send ESP-NOW broadcast payloads from one board and validate SDK send path",
        example: Some("wifi_esp_now_tx"),
        features: &["interrupt", "rt", "wifi-sta", "wifi-esp-now"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::UartWatch {
            timeout: Duration::from_secs(20),
            success_markers: &["esp-now-tx-ok"],
            failure_markers: &[
                "esp-now start err",
                "esp-now session err",
                "esp-now channel err",
                "esp-now peer err",
                "esp-now send err",
                "esp-now send status failed",
                "esp-now send status timeout",
                "panic",
                "Guru Meditation",
            ],
        },
    },
    Scenario {
        name: "esp_now_encrypted",
        description: "exchange encrypted ESP-NOW payloads between two boards",
        example: Some("wifi_esp_now_rx"),
        features: &["interrupt", "rt", "wifi-sta", "wifi-esp-now"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::UartWatch {
            timeout: Duration::from_secs(30),
            success_markers: &["esp-now-tx-ok"],
            failure_markers: &[
                "esp-now start err",
                "esp-now session err",
                "esp-now channel err",
                "esp-now peer err",
                "esp-now send err",
                "esp-now send status failed",
                "esp-now send status timeout",
                "panic",
                "Guru Meditation",
            ],
        },
    },
    Scenario {
        name: "promisc_capture",
        description: "capture management/data/control frames in promiscuous mode",
        example: Some("wifi_promisc"),
        features: &["interrupt", "rt", "wifi-sta", "wifi-promiscuous"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::UartWatch {
            timeout: Duration::from_secs(25),
            success_markers: &["promisc-ok"],
            failure_markers: &[
                "promisc start err",
                "promisc set channel err",
                "promisc enable err",
                "promisc ctrl filter err",
                "promisc api state err",
                "promisc api restart err",
                "promisc stop err",
                "promisc session err",
                "promisc timeout",
                "panic",
                "Guru Meditation",
            ],
        },
    },
    Scenario {
        name: "promisc_filters",
        description: "validate promiscuous packet and control filters",
        example: Some("wifi_promisc"),
        features: &["interrupt", "rt", "wifi-sta", "wifi-promiscuous"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::UartWatch {
            timeout: Duration::from_secs(25),
            success_markers: &["promisc-management-ok"],
            failure_markers: &[
                "promisc start err",
                "promisc set channel err",
                "promisc enable err",
                "promisc ctrl filter err",
                "promisc api state err",
                "promisc api restart err",
                "promisc stop err",
                "promisc session err",
                "promisc timeout",
                "panic",
                "Guru Meditation",
            ],
        },
    },
    Scenario {
        name: "promisc_control_filter_api",
        description: "validate promiscuous control-subtype filter setup without RF control-frame injection",
        example: Some("wifi_promisc"),
        features: &["interrupt", "rt", "wifi-sta", "wifi-promiscuous"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::UartWatch {
            timeout: Duration::from_secs(25),
            success_markers: &["promisc-control-api-ok"],
            failure_markers: &[
                "promisc start err",
                "promisc set channel err",
                "promisc enable err",
                "promisc ctrl filter err",
                "promisc api state err",
                "promisc api restart err",
                "promisc stop err",
                "promisc session err",
                "promisc timeout",
                "panic",
                "Guru Meditation",
            ],
        },
    },
    Scenario {
        name: "promisc_control_filters",
        description: "validate promiscuous control-frame capture with RF traffic",
        example: Some("wifi_promisc"),
        features: &["interrupt", "rt", "wifi-sta", "wifi-promiscuous"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::UartWatch {
            timeout: Duration::from_secs(25),
            success_markers: &["promisc-control-ok"],
            failure_markers: &[
                "promisc start err",
                "promisc set channel err",
                "promisc enable err",
                "promisc ctrl filter err",
                "promisc api state err",
                "promisc api restart err",
                "promisc stop err",
                "promisc session err",
                "promisc timeout",
                "panic",
                "Guru Meditation",
            ],
        },
    },
];

pub fn scenario_names() -> Vec<&'static str> {
    WIFI_SCENARIOS
        .iter()
        .map(|scenario| scenario.name)
        .collect()
}

pub fn scenario_by_name(name: &str) -> Option<&'static Scenario> {
    WIFI_SCENARIOS.iter().find(|scenario| scenario.name == name)
}

pub fn scenario_resource_labels(scenario: &Scenario) -> Vec<&'static str> {
    let mut resources = Vec::new();

    if scenario_uses_router(scenario.name) {
        resources.push("router");
    }
    if scenario_uses_companion_resource(scenario.name) {
        resources.push("esp32-companion");
    }
    if resources.is_empty() {
        resources.push("none");
    }

    resources
}

pub fn scenario_matrix_lane_label(scenario: &Scenario) -> &'static str {
    if matrix_secondary_lane_candidate(scenario) {
        "secondary-candidate"
    } else {
        "primary"
    }
}

pub const fn matrix_attempt_limit() -> usize {
    DEFAULT_MATRIX_ATTEMPTS
}

pub fn artifacts_root(explicit: Option<&Path>) -> PathBuf {
    explicit
        .map(Path::to_path_buf)
        .or_else(|| env::var_os("ESP8266_HAL_WIFI_HW_ROOT").map(PathBuf::from))
        .unwrap_or_else(default_artifacts_root)
}

fn default_artifacts_root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .unwrap_or(manifest_dir)
        .join("target/wifi-hw-artifacts")
}

pub fn elf2image_args(out_prefix: &Path, example_path: &Path) -> Vec<OsString> {
    vec![
        OsString::from("--chip"),
        OsString::from("esp8266"),
        OsString::from("elf2image"),
        OsString::from("--version"),
        OsString::from("1"),
        OsString::from("--output"),
        out_prefix.as_os_str().to_owned(),
        example_path.as_os_str().to_owned(),
    ]
}

pub fn run_cli(args: Vec<String>) -> Result<()> {
    let (command, rest) = args
        .split_first()
        .ok_or_else(|| anyhow!("usage: wifi-hw <list|doctor|run|matrix>"))?;

    match command.as_str() {
        "list" => list(parse_list_format(rest)?),
        "companion" => crate::esp32_companion::run_cli(rest.to_vec()),
        "doctor" => {
            ensure_no_extra_args(rest, "doctor")?;
            doctor()
        }
        "run" => {
            let options = parse_run_options(rest, true)?;
            let scenario = options.scenario.as_deref().ok_or_else(|| {
                anyhow!(
                    "usage: wifi-hw run <scenario> [--root PATH] [--serial-port PATH] [--companion-port PATH]"
                )
            })?;
            run_scenario(
                scenario,
                options.root.as_deref(),
                &options.serial_port,
                &options.companion_port,
            )
        }
        "matrix" => {
            let options = parse_matrix_options(rest)?;
            if options.dry_run {
                matrix_dry_run(
                    options.root.as_deref(),
                    &options.serial_port,
                    &options.companion_port,
                    options.secondary_serial_port.as_deref(),
                    options.jobs,
                )
            } else {
                run_matrix(
                    options.root.as_deref(),
                    &options.serial_port,
                    &options.companion_port,
                    options.secondary_serial_port.as_deref(),
                    options.jobs,
                )
            }
        }
        "--help" | "-h" | "help" => {
            print_help();
            Ok(())
        }
        other => bail!("unknown wifi-hw command `{other}`"),
    }
}

#[derive(Debug)]
struct RunOptions {
    scenario: Option<String>,
    root: Option<PathBuf>,
    serial_port: PathBuf,
    companion_port: PathBuf,
}

#[derive(Debug)]
struct MatrixOptions {
    root: Option<PathBuf>,
    serial_port: PathBuf,
    companion_port: PathBuf,
    secondary_serial_port: Option<PathBuf>,
    jobs: usize,
    dry_run: bool,
}

fn parse_list_format(args: &[String]) -> Result<OutputFormat> {
    match args {
        [] => Ok(OutputFormat::Text),
        [flag, value] if flag == "--format" && value == "text" => Ok(OutputFormat::Text),
        [flag, value] if flag == "--format" && value == "json" => Ok(OutputFormat::Json),
        _ => bail!("usage: wifi-hw list [--format text|json]"),
    }
}

fn parse_run_options(args: &[String], require_scenario: bool) -> Result<RunOptions> {
    let mut scenario = None;
    let mut root = None;
    let mut serial_port = PathBuf::from(DEFAULT_SERIAL_PORT);
    let mut companion_port = env::var_os("ESP8266_HAL_COMPANION_PORT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_COMPANION_PORT));
    let mut iter = args.iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--root" => {
                let path = iter
                    .next()
                    .ok_or_else(|| anyhow!("--root requires a path"))?;
                root = Some(PathBuf::from(path));
            }
            "--serial-port" => {
                let path = iter
                    .next()
                    .ok_or_else(|| anyhow!("--serial-port requires a path"))?;
                serial_port = PathBuf::from(path);
            }
            "--companion-port" => {
                let path = iter
                    .next()
                    .ok_or_else(|| anyhow!("--companion-port requires a path"))?;
                companion_port = PathBuf::from(path);
            }
            value if value.starts_with('-') => bail!("unknown option `{value}`"),
            value => {
                if scenario.replace(value.to_owned()).is_some() {
                    bail!("only one scenario can be supplied");
                }
            }
        }
    }

    if require_scenario && scenario.is_none() {
        bail!(
            "usage: wifi-hw run <scenario> [--root PATH] [--serial-port PATH] [--companion-port PATH]"
        );
    }

    Ok(RunOptions {
        scenario,
        root,
        serial_port,
        companion_port,
    })
}

fn parse_matrix_options(args: &[String]) -> Result<MatrixOptions> {
    let mut root = None;
    let mut serial_port = PathBuf::from(DEFAULT_SERIAL_PORT);
    let mut companion_port = env::var_os("ESP8266_HAL_COMPANION_PORT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_COMPANION_PORT));
    let mut secondary_serial_port =
        env::var_os("ESP8266_HAL_SECONDARY_SERIAL_PORT").map(PathBuf::from);
    let mut jobs = 1_usize;
    let mut dry_run = false;
    let mut iter = args.iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--root" => {
                let path = iter
                    .next()
                    .ok_or_else(|| anyhow!("--root requires a path"))?;
                root = Some(PathBuf::from(path));
            }
            "--serial-port" => {
                let path = iter
                    .next()
                    .ok_or_else(|| anyhow!("--serial-port requires a path"))?;
                serial_port = PathBuf::from(path);
            }
            "--companion-port" => {
                let path = iter
                    .next()
                    .ok_or_else(|| anyhow!("--companion-port requires a path"))?;
                companion_port = PathBuf::from(path);
            }
            "--secondary-serial-port" => {
                let path = iter
                    .next()
                    .ok_or_else(|| anyhow!("--secondary-serial-port requires a path"))?;
                secondary_serial_port = Some(PathBuf::from(path));
            }
            "--jobs" | "-j" => {
                let value = iter
                    .next()
                    .ok_or_else(|| anyhow!("{arg} requires a positive integer"))?;
                jobs = value
                    .parse::<usize>()
                    .with_context(|| format!("invalid {arg} value `{value}`"))?;
                if jobs == 0 {
                    bail!("{arg} requires a positive integer");
                }
            }
            "--dry-run" => {
                dry_run = true;
            }
            value if value.starts_with('-') => bail!("unknown option `{value}`"),
            value => bail!("matrix does not accept scenario argument `{value}`"),
        }
    }

    if jobs > 1 && secondary_serial_port.is_none() {
        bail!("matrix --jobs > 1 requires --secondary-serial-port PATH");
    }
    if jobs > 1 && secondary_serial_port.as_ref() == Some(&serial_port) {
        bail!("--secondary-serial-port must differ from --serial-port");
    }

    Ok(MatrixOptions {
        root,
        serial_port,
        companion_port,
        secondary_serial_port,
        jobs,
        dry_run,
    })
}

fn ensure_no_extra_args(args: &[String], command: &str) -> Result<()> {
    if args.is_empty() {
        Ok(())
    } else {
        bail!("wifi-hw {command} does not accept arguments")
    }
}

fn print_help() {
    println!("usage: wifi-hw <list|doctor|run|matrix|companion>");
    println!("  list [--format text|json]");
    println!("  doctor");
    println!("  run <scenario> [--root PATH] [--serial-port PATH] [--companion-port PATH]");
    println!("  matrix [--dry-run] [--root PATH] [--serial-port PATH] [--companion-port PATH] [--jobs N --secondary-serial-port PATH]");
    println!("  matrix --jobs uses at most two lanes; secondary lane is limited to scenarios marked matrix_lane=secondary-candidate");
    println!("  companion <doctor|setup|build|flash|command>");
    println!("  env {SKIP_UNCHANGED_FLASH_ENV}=1 skips unchanged ESP8266 images after a known harness flash");
}

fn list(format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Text => {
            for scenario in WIFI_SCENARIOS {
                println!(
                    "{:<28} {:<16} runtime={} matrix_lane={} resources={} example={} profile={} features={} description={}{}",
                    scenario.name,
                    scenario.state.label(),
                    scenario.runtime.label(),
                    scenario_matrix_lane_label(scenario),
                    scenario_resource_labels(scenario).join(","),
                    scenario.example.unwrap_or("-"),
                    scenario.profile,
                    scenario.features.join(","),
                    scenario.description,
                    scenario
                        .state
                        .rig_blocked_reason()
                        .map(|reason| format!(" reason={reason}"))
                        .unwrap_or_default(),
                );
            }
        }
        OutputFormat::Json => {
            println!("[");
            for (index, scenario) in WIFI_SCENARIOS.iter().enumerate() {
                let comma = if index + 1 == WIFI_SCENARIOS.len() {
                    ""
                } else {
                    ","
                };
                let resources = scenario_resource_labels(scenario);
                println!(
                    "  {{\"name\":\"{}\",\"state\":\"{}\",\"runtime\":\"{}\",\"matrix_lane\":\"{}\",\"resources\":[{}],\"example\":{},\"profile\":\"{}\",\"features\":[{}],\"description\":\"{}\",\"reason\":{}}}{}",
                    json_escape(scenario.name),
                    scenario.state.label(),
                    scenario.runtime.label(),
                    scenario_matrix_lane_label(scenario),
                    json_string_array(&resources),
                    scenario
                        .example
                        .map(json_string)
                        .unwrap_or_else(|| "null".to_owned()),
                    json_escape(scenario.profile),
                    scenario
                        .features
                        .iter()
                        .map(|feature| json_string(feature))
                        .collect::<Vec<_>>()
                        .join(","),
                    json_escape(scenario.description),
                    scenario
                        .state
                        .rig_blocked_reason()
                        .map(json_string)
                        .unwrap_or_else(|| "null".to_owned()),
                    comma
                );
            }
            println!("]");
        }
    }

    Ok(())
}

fn json_string(value: &str) -> String {
    format!("\"{}\"", json_escape(value))
}

fn json_string_array(values: &[&str]) -> String {
    values
        .iter()
        .map(|value| json_string(value))
        .collect::<Vec<_>>()
        .join(",")
}

fn json_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            ch if ch.is_control() => escaped.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => escaped.push(ch),
        }
    }
    escaped
}

fn doctor() -> Result<()> {
    let checks = [
        command_available("bash"),
        command_available("cargo"),
        command_available("esptool").or_else(|_| command_available("esptool.py")),
        command_available("python3"),
    ];

    let mut failures = Vec::new();
    for result in checks {
        if let Err(error) = result {
            failures.push(error.to_string());
        }
    }

    if failures.is_empty() {
        println!("wifi-hw doctor: ok");
        println!("default artifacts root: {}", artifacts_root(None).display());
        let serial = Path::new(DEFAULT_SERIAL_PORT);
        let companion = Path::new(DEFAULT_COMPANION_PORT);
        println!(
            "serial default: {DEFAULT_SERIAL_PORT} ({})",
            path_presence(serial)
        );
        println!(
            "companion default: {DEFAULT_COMPANION_PORT} ({})",
            path_presence(companion)
        );
        println!(
            "router default: {DEFAULT_ROUTER_USER}@{DEFAULT_ROUTER_HOST} hostapd_iface={DEFAULT_ROUTER_HOSTAPD_IFACE}"
        );
        println!(
            "flash skip unchanged: {} ({SKIP_UNCHANGED_FLASH_ENV})",
            flash_skip_status()
        );
        return Ok(());
    }

    bail!(
        "wifi-hw doctor failed:\n{}",
        failures
            .into_iter()
            .map(|line| format!("- {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

#[derive(Clone)]
struct MatrixResourceLocks {
    router: Arc<Mutex<()>>,
    companion: Arc<Mutex<()>>,
}

impl MatrixResourceLocks {
    fn new() -> Self {
        Self {
            router: Arc::new(Mutex::new(())),
            companion: Arc::new(Mutex::new(())),
        }
    }
}

fn matrix_dry_run(
    root: Option<&Path>,
    serial_port: &Path,
    companion_port: &Path,
    secondary_serial_port: Option<&Path>,
    jobs: usize,
) -> Result<()> {
    let root = artifacts_root(root);
    let implemented: Vec<_> = WIFI_SCENARIOS
        .iter()
        .filter(|scenario| scenario.state == ScenarioState::Implemented)
        .collect();
    let (primary, secondary) = partition_matrix_lanes(&implemented);

    println!("wifi-hw matrix dry-run");
    println!("root={}", root.display());
    println!("jobs={jobs}");
    println!("primary_serial={}", serial_port.display());
    println!("companion_port={}", companion_port.display());

    if jobs > 1 {
        let secondary_serial_port =
            secondary_serial_port.expect("parse_matrix_options requires secondary serial");
        println!("secondary_serial={}", secondary_serial_port.display());
        print_matrix_plan_lane("primary", &primary);
        print_matrix_plan_lane("secondary", &secondary);
    } else {
        println!(
            "secondary_serial=- (single lane; secondary-candidate scenarios run on primary serial)"
        );
        print_matrix_plan_lane("primary", &implemented);
    }

    if jobs > 2 {
        println!("note=matrix scheduler currently uses at most two hardware lanes");
    }

    Ok(())
}

fn run_matrix(
    root: Option<&Path>,
    serial_port: &Path,
    companion_port: &Path,
    secondary_serial_port: Option<&Path>,
    jobs: usize,
) -> Result<()> {
    let root = artifacts_root(root);
    let implemented: Vec<_> = WIFI_SCENARIOS
        .iter()
        .filter(|scenario| scenario.state == ScenarioState::Implemented)
        .collect();

    let mut failures = if jobs > 1 {
        let secondary_serial_port =
            secondary_serial_port.expect("parse_matrix_options requires secondary serial");
        run_matrix_parallel(
            &root,
            &implemented,
            serial_port,
            secondary_serial_port,
            companion_port,
        )
    } else {
        run_matrix_lane(
            "matrix",
            &implemented,
            &root,
            serial_port,
            companion_port,
            None,
        )
    };

    if jobs > 2 {
        eprintln!(
            "wifi-hw matrix: --jobs {jobs} requested; current scheduler uses at most 2 hardware lanes"
        );
    }

    for scenario in WIFI_SCENARIOS
        .iter()
        .filter(|scenario| matches!(scenario.state, ScenarioState::RigBlocked { .. }))
    {
        println!(
            "wifi-hw matrix: rig-blocked {}: {}",
            scenario.name,
            scenario
                .state
                .rig_blocked_reason()
                .expect("rig-blocked scenario reason")
        );
    }

    if failures.is_empty() {
        println!("wifi-hw matrix: pass");
        return Ok(());
    }

    failures.sort();
    bail!(
        "wifi-hw matrix failed:\n{}",
        failures
            .into_iter()
            .map(|line| format!("- {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

fn run_matrix_parallel(
    root: &Path,
    implemented: &[&'static Scenario],
    primary_serial_port: &Path,
    secondary_serial_port: &Path,
    companion_port: &Path,
) -> Vec<String> {
    let (primary, secondary) = partition_matrix_lanes(implemented);

    if secondary.is_empty() {
        eprintln!("wifi-hw matrix: no scenarios are currently eligible for the secondary lane");
        return run_matrix_lane(
            "matrix",
            &primary,
            root,
            primary_serial_port,
            companion_port,
            None,
        );
    }

    eprintln!(
        "wifi-hw matrix: running {} scenarios on primary {} and {} scenarios on secondary {}",
        primary.len(),
        primary_serial_port.display(),
        secondary.len(),
        secondary_serial_port.display()
    );

    let locks = MatrixResourceLocks::new();
    let secondary_locks = locks.clone();

    thread::scope(|scope| {
        let secondary_handle = scope.spawn(|| {
            run_matrix_lane(
                "matrix-secondary",
                &secondary,
                root,
                secondary_serial_port,
                companion_port,
                Some(&secondary_locks),
            )
        });
        let mut failures = run_matrix_lane(
            "matrix-primary",
            &primary,
            root,
            primary_serial_port,
            companion_port,
            Some(&locks),
        );
        match secondary_handle.join() {
            Ok(mut secondary_failures) => failures.append(&mut secondary_failures),
            Err(_) => failures.push("matrix-secondary: worker thread panicked".to_owned()),
        }
        failures
    })
}

fn partition_matrix_lanes<'a>(
    implemented: &[&'a Scenario],
) -> (Vec<&'a Scenario>, Vec<&'a Scenario>) {
    let mut primary = Vec::new();
    let mut secondary = Vec::new();
    for scenario in implemented {
        if matrix_secondary_lane_candidate(scenario) {
            secondary.push(*scenario);
        } else {
            primary.push(*scenario);
        }
    }
    (primary, secondary)
}

fn print_matrix_plan_lane(label: &str, scenarios: &[&Scenario]) {
    println!("{label}_scenario_count={}", scenarios.len());
    for scenario in scenarios {
        println!(
            "{label}: {} runtime={} matrix_lane={} resources={}",
            scenario.name,
            scenario.runtime.label(),
            scenario_matrix_lane_label(scenario),
            scenario_resource_labels(scenario).join(",")
        );
    }
}

fn run_matrix_lane(
    label: &str,
    scenarios: &[&'static Scenario],
    root: &Path,
    serial_port: &Path,
    companion_port: &Path,
    locks: Option<&MatrixResourceLocks>,
) -> Vec<String> {
    let mut failures = Vec::new();

    for scenario in scenarios {
        if let Err(error) = run_matrix_scenario_with_resources(
            label,
            scenario,
            root,
            serial_port,
            companion_port,
            locks,
        ) {
            failures.push(format!("{}: {error:#}", scenario.name));
        }
    }

    failures
}

fn run_matrix_scenario_with_resources(
    label: &str,
    scenario: &Scenario,
    root: &Path,
    serial_port: &Path,
    companion_port: &Path,
    locks: Option<&MatrixResourceLocks>,
) -> Result<()> {
    let _router_guard = if let Some(locks) = locks.filter(|_| scenario_uses_router(scenario.name)) {
        Some(
            locks
                .router
                .lock()
                .map_err(|_| anyhow!("matrix router lock poisoned"))?,
        )
    } else {
        None
    };
    let _companion_guard =
        if let Some(locks) = locks.filter(|_| scenario_uses_companion_resource(scenario.name)) {
            Some(
                locks
                    .companion
                    .lock()
                    .map_err(|_| anyhow!("matrix companion lock poisoned"))?,
            )
        } else {
            None
        };

    run_scenario_with_retries(
        label,
        scenario,
        root,
        serial_port,
        companion_port,
        DEFAULT_MATRIX_ATTEMPTS,
    )
}

fn matrix_secondary_lane_candidate(scenario: &Scenario) -> bool {
    matches!(scenario.name, "eap_sta_assoc" | "eap_wrong_password")
}

fn scenario_uses_router(name: &str) -> bool {
    matches!(
        name,
        "eap_sta_assoc" | "eap_wrong_password" | "wps" | "wps_pin" | "smartconfig"
    )
}

fn scenario_uses_companion_resource(name: &str) -> bool {
    matches!(
        name,
        "sta_connect"
            | "sta_scan_connect"
            | "sta_wrong_password"
            | "sta_wpa2_connect"
            | "sta_stale_channel_hint"
            | "sta_hidden_ssid"
            | "sta_scan"
            | "sta_dhcp"
            | "sta_static_ip"
            | "tcp_server"
            | "tcp_client"
            | "smartconfig"
    ) || scenario_uses_companion_rig(name)
}

fn run_scenario(
    name: &str,
    root: Option<&Path>,
    serial_port: &Path,
    companion_port: &Path,
) -> Result<()> {
    let scenario = scenario_by_name(name).ok_or_else(|| anyhow!("unknown scenario: {name}"))?;
    let root = artifacts_root(root);
    let attempts = focused_run_attempt_limit(scenario);
    if attempts > 1 {
        run_scenario_with_retries(
            "run",
            scenario,
            &root,
            serial_port,
            companion_port,
            attempts,
        )
    } else {
        run_scenario_with_root(scenario, &root, serial_port, companion_port)
    }
}

fn focused_run_attempt_limit(scenario: &Scenario) -> usize {
    match scenario.name {
        "wps" | "wps_pin" => DEFAULT_MATRIX_ATTEMPTS,
        _ => 1,
    }
}

fn allow_rig_blocked_scenarios() -> bool {
    env::var("ESP8266_HAL_WIFI_HW_ALLOW_RIG_BLOCKED")
        .map(|value| matches!(value.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

fn run_scenario_with_retries(
    label: &str,
    scenario: &Scenario,
    root: &Path,
    serial_port: &Path,
    companion_port: &Path,
    attempts: usize,
) -> Result<()> {
    let mut failures = Vec::new();

    for attempt in 1..=attempts {
        match run_scenario_with_root(scenario, root, serial_port, companion_port) {
            Ok(()) => return Ok(()),
            Err(error) => {
                failures.push(format!("attempt {attempt}: {error:#}"));
                if attempt < attempts {
                    eprintln!(
                        "wifi-hw {label}: {} failed attempt {attempt}/{attempts}; retrying after {}s",
                        scenario.name,
                        MATRIX_RETRY_BACKOFF.as_secs()
                    );
                    thread::sleep(MATRIX_RETRY_BACKOFF);
                }
            }
        }
    }

    bail!(
        "failed after {} attempts:\n{}",
        failures.len(),
        failures
            .into_iter()
            .map(|line| format!("  - {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

fn run_scenario_with_root(
    scenario: &Scenario,
    root: &Path,
    serial_port: &Path,
    companion_port: &Path,
) -> Result<()> {
    if scenario.state == ScenarioState::PendingExample {
        bail!(
            "scenario `{}` is registered but not implemented yet (expected example `{}`)",
            scenario.name,
            scenario.example.unwrap_or("-")
        );
    }
    if !allow_rig_blocked_scenarios() {
        if let ScenarioState::RigBlocked { reason } = scenario.state {
            bail!(
                "scenario `{}` is blocked by the current hardware rig: {reason}; see research.md",
                scenario.name,
            );
        }
    }

    if matches!(scenario.name, "wps" | "wps_pin") {
        return run_wps_router_scenario(scenario, root, serial_port);
    }

    if scenario.name == "smartconfig" {
        return run_smartconfig_router_scenario(scenario, root, serial_port, companion_port);
    }

    if scenario_uses_companion_rig(scenario.name) {
        return run_companion_scenario_with_root(scenario, root, serial_port, companion_port);
    }

    let example = scenario
        .example
        .ok_or_else(|| anyhow!("scenario `{}` has no example", scenario.name))?;
    let run_dir = prepare_run_dir(root, scenario.name)?;

    write_manifest(&run_dir, scenario, serial_port)?;
    let mut companion_ap = start_companion_ap_for_scenario(&run_dir, scenario, companion_port)?;

    let result = {
        let build_env = companion_ap
            .as_ref()
            .map(|session| session.build_env.clone())
            .unwrap_or_default();

        (|| -> Result<()> {
            build_example(&run_dir, example, scenario, &build_env)?;
            let images = elf2image(&run_dir, example, scenario)?;
            flash_images(&run_dir, &images, serial_port)?;

            match scenario.runtime {
                RuntimePlan::UartWatch {
                    timeout,
                    success_markers,
                    failure_markers,
                } => watch_uart(
                    &run_dir,
                    serial_port,
                    timeout,
                    success_markers,
                    failure_markers,
                ),
                RuntimePlan::TcpEcho {
                    timeout,
                    port,
                    dhcp_regex,
                    listen_marker,
                    failure_markers,
                    payload,
                } if scenario.name == "tcp_server" && companion_ap.is_some() => {
                    let session = companion_ap.as_mut().expect("checked companion AP");
                    watch_companion_tcp_echo(
                        &run_dir,
                        serial_port,
                        session,
                        timeout,
                        port,
                        dhcp_regex,
                        listen_marker,
                        failure_markers,
                        payload,
                    )
                }
                RuntimePlan::TcpEcho {
                    timeout,
                    port,
                    dhcp_regex,
                    listen_marker,
                    failure_markers,
                    payload,
                } => watch_tcp_echo(
                    &run_dir,
                    serial_port,
                    timeout,
                    port,
                    dhcp_regex,
                    listen_marker,
                    failure_markers,
                    payload,
                ),
                RuntimePlan::TcpClient {
                    timeout,
                    port,
                    success_markers,
                    failure_markers,
                    response,
                } if companion_ap.is_some() => {
                    let session = companion_ap.as_mut().expect("checked companion AP");
                    watch_companion_tcp_client(
                        &run_dir,
                        serial_port,
                        session,
                        timeout,
                        port,
                        success_markers,
                        failure_markers,
                        response,
                    )
                }
                RuntimePlan::TcpClient {
                    timeout,
                    port,
                    success_markers,
                    failure_markers,
                    response,
                } => watch_tcp_client(
                    &run_dir,
                    serial_port,
                    timeout,
                    port,
                    success_markers,
                    failure_markers,
                    response,
                ),
                RuntimePlan::CompanionRig => bail!(
                    "scenario `{}` requires the companion-rig runner but was not dispatched to it",
                    scenario.name
                ),
                RuntimePlan::Pending => bail!("scenario `{}` has no runtime plan", scenario.name),
            }
        })()
    };

    if let Some(session) = companion_ap {
        stop_companion_ap(session);
    }

    result
}

fn scenario_uses_companion_rig(name: &str) -> bool {
    matches!(
        name,
        "ap_station_list"
            | "ap_dhcp"
            | "ap_sta"
            | "esp_now_unicast"
            | "esp_now_encrypted"
            | "promisc_capture"
            | "promisc_filters"
            | "promisc_control_filters"
    ) || name.ends_with("_companion")
}

fn is_promisc_rig_scenario(name: &str) -> bool {
    matches!(
        name,
        "promisc_capture"
            | "promisc_filters"
            | "promisc_control_filters"
            | "promisc_data_companion"
    )
}

fn run_wps_router_scenario(scenario: &Scenario, root: &Path, serial_port: &Path) -> Result<()> {
    let example = scenario
        .example
        .ok_or_else(|| anyhow!("scenario `{}` has no example", scenario.name))?;
    let run_dir = prepare_run_dir(root, scenario.name)?;
    write_manifest(&run_dir, scenario, serial_port)?;

    let fixture = RouterWpsFixture::new(scenario.name)?;
    let mut build_env = Vec::new();
    if let Some(env) = fixture.mode.build_env() {
        build_env.push(env);
    }
    build_env.push((
        "ESP8266_HAL_WPS_MAX_ATTEMPTS",
        WPS_HARNESS_MAX_ATTEMPTS.to_owned(),
    ));

    build_example(&run_dir, example, scenario, &build_env)?;
    let images = elf2image(&run_dir, example, scenario)?;
    flash_images(&run_dir, &images, serial_port)?;

    append_router_manifest(&run_dir, &fixture)?;

    if let Err(error) = configure_router_wps_fixture(&run_dir, &fixture) {
        let _ = restore_router_wps_fixture(&run_dir, &fixture);
        return Err(error);
    }

    let result = match scenario.runtime {
        RuntimePlan::UartWatch {
            timeout,
            success_markers,
            failure_markers,
        } => watch_uart(
            &run_dir,
            serial_port,
            timeout,
            success_markers,
            failure_markers,
        ),
        _ => bail!("WPS router scenario requires a UART watch runtime plan"),
    };

    let status_result = collect_router_wps_status(&run_dir, &fixture, "router-after");
    let restore_result = restore_router_wps_fixture(&run_dir, &fixture);

    match (result, status_result, restore_result) {
        (Ok(()), Ok(()), Ok(())) => Ok(()),
        (Err(error), _, Ok(())) => Err(error),
        (Ok(()), Err(error), Ok(())) => Err(error),
        (Ok(()), Ok(()), Err(error)) => Err(error),
        (Err(error), _, Err(restore_error)) => {
            Err(error.context(format!("router restore also failed: {restore_error:#}")))
        }
        (Ok(()), Err(error), Err(restore_error)) => {
            Err(error.context(format!("router restore also failed: {restore_error:#}")))
        }
    }
}

impl RouterRig {
    fn from_env() -> Result<Self> {
        let host =
            env::var("ESP8266_HAL_ROUTER_HOST").unwrap_or_else(|_| DEFAULT_ROUTER_HOST.to_owned());
        let user =
            env::var("ESP8266_HAL_ROUTER_USER").unwrap_or_else(|_| DEFAULT_ROUTER_USER.to_owned());
        let known_hosts_file = env::var_os("ESP8266_HAL_ROUTER_KNOWN_HOSTS")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_ROUTER_KNOWN_HOSTS));
        let hostapd_iface = env::var("ESP8266_HAL_ROUTER_HOSTAPD_IFACE")
            .unwrap_or_else(|_| DEFAULT_ROUTER_HOSTAPD_IFACE.to_owned());

        let identity_file = env::var_os("ESP8266_HAL_ROUTER_SSH_KEY")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                repo_root()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join(DEFAULT_ROUTER_IDENTITY)
            });
        let identity_file = (!identity_file.as_os_str().is_empty()).then_some(identity_file);

        Ok(Self {
            host,
            user,
            identity_file,
            known_hosts_file,
            hostapd_iface,
        })
    }

    fn target(&self) -> String {
        format!("{}@{}", self.user, self.host)
    }

    fn ssh_command(&self) -> Command {
        let mut command = Command::new("ssh");
        command
            .arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg("StrictHostKeyChecking=no")
            .arg("-o")
            .arg(format!(
                "UserKnownHostsFile={}",
                self.known_hosts_file.display()
            ));
        if let Some(identity_file) = &self.identity_file {
            command
                .arg("-i")
                .arg(identity_file)
                .arg("-o")
                .arg("IdentitiesOnly=yes");
        }
        command.arg(self.target());
        command
    }
}

impl RouterWpsFixture {
    fn new(scenario_name: &str) -> Result<Self> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before epoch")
            .as_secs();
        let mode = if scenario_name == "wps_pin" {
            WpsRouterMode::Pin
        } else {
            WpsRouterMode::from_env()?
        };
        Ok(Self {
            router: RouterRig::from_env()?,
            backup_path: format!(
                "/tmp/esp8266-hal-wireless-wps-{stamp}-{}",
                std::process::id()
            ),
            dut_mac: env::var("ESP8266_HAL_WPS_DUT_MAC")
                .unwrap_or_else(|_| DEFAULT_WPS_DUT_MAC.to_owned()),
            dut_uuid: env::var("ESP8266_HAL_WPS_DUT_UUID")
                .unwrap_or_else(|_| DEFAULT_WPS_DUT_UUID.to_owned()),
            mode,
        })
    }
}

impl RouterSmartConfigFixture {
    fn new() -> Result<Self> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before epoch")
            .as_secs();
        let channel = env::var("ESP8266_HAL_SMARTCONFIG_CHANNEL")
            .ok()
            .map(|value| {
                value
                    .parse::<u8>()
                    .with_context(|| format!("invalid ESP8266_HAL_SMARTCONFIG_CHANNEL `{value}`"))
            })
            .transpose()?
            .unwrap_or(DEFAULT_SMARTCONFIG_CHANNEL);
        if !(1..=14).contains(&channel) {
            bail!("ESP8266_HAL_SMARTCONFIG_CHANNEL must be in 1..=14, got {channel}");
        }
        Ok(Self {
            router: RouterRig::from_env()?,
            backup_path: format!(
                "/tmp/esp8266-hal-wireless-smartconfig-{stamp}-{}",
                std::process::id()
            ),
            ssid: env::var("ESP8266_HAL_SMARTCONFIG_SSID")
                .unwrap_or_else(|_| SMARTCONFIG_ROUTER_SSID.to_owned()),
            passphrase: env::var("ESP8266_HAL_SMARTCONFIG_PASSWORD")
                .unwrap_or_else(|_| SMARTCONFIG_ROUTER_PASSPHRASE.to_owned()),
            channel,
        })
    }
}

fn append_router_manifest(run_dir: &Path, fixture: &RouterWpsFixture) -> Result<()> {
    let mut file = OpenOptions::new()
        .append(true)
        .open(run_dir.join("manifest.txt"))
        .with_context(|| format!("failed to open {}", run_dir.join("manifest.txt").display()))?;
    writeln!(file, "router_host={}", fixture.router.host)?;
    writeln!(file, "router_user={}", fixture.router.user)?;
    writeln!(
        file,
        "router_hostapd_iface={}",
        fixture.router.hostapd_iface
    )?;
    writeln!(file, "router_wireless_backup={}", fixture.backup_path)?;
    writeln!(file, "wps_dut_mac={}", fixture.dut_mac)?;
    writeln!(file, "wps_dut_uuid={}", fixture.dut_uuid)?;
    writeln!(file, "wps_mode={}", fixture.mode.as_str())?;
    Ok(())
}

fn append_smartconfig_router_manifest(
    run_dir: &Path,
    fixture: &RouterSmartConfigFixture,
) -> Result<()> {
    let mut file = OpenOptions::new()
        .append(true)
        .open(run_dir.join("manifest.txt"))
        .with_context(|| format!("failed to open {}", run_dir.join("manifest.txt").display()))?;
    writeln!(file, "router_host={}", fixture.router.host)?;
    writeln!(file, "router_user={}", fixture.router.user)?;
    writeln!(
        file,
        "router_hostapd_iface={}",
        fixture.router.hostapd_iface
    )?;
    writeln!(file, "router_wireless_backup={}", fixture.backup_path)?;
    writeln!(file, "smartconfig_ssid={}", fixture.ssid)?;
    writeln!(file, "smartconfig_channel={}", fixture.channel)?;
    Ok(())
}

fn append_manifest_line(run_dir: &Path, key: &str, value: &str) -> Result<()> {
    let mut file = OpenOptions::new()
        .append(true)
        .open(run_dir.join("manifest.txt"))
        .with_context(|| format!("failed to open {}", run_dir.join("manifest.txt").display()))?;
    writeln!(file, "{key}={value}")?;
    Ok(())
}

fn router_run(
    run_dir: &Path,
    fixture: &RouterWpsFixture,
    label: &str,
    script: &str,
) -> Result<String> {
    router_run_with_rig(run_dir, &fixture.router, label, script)
}

fn smartconfig_router_run(
    run_dir: &Path,
    fixture: &RouterSmartConfigFixture,
    label: &str,
    script: &str,
) -> Result<String> {
    router_run_with_rig(run_dir, &fixture.router, label, script)
}

fn router_run_with_rig(
    run_dir: &Path,
    router: &RouterRig,
    label: &str,
    script: &str,
) -> Result<String> {
    let log_path = run_dir.join(format!("{label}.log"));
    let mut log = File::create(&log_path)
        .with_context(|| format!("failed to create {}", log_path.display()))?;
    writeln!(log, "$ ssh {} {label}", router.target())?;
    writeln!(log, "{script}")?;

    let mut command = router.ssh_command();
    command
        .arg(script)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = command
        .output()
        .with_context(|| format!("failed to run router command `{label}`"))?;
    log.write_all(&output.stdout)?;
    log.write_all(&output.stderr)?;

    if !output.status.success() {
        bail!(
            "router command `{label}` failed; see {}",
            log_path.display()
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn configure_router_wps_fixture(run_dir: &Path, fixture: &RouterWpsFixture) -> Result<()> {
    let iface = shell_quote(&fixture.router.hostapd_iface);
    let backup = shell_quote(&fixture.backup_path);
    let ssid = shell_quote(WPS_ROUTER_SSID);
    let passphrase = shell_quote(WPS_ROUTER_PASSPHRASE);
    let mode = shell_quote(fixture.mode.as_str());
    let pin = shell_quote(WPS_PIN);
    let dut_mac = shell_quote(&fixture.dut_mac);
    let dut_uuid = shell_quote(&fixture.dut_uuid);
    let script = format!(
        r#"set -e
iface={iface}
backup={backup}
expected_ssid={ssid}
wps_mode={mode}
wps_pin={pin}
dut_mac={dut_mac}
dut_uuid={dut_uuid}
cp /etc/config/wireless "$backup"
uci set wireless.radio0.disabled=0
uci set wireless.radio0.channel=6
uci set wireless.radio0.htmode=NOHT
uci set wireless.default_radio0.disabled=0
uci set wireless.default_radio0.mode=ap
uci set wireless.default_radio0.network=lan
uci set wireless.default_radio0.ssid={ssid}
uci set wireless.default_radio0.encryption=psk2
uci set wireless.default_radio0.key={passphrase}
uci set wireless.default_radio0.hidden=0
uci set wireless.default_radio0.ieee80211w=0
uci set wireless.default_radio0.uapsd=0
uci set wireless.default_radio0.wmm=0
uci set wireless.default_radio0.wps_pushbutton=1
uci set wireless.default_radio0.wps_label=1
uci -q delete wireless.default_radio0.auth_server || true
uci -q delete wireless.default_radio0.auth_port || true
uci -q delete wireless.default_radio0.auth_secret || true
uci commit wireless
wifi down radio0 || true
sleep 2
wifi up radio0
ready=0
tries=0
while [ "$tries" -lt 120 ]; do
    tries=$((tries + 1))
    status="$(ubus -S call "hostapd.$iface" get_status 2>/dev/null || true)"
    if printf '%s\n' "$status" | grep -Fq '"status":"ENABLED"' &&
       printf '%s\n' "$status" | grep -Fq "\"ssid\":\"$expected_ssid\""; then
        ready=1
        break
    fi
    sleep 1
done
ubus -S call "hostapd.$iface" get_status 2>/dev/null || true
test "$ready" = "1"
sleep 1
config="$(hostapd_cli -p /var/run/hostapd -i "$iface" get_config 2>&1)"
printf '%s\n' "$config"
printf '%s\n' "$config" | grep -qx "ssid=$expected_ssid"
printf '%s\n' "$config" | grep -qx "wps_state=configured"
hostapd_cli -p /var/run/hostapd -i "$iface" wps_cancel >/dev/null 2>&1 || true
case "$wps_mode" in
    pbc)
        wps_result="$(hostapd_cli -p /var/run/hostapd -i "$iface" wps_pbc)"
        ;;
    pin)
        wps_result="$(hostapd_cli -p /var/run/hostapd -i "$iface" wps_pin "$dut_uuid" "$wps_pin" 300 "$dut_mac" 2>&1 || true)"
        printf 'wps_pin_uuid=%s\n' "$wps_result"
        if [ "$wps_result" != "OK" ]; then
            wps_result="$(hostapd_cli -p /var/run/hostapd -i "$iface" wps_pin any "$wps_pin" 300 "$dut_mac" 2>&1 || true)"
            printf 'wps_pin_any=%s\n' "$wps_result"
        fi
        ;;
    *)
        echo "unexpected wps mode: $wps_mode" >&2
        exit 1
        ;;
esac
printf '%s\n' "$wps_result"
test "$wps_result" = "OK"
hostapd_cli -p /var/run/hostapd -i "$iface" update_beacon >/dev/null 2>&1 || true
hostapd_cli -p /var/run/hostapd -i "$iface" wps_get_status
sleep 1
echo "backup_path=$backup"
"#
    );
    router_run(run_dir, fixture, "router-wps-setup", &script).map(|_| ())
}

fn collect_router_wps_status(
    run_dir: &Path,
    fixture: &RouterWpsFixture,
    label: &str,
) -> Result<()> {
    let iface = shell_quote(&fixture.router.hostapd_iface);
    let script = format!(
        r#"set -e
iface={iface}
echo "---wps-status"
hostapd_cli -p /var/run/hostapd -i "$iface" wps_get_status 2>&1 || true
echo "---hostapd-status"
ubus -S call "hostapd.$iface" get_status 2>/dev/null || true
echo "---recent-log"
logread | tail -n 180 | grep -i "wps\|eap\|48:3f:da:68:e3:04\|ap-sta\|wpa" || true
"#
    );
    router_run(run_dir, fixture, label, &script).map(|_| ())
}

fn restore_router_wps_fixture(run_dir: &Path, fixture: &RouterWpsFixture) -> Result<()> {
    let iface = shell_quote(&fixture.router.hostapd_iface);
    let backup = shell_quote(&fixture.backup_path);
    let script = format!(
        r#"set -e
iface={iface}
backup={backup}
if [ -f "$backup" ]; then
    cp "$backup" /etc/config/wireless
    rm -f "$backup"
fi
expected_ssid="$(uci -q get wireless.default_radio0.ssid || true)"
wifi down radio0 || true
sleep 3
wifi up radio0
sleep 70
uci show wireless
[ -n "$expected_ssid" ]
uci -q get wireless.default_radio0.ssid | grep -qx "$expected_ssid"
ps w | grep hostapd | grep -v grep || true
logread | tail -n 120 | grep -i "radio0\|phy0\|hostapd\|wps\|eap\|radius" || true
"#
    );
    router_run(run_dir, fixture, "router-restore", &script).map(|_| ())
}

fn run_smartconfig_router_scenario(
    scenario: &Scenario,
    root: &Path,
    serial_port: &Path,
    companion_port: &Path,
) -> Result<()> {
    let example = scenario
        .example
        .ok_or_else(|| anyhow!("scenario `{}` has no example", scenario.name))?;
    let run_dir = prepare_run_dir(root, scenario.name)?;
    write_manifest(&run_dir, scenario, serial_port)?;
    let companion = probe_companion_rig(companion_port)?;
    append_companion_manifest(&run_dir, &companion)?;
    let sender = SmartConfigSenderKind::from_env()?;
    let build_env = sender.build_env();

    build_example(&run_dir, example, scenario, &build_env)?;
    let images = elf2image(&run_dir, example, scenario)?;
    flash_images(&run_dir, &images, serial_port)?;

    let fixture = RouterSmartConfigFixture::new()?;
    append_smartconfig_router_manifest(&run_dir, &fixture)?;
    append_manifest_line(&run_dir, "smartconfig_sender", sender.as_str())?;
    let static_ack = build_env
        .iter()
        .any(|(key, _)| *key == "ESP8266_HAL_SMARTCONFIG_STATIC_ACK")
        || env::var_os("ESP8266_HAL_SMARTCONFIG_STATIC_ACK").is_some();
    append_manifest_line(
        &run_dir,
        "smartconfig_static_ack",
        if static_ack { "1" } else { "0" },
    )?;
    let start_after_scan = sender.default_start_after_scan()
        || env::var_os("ESP8266_HAL_SMARTCONFIG_START_AFTER_SCAN").is_some();
    append_manifest_line(
        &run_dir,
        "smartconfig_start_after_scan",
        if start_after_scan { "1" } else { "0" },
    )?;
    ensure_companion_command_token("smartconfig ssid", &fixture.ssid)?;
    ensure_companion_command_token("smartconfig passphrase", &fixture.passphrase)?;

    let bssid = if sender.needs_router_ap() {
        match configure_router_smartconfig_fixture(&run_dir, &fixture) {
            Ok(bssid) => bssid,
            Err(error) => {
                let _ = restore_router_smartconfig_fixture(&run_dir, &fixture);
                return Err(error);
            }
        }
    } else {
        [0; 6]
    };
    let mut companion_log = open_companion_log(&run_dir)?;
    let mut companion_commands = esp32_companion::CommandSession::open(companion_port)?;
    ensure_companion_session_ready(&mut companion_log, &mut companion_commands)?;
    if sender == SmartConfigSenderKind::CompanionAp {
        prestart_companion_smartconfig_ap(&fixture, &mut companion_log, &mut companion_commands)?;
        append_manifest_line(&run_dir, "smartconfig_companion_ap_prestarted", "1")?;
    }

    let result = match scenario.runtime {
        RuntimePlan::UartWatch {
            timeout,
            success_markers,
            failure_markers,
        } => watch_smartconfig_uart(
            &run_dir,
            serial_port,
            timeout,
            success_markers,
            failure_markers,
            &fixture,
            &mut companion_log,
            &mut companion_commands,
            sender,
            bssid,
            start_after_scan,
        ),
        _ => bail!("SmartConfig router scenario requires a UART watch runtime plan"),
    };

    let status_result = if sender.needs_router_ap() {
        collect_router_smartconfig_status(&run_dir, &fixture, "router-after")
    } else {
        Ok(())
    };
    let restore_result = if sender.needs_router_ap() {
        restore_router_smartconfig_fixture(&run_dir, &fixture)
    } else {
        Ok(())
    };

    match (result, status_result, restore_result) {
        (Ok(()), Ok(()), Ok(())) => Ok(()),
        (Err(error), _, Ok(())) => Err(error),
        (Ok(()), Err(error), Ok(())) => Err(error),
        (Ok(()), Ok(()), Err(error)) => Err(error),
        (Err(error), _, Err(restore_error)) => {
            Err(error.context(format!("router restore also failed: {restore_error:#}")))
        }
        (Ok(()), Err(error), Err(restore_error)) => {
            Err(error.context(format!("router restore also failed: {restore_error:#}")))
        }
    }
}

fn configure_router_smartconfig_fixture(
    run_dir: &Path,
    fixture: &RouterSmartConfigFixture,
) -> Result<[u8; 6]> {
    let iface = shell_quote(&fixture.router.hostapd_iface);
    let backup = shell_quote(&fixture.backup_path);
    let ssid = shell_quote(&fixture.ssid);
    let passphrase = shell_quote(&fixture.passphrase);
    let channel = fixture.channel;
    let script = format!(
        r#"set -e
iface={iface}
backup={backup}
expected_ssid={ssid}
smart_pass={passphrase}
expected_channel={channel}
cp /etc/config/wireless "$backup"
uci set wireless.radio0.disabled=0
uci set wireless.radio0.channel={channel}
uci set wireless.radio0.htmode=NOHT
uci set wireless.default_radio0.disabled=0
uci set wireless.default_radio0.mode=ap
uci set wireless.default_radio0.network=lan
uci set wireless.default_radio0.ssid={ssid}
if [ -n "$smart_pass" ] && [ "$smart_pass" != "open" ]; then
    uci set wireless.default_radio0.encryption=psk2
    uci set wireless.default_radio0.key="$smart_pass"
else
    uci set wireless.default_radio0.encryption=none
    uci -q delete wireless.default_radio0.key || true
fi
uci set wireless.default_radio0.hidden=0
uci set wireless.default_radio0.ieee80211w=0
uci set wireless.default_radio0.uapsd=0
uci set wireless.default_radio0.wmm=0
uci -q delete wireless.default_radio0.wps_pushbutton || true
uci -q delete wireless.default_radio0.auth_server || true
uci -q delete wireless.default_radio0.auth_port || true
uci -q delete wireless.default_radio0.auth_secret || true
uci commit wireless
wifi down radio0 || true
sleep 2
wifi up radio0
ready=0
tries=0
while [ "$tries" -lt 120 ]; do
    tries=$((tries + 1))
    status="$(ubus -S call "hostapd.$iface" get_status 2>/dev/null || true)"
    if printf '%s\n' "$status" | grep -Fq '"status":"ENABLED"' &&
       printf '%s\n' "$status" | grep -Fq "\"ssid\":\"$expected_ssid\"" &&
       printf '%s\n' "$status" | grep -Fq "\"channel\":$expected_channel"; then
        ready=1
        break
    fi
    sleep 1
done
echo "---hostapd-status"
status="$(ubus -S call "hostapd.$iface" get_status 2>/dev/null || true)"
printf '%s\n' "$status"
printf '%s\n' "$status" | grep -Fq '"status":"ENABLED"'
printf '%s\n' "$status" | grep -Fq "\"ssid\":\"$expected_ssid\""
printf '%s\n' "$status" | grep -Fq "\"channel\":$expected_channel"
echo "---hostapd-cli-status"
hostapd_status="$(hostapd_cli -p /var/run/hostapd -i "$iface" status 2>/dev/null || true)"
printf '%s\n' "$hostapd_status"
test "$ready" = "1"
if [ -n "$hostapd_status" ]; then
    printf '%s\n' "$hostapd_status" | grep -qx "state=ENABLED"
fi
sleep 1
echo "---hostapd-config"
config="$(hostapd_cli -p /var/run/hostapd -i "$iface" get_config 2>&1)"
printf '%s\n' "$config"
printf '%s\n' "$config" | grep -qx "ssid=$expected_ssid"
if [ -n "$smart_pass" ] && [ "$smart_pass" != "open" ]; then
    printf '%s\n' "$config" | grep -qx "key_mgmt=WPA-PSK"
else
    if printf '%s\n' "$config" | grep -qx "key_mgmt=WPA-PSK"; then
        exit 1
    fi
fi
echo "backup_path=$backup"
"#
    );
    let output = smartconfig_router_run(run_dir, fixture, "router-smartconfig-setup", &script)?;
    parse_bssid_from_router_output(&output)
}

fn collect_router_smartconfig_status(
    run_dir: &Path,
    fixture: &RouterSmartConfigFixture,
    label: &str,
) -> Result<()> {
    let iface = shell_quote(&fixture.router.hostapd_iface);
    let dut_mac = shell_quote(DEFAULT_WPS_DUT_MAC);
    let script = format!(
        r#"set -e
iface={iface}
dut_mac={dut_mac}
echo "---hostapd-status"
ubus -S call "hostapd.$iface" get_status 2>/dev/null || true
echo "---hostapd-config"
hostapd_cli -p /var/run/hostapd -i "$iface" get_config 2>/dev/null || true
echo "---sta"
hostapd_cli -p /var/run/hostapd -i "$iface" sta "$dut_mac" 2>/dev/null || true
echo "---recent-log"
logread | tail -n 180 | grep -i "smart\|48:3f:da:68:e3:04\|ap-sta\|wpa\|eap" || true
"#
    );
    smartconfig_router_run(run_dir, fixture, label, &script).map(|_| ())
}

fn restore_router_smartconfig_fixture(
    run_dir: &Path,
    fixture: &RouterSmartConfigFixture,
) -> Result<()> {
    let backup = shell_quote(&fixture.backup_path);
    let script = format!(
        r#"set -e
backup={backup}
if [ -f "$backup" ]; then
    cp "$backup" /etc/config/wireless
    rm -f "$backup"
fi
expected_ssid="$(uci -q get wireless.default_radio0.ssid || true)"
wifi down radio0 || true
sleep 3
wifi up radio0
sleep 70
uci show wireless
[ -n "$expected_ssid" ]
uci -q get wireless.default_radio0.ssid | grep -qx "$expected_ssid"
ps w | grep hostapd | grep -v grep || true
logread | tail -n 120 | grep -i "radio0\|phy0\|hostapd\|wps\|eap\|radius\|smart" || true
"#
    );
    smartconfig_router_run(run_dir, fixture, "router-restore", &script).map(|_| ())
}

fn watch_smartconfig_uart(
    run_dir: &Path,
    serial_port: &Path,
    timeout: Duration,
    success_markers: &[&str],
    failure_markers: &[&str],
    fixture: &RouterSmartConfigFixture,
    companion_log: &mut File,
    companion_commands: &mut esp32_companion::CommandSession,
    sender: SmartConfigSenderKind,
    bssid: [u8; 6],
    start_after_scan: bool,
) -> Result<()> {
    let mut port = open_serial_port(serial_port)?;
    pulse_reset(port.as_mut())?;
    let uart_path = run_dir.join("uart.log");
    let mut uart_log = File::create(&uart_path)
        .with_context(|| format!("failed to create {}", uart_path.display()))?;
    let start = Instant::now();
    let mut buffer = Vec::with_capacity(32 * 1024);
    let mut provision_started = false;
    let bssid_text = format_colon_mac(bssid);

    thread::scope(|scope| -> Result<()> {
        let mut companion_log = Some(companion_log);
        let mut companion_commands = Some(companion_commands);
        let mut sender_handle = None;

        while start.elapsed() < timeout {
            if sender_handle
                .as_ref()
                .is_some_and(std::thread::ScopedJoinHandle::is_finished)
            {
                let handle = sender_handle.take().expect("sender handle");
                handle
                    .join()
                    .map_err(|_| anyhow!("SmartConfig sender thread panicked"))??;
            }

            let mut chunk = [0_u8; 4096];
            match port.read(&mut chunk) {
                Ok(read) if read > 0 => {
                    uart_log.write_all(&chunk[..read])?;
                    buffer.extend_from_slice(&chunk[..read]);
                    if buffer.len() > 32 * 1024 {
                        let excess = buffer.len() - 32 * 1024;
                        buffer.drain(0..excess);
                    }

                    let text = String::from_utf8_lossy(&buffer);
                    if let Some(marker) = failure_markers
                        .iter()
                        .find(|marker| text.contains(**marker))
                    {
                        bail!(
                            "uart reported failure marker `{marker}`; see {}",
                            uart_path.display()
                        );
                    }
                    let trigger = if start_after_scan {
                        "smartconfig scan done"
                    } else {
                        "smartconfig ready"
                    };
                    if !provision_started && text.contains(trigger) {
                        let run_dir = run_dir.to_path_buf();
                        let bssid_text = bssid_text.clone();
                        let companion_log = companion_log.take().expect("companion log available");
                        let companion_commands = companion_commands
                            .take()
                            .expect("companion command session available");
                        sender_handle = Some(scope.spawn(move || {
                            run_smartconfig_sender(
                                &run_dir,
                                fixture,
                                companion_log,
                                companion_commands,
                                sender,
                                &bssid_text,
                            )
                        }));
                        provision_started = true;
                    }
                    if success_markers.iter().any(|marker| text.contains(*marker)) {
                        if let Some(handle) = sender_handle.take() {
                            handle
                                .join()
                                .map_err(|_| anyhow!("SmartConfig sender thread panicked"))??;
                        }
                        return Ok(());
                    }
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
                Err(error) => return Err(error).context("failed while reading UART"),
            }
        }

        if let Some(handle) = sender_handle.take() {
            handle
                .join()
                .map_err(|_| anyhow!("SmartConfig sender thread panicked"))??;
        }

        bail!(
            "timed out waiting for SmartConfig router flow; see {}",
            uart_path.display()
        )
    })
}

fn run_smartconfig_sender(
    run_dir: &Path,
    fixture: &RouterSmartConfigFixture,
    companion_log: &mut File,
    companion_commands: &mut esp32_companion::CommandSession,
    sender: SmartConfigSenderKind,
    bssid_text: &str,
) -> Result<()> {
    match sender {
        SmartConfigSenderKind::CompanionSta => run_companion_smartconfig_sta_sender(
            fixture,
            companion_log,
            companion_commands,
            bssid_text,
        ),
        SmartConfigSenderKind::CompanionAp => {
            run_companion_smartconfig_ap_sender(fixture, companion_log, companion_commands)
        }
        SmartConfigSenderKind::RouterLocal => {
            run_router_smartconfig_sender(run_dir, fixture, bssid_text)
        }
    }
}

fn smartconfig_sender_passphrase(fixture: &RouterSmartConfigFixture) -> &str {
    if fixture.passphrase.is_empty() {
        "open"
    } else {
        fixture.passphrase.as_str()
    }
}

fn run_companion_smartconfig_sta_sender(
    fixture: &RouterSmartConfigFixture,
    companion_log: &mut File,
    companion_commands: &mut esp32_companion::CommandSession,
    bssid_text: &str,
) -> Result<()> {
    if fixture.passphrase.is_empty() {
        bail!(
            "companion-sta SmartConfig sender cannot encode an empty passphrase with the current companion CLI; use router-local or set ESP8266_HAL_SMARTCONFIG_PASSWORD"
        );
    }
    let sender_passphrase = smartconfig_sender_passphrase(fixture);
    let response = companion_session_command(
        companion_log,
        companion_commands,
        &format!(
            "SMARTCONFIG_STA_PROVISION ssid={} pass={} bssid={} sender_ssid={} sender_pass={} channel={} timeout_ms=90000",
            fixture.ssid, fixture.passphrase, bssid_text, fixture.ssid, sender_passphrase, 0
        ),
        Duration::from_secs(160),
    )?;
    if let Err(error) = ensure_ok_response(&response, "SMARTCONFIG_STA_PROVISION") {
        return Err(error);
    }
    Ok(())
}

fn run_companion_smartconfig_ap_sender(
    fixture: &RouterSmartConfigFixture,
    companion_log: &mut File,
    companion_commands: &mut esp32_companion::CommandSession,
) -> Result<()> {
    let passphrase = smartconfig_sender_passphrase(fixture);
    let response = companion_session_command(
        companion_log,
        companion_commands,
        &format!(
            "SMARTCONFIG_AP_PROVISION ssid={} pass={} channel={} timeout_ms=90000",
            fixture.ssid, passphrase, fixture.channel
        ),
        Duration::from_secs(160),
    )?;
    if let Err(error) = ensure_ok_response(&response, "SMARTCONFIG_AP_PROVISION") {
        return Err(error);
    }
    Ok(())
}

fn prestart_companion_smartconfig_ap(
    fixture: &RouterSmartConfigFixture,
    companion_log: &mut File,
    companion_commands: &mut esp32_companion::CommandSession,
) -> Result<()> {
    let passphrase = smartconfig_sender_passphrase(fixture);
    let response = companion_session_command(
        companion_log,
        companion_commands,
        &format!(
            "AP_START ssid={} pass={} channel={}",
            fixture.ssid, passphrase, fixture.channel
        ),
        Duration::from_secs(10),
    )?;
    ensure_ok_response(&response, "AP_START")?;
    Ok(())
}

fn run_router_smartconfig_sender(
    run_dir: &Path,
    fixture: &RouterSmartConfigFixture,
    bssid_text: &str,
) -> Result<()> {
    let sender = env::var("ESP8266_HAL_SMARTCONFIG_ROUTER_SENDER")
        .unwrap_or_else(|_| "/tmp/router_esptouch_sender".to_owned());
    let iface =
        env::var("ESP8266_HAL_SMARTCONFIG_ROUTER_IFACE").unwrap_or_else(|_| "br-lan".to_owned());
    let timeout_ms =
        env::var("ESP8266_HAL_SMARTCONFIG_SEND_TIMEOUT_MS").unwrap_or_else(|_| "90000".to_owned());
    let sender = shell_quote(&sender);
    let iface = shell_quote(&iface);
    let ssid = shell_quote(&fixture.ssid);
    let passphrase = shell_quote(&fixture.passphrase);
    let bssid = shell_quote(bssid_text);
    let phone_ip = shell_quote(&fixture.router.host);
    let timeout_ms = shell_quote(&timeout_ms);
    let script = format!(
        r#"set -e
sender={sender}
iface={iface}
ssid={ssid}
passphrase={passphrase}
bssid={bssid}
phone_ip={phone_ip}
timeout_ms={timeout_ms}
test -x "$sender"
set +e
output="$("$sender" "$iface" "$ssid" "$passphrase" "$bssid" "$phone_ip" "$timeout_ms" 2>&1)"
status=$?
set -e
printf '%s\n' "$output"
if [ "$status" -ne 0 ] && ! printf '%s\n' "$output" | grep -q '^ERR ack-timeout'; then
    exit "$status"
fi
"#
    );
    smartconfig_router_run(run_dir, fixture, "router-esptouch-send", &script).map(|_| ())
}

fn parse_bssid_from_router_output(output: &str) -> Result<[u8; 6]> {
    for line in output.lines() {
        if let Some(value) = line.strip_prefix("bssid=") {
            return parse_colon_mac(value);
        }
        if let Some(start) = line.find(r#""bssid":""#) {
            let value = &line[start + r#""bssid":""#.len()..];
            if let Some(end) = value.find('"') {
                return parse_colon_mac(&value[..end]);
            }
        }
    }
    bail!("router output did not include a BSSID")
}

fn format_colon_mac(mac: [u8; 6]) -> String {
    format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    )
}

fn parse_colon_mac(value: &str) -> Result<[u8; 6]> {
    let mut out = [0_u8; 6];
    let mut parts = value.split(':');
    for byte in &mut out {
        let part = parts
            .next()
            .ok_or_else(|| anyhow!("MAC `{value}` has too few octets"))?;
        *byte = u8::from_str_radix(part, 16)
            .with_context(|| format!("invalid MAC octet `{part}` in `{value}`"))?;
    }
    if parts.next().is_some() {
        bail!("MAC `{value}` has too many octets");
    }
    Ok(out)
}

fn promisc_success_marker(name: &str) -> &'static str {
    match name {
        "promisc_filters" => "promisc-management-ok",
        "promisc_control_filters" => "promisc-control-ok",
        _ => "promisc-ok",
    }
}

fn promisc_raw_kind(name: &str) -> &'static str {
    match name {
        "promisc_capture" => "ap-beacon",
        "promisc_filters" => "beacon",
        "promisc_data_companion" => "data",
        _ => "beacon",
    }
}

fn configured_promisc_channel() -> Result<u8> {
    let value = env::var("ESP8266_HAL_PROMISC_CHANNEL")
        .or_else(|_| env::var("ESP8266_HAL_COMPANION_AP_CHANNEL"))
        .unwrap_or_else(|_| "6".to_owned());
    let channel = value
        .parse::<u8>()
        .with_context(|| format!("invalid promiscuous channel `{value}`"))?;
    if !(1..=14).contains(&channel) {
        bail!("promiscuous channel must be in 1..=14, got {channel}");
    }
    Ok(channel)
}

#[derive(Debug, Clone)]
struct CompanionRig {
    port: PathBuf,
    chip_kind: Option<String>,
    mac: String,
}

#[derive(Debug, Clone)]
struct RouterCredentials {
    ssid: String,
    pass: String,
}

struct CompanionApSession {
    log: File,
    commands: esp32_companion::CommandSession,
    build_env: Vec<(&'static str, String)>,
}

#[derive(Debug, Clone)]
struct CompanionApOptions {
    credentials: RouterCredentials,
    channel: u8,
    hidden: bool,
}

impl CompanionApOptions {
    fn from_env() -> Result<Self> {
        Ok(Self {
            credentials: companion_ap_credentials()?,
            channel: companion_ap_channel()?,
            hidden: false,
        })
    }
}

fn start_companion_ap_for_scenario(
    run_dir: &Path,
    scenario: &Scenario,
    companion_port: &Path,
) -> Result<Option<CompanionApSession>> {
    if !matches!(
        scenario.name,
        "sta_connect"
            | "sta_scan_connect"
            | "sta_wrong_password"
            | "sta_wpa2_connect"
            | "sta_stale_channel_hint"
            | "sta_hidden_ssid"
            | "sta_scan"
            | "sta_dhcp"
            | "sta_static_ip"
            | "tcp_server"
            | "tcp_client"
    ) {
        return Ok(None);
    }

    let mut options = CompanionApOptions::from_env()?;
    match scenario.name {
        "sta_wrong_password" => {
            options.credentials = RouterCredentials {
                ssid: "esp8266-hal-sta".to_owned(),
                pass: "esp8266-hal-pass".to_owned(),
            };
        }
        "sta_wpa2_connect" => {
            options.credentials = RouterCredentials {
                ssid: "esp8266-hal-wpa2".to_owned(),
                pass: "esp8266-hal-pass".to_owned(),
            };
        }
        "sta_hidden_ssid" => {
            options.credentials = RouterCredentials {
                ssid: "esp8266-hal-hidden".to_owned(),
                pass: "esp8266-hal-pass".to_owned(),
            };
            options.hidden = true;
        }
        _ => {}
    }

    let ap_channel = options.channel;
    let mut session =
        start_companion_ap_session_with_options(run_dir, companion_port, true, options)?;
    if scenario.name == "sta_stale_channel_hint" {
        session
            .build_env
            .retain(|(key, _)| *key != "ESP8266_HAL_WIFI_CHANNEL");
        session.build_env.push((
            "ESP8266_HAL_WIFI_CHANNEL",
            wrong_channel_hint(ap_channel).to_string(),
        ));
    };
    if scenario.name == "sta_wrong_password" {
        session
            .build_env
            .retain(|(key, _)| *key != "ESP8266_HAL_WIFI_PASS");
        session
            .build_env
            .push(("ESP8266_HAL_WIFI_PASS", "esp8266-wrong-pass".to_owned()));
        session
            .build_env
            .push(("ESP8266_HAL_WIFI_CONNECT_TIMEOUT_MS", "30000".to_owned()));
    }
    if matches!(scenario.name, "sta_scan_connect" | "tcp_client") {
        session
            .build_env
            .retain(|(key, _)| *key != "ESP8266_HAL_WIFI_CHANNEL");
    }
    if let RuntimePlan::TcpClient { port, .. } = scenario.runtime {
        session
            .build_env
            .push(("ESP8266_HAL_TCP_CLIENT_HOST", "192.168.4.1".to_owned()));
        session
            .build_env
            .push(("ESP8266_HAL_TCP_CLIENT_PORT", port.to_string()));
    }
    if scenario.name == "sta_static_ip" || scenario.name == "tcp_client" {
        session
            .build_env
            .push(("ESP8266_HAL_STATIC_IPV4", "192.168.4.2".to_owned()));
        session
            .build_env
            .push(("ESP8266_HAL_STATIC_PREFIX", "24".to_owned()));
        session
            .build_env
            .push(("ESP8266_HAL_STATIC_GATEWAY", "192.168.4.1".to_owned()));
    }

    Ok(Some(session))
}

fn start_companion_ap_session(
    run_dir: &Path,
    companion_port: &Path,
    append_manifest_entry: bool,
) -> Result<CompanionApSession> {
    start_companion_ap_session_with_options(
        run_dir,
        companion_port,
        append_manifest_entry,
        CompanionApOptions::from_env()?,
    )
}

fn start_companion_ap_session_with_options(
    run_dir: &Path,
    companion_port: &Path,
    append_manifest_entry: bool,
    options: CompanionApOptions,
) -> Result<CompanionApSession> {
    let companion = probe_companion_rig(companion_port)?;
    if append_manifest_entry {
        append_companion_manifest(run_dir, &companion)?;
    }
    let mut log = open_companion_log(run_dir)?;
    let mut commands = esp32_companion::CommandSession::open(companion_port)?;
    ensure_companion_session_ready(&mut log, &mut commands)?;
    let reset_response = companion_session_command(
        &mut log,
        &mut commands,
        "RESET_WIFI",
        Duration::from_secs(5),
    )?;
    ensure_ok_response(&reset_response, "RESET_WIFI")?;

    ensure_companion_command_token("ssid", &options.credentials.ssid)?;
    ensure_companion_command_token("pass", &options.credentials.pass)?;
    let pass = if options.credentials.pass.is_empty() {
        "open"
    } else {
        options.credentials.pass.as_str()
    };
    let response = companion_session_command(
        &mut log,
        &mut commands,
        &format!(
            "AP_START ssid={} pass={} channel={} hidden={}",
            options.credentials.ssid,
            pass,
            options.channel,
            if options.hidden { 1 } else { 0 }
        ),
        Duration::from_secs(10),
    )?;
    let ok_line = ensure_ok_response(&response, "AP_START")?;
    if options.hidden && response_field(ok_line, "hidden") != Some("1") {
        bail!("companion AP_START did not confirm hidden=1: {ok_line}");
    }
    let build_env = vec![
        ("ESP8266_HAL_WIFI_SSID", options.credentials.ssid),
        ("ESP8266_HAL_WIFI_PASS", options.credentials.pass),
        ("ESP8266_HAL_WIFI_CHANNEL", options.channel.to_string()),
    ];

    Ok(CompanionApSession {
        log,
        commands,
        build_env,
    })
}

fn wrong_channel_hint(channel: u8) -> u8 {
    if channel == 6 {
        11
    } else if channel == 11 {
        6
    } else if channel > 1 {
        channel - 1
    } else {
        2
    }
}

fn stop_companion_ap(mut session: CompanionApSession) {
    let _ = companion_session_command(
        &mut session.log,
        &mut session.commands,
        "RESET_WIFI",
        Duration::from_secs(5),
    );
}

fn ensure_companion_command_token(name: &str, value: &str) -> Result<()> {
    if value.bytes().any(|byte| byte.is_ascii_whitespace()) {
        bail!("companion AP {name} may not contain ASCII whitespace");
    }
    Ok(())
}

fn companion_ap_channel() -> Result<u8> {
    let value = env::var("ESP8266_HAL_COMPANION_AP_CHANNEL").unwrap_or_else(|_| "6".to_owned());
    let channel = value
        .parse::<u8>()
        .with_context(|| format!("invalid ESP8266_HAL_COMPANION_AP_CHANNEL={value}"))?;
    if !(1..=14).contains(&channel) {
        bail!("ESP8266_HAL_COMPANION_AP_CHANNEL must be in 1..=14, got {channel}");
    }
    Ok(channel)
}

fn companion_ap_credentials() -> Result<RouterCredentials> {
    let ssid =
        env::var("ESP8266_HAL_COMPANION_AP_SSID").unwrap_or_else(|_| "esp8266-hal-sta".to_owned());
    let pass = env::var("ESP8266_HAL_COMPANION_AP_PASS").unwrap_or_default();
    Ok(RouterCredentials { ssid, pass })
}

fn run_companion_scenario_with_root(
    scenario: &Scenario,
    root: &Path,
    serial_port: &Path,
    companion_port: &Path,
) -> Result<()> {
    let example = scenario
        .example
        .ok_or_else(|| anyhow!("scenario `{}` has no example", scenario.name))?;
    let run_dir = prepare_run_dir(root, scenario.name)?;
    write_manifest(&run_dir, scenario, serial_port)?;

    let companion = probe_companion_rig(companion_port)?;
    append_companion_manifest(&run_dir, &companion)?;

    let mut companion_log = open_companion_log(&run_dir)?;
    ensure_companion_ready(&mut companion_log, &companion)?;

    let mut extra_env = Vec::<(&str, String)>::new();
    let mut promisc_channel = None;
    if is_promisc_rig_scenario(scenario.name) {
        let channel = configured_promisc_channel()?.to_string();
        extra_env.push(("ESP8266_HAL_PROMISC_CHANNEL", channel.clone()));
        promisc_channel = Some(channel);
    }

    if matches!(
        scenario.name,
        "esp_now_unicast"
            | "esp_now_encrypted"
            | "esp_now_unicast_companion"
            | "esp_now_encrypted_companion"
            | "esp_now_bad_key_companion"
    ) {
        extra_env.push(("ESP8266_HAL_ESPNOW_PEER", companion.mac.clone()));
    }

    let mut mixed_companion_ap =
        if matches!(scenario.name, "ap_sta" | "ap_sta_downstream_companion") {
            let session = start_companion_ap_session(&run_dir, companion_port, false)?;
            extra_env.extend(
                session
                    .build_env
                    .iter()
                    .map(|(key, value)| (*key, value.clone())),
            );
            Some(session)
        } else {
            None
        };

    build_example(&run_dir, example, scenario, &extra_env)?;
    let images = elf2image(&run_dir, example, scenario)?;
    flash_images(&run_dir, &images, serial_port)?;

    let result = match scenario.name {
        "ap_station_list" | "ap_station_list_companion" => {
            run_ap_station_list_companion(&run_dir, serial_port, &companion)
        }
        "ap_rejoin_companion" => run_ap_rejoin_companion(&run_dir, serial_port, &companion),
        "ap_attach_companion" => run_ap_attach_companion(&run_dir, serial_port, &companion),
        "ap_net_attach_companion" => run_ap_net_attach_companion(&run_dir, serial_port, &companion),
        "ap_dhcp_bind_companion" => run_ap_dhcp_bind_companion(&run_dir, serial_port, &companion),
        "ap_dhcp" | "ap_dhcp_companion" => run_ap_dhcp_companion(&run_dir, serial_port, &companion),
        "ap_sta" | "ap_sta_downstream_companion" => {
            let session = mixed_companion_ap
                .as_mut()
                .expect("mixed companion AP started before build");
            run_ap_sta_downstream_companion(&run_dir, serial_port, session)
        }
        "esp_now_unicast" | "esp_now_unicast_companion" => {
            run_esp_now_companion(&run_dir, serial_port, &companion, EspNowMode::Unicast)
        }
        "esp_now_encrypted" | "esp_now_encrypted_companion" => {
            run_esp_now_companion(&run_dir, serial_port, &companion, EspNowMode::Encrypted)
        }
        "esp_now_bad_key_companion" => {
            run_esp_now_companion(&run_dir, serial_port, &companion, EspNowMode::BadKey)
        }
        "promisc_capture"
        | "promisc_filters"
        | "promisc_control_filters"
        | "promisc_data_companion" => run_promisc_companion(
            &run_dir,
            serial_port,
            &companion,
            scenario.name,
            promisc_channel
                .as_deref()
                .expect("promisc preflight channel"),
            promisc_success_marker(scenario.name),
        ),
        other => bail!("unsupported companion scenario `{other}`"),
    };

    if let Some(session) = mixed_companion_ap {
        stop_companion_ap(session);
    }

    result
}

fn probe_companion_rig(port: &Path) -> Result<CompanionRig> {
    let device = esp32_companion::probe_device(port)
        .with_context(|| format!("failed to probe companion on {}", port.display()))?;
    let mac = device
        .mac
        .clone()
        .ok_or_else(|| anyhow!("companion on {} did not report a MAC", port.display()))?;
    Ok(CompanionRig {
        port: port.to_path_buf(),
        chip_kind: device.chip_kind,
        mac,
    })
}

fn append_companion_manifest(run_dir: &Path, companion: &CompanionRig) -> Result<()> {
    let path = run_dir.join("manifest.txt");
    let mut file = OpenOptions::new()
        .append(true)
        .open(&path)
        .with_context(|| format!("failed to append {}", path.display()))?;
    writeln!(file, "companion_port={}", companion.port.display())?;
    writeln!(file, "companion_mac={}", companion.mac)?;
    writeln!(
        file,
        "companion_chip={}",
        companion.chip_kind.as_deref().unwrap_or("unknown")
    )?;
    Ok(())
}

fn open_companion_log(run_dir: &Path) -> Result<File> {
    let path = run_dir.join("companion.log");
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("failed to open {}", path.display()))
}

fn ensure_companion_ready(log: &mut File, companion: &CompanionRig) -> Result<()> {
    let response = companion_command(log, companion, "PING", Duration::from_secs(5))
        .context("companion did not answer PING; flash the companion firmware first")?;
    ensure_ok_response(&response, "PING")?;
    companion_command(log, companion, "RESET_WIFI", Duration::from_secs(5))?;
    Ok(())
}

fn ensure_companion_session_ready(
    log: &mut File,
    commands: &mut esp32_companion::CommandSession,
) -> Result<()> {
    let response = companion_session_command(log, commands, "PING", Duration::from_secs(5))
        .context("companion did not answer PING; flash the companion firmware first")?;
    ensure_ok_response(&response, "PING")?;
    companion_session_command(log, commands, "RESET_WIFI", Duration::from_secs(5))?;
    Ok(())
}

fn companion_command(
    log: &mut File,
    companion: &CompanionRig,
    line: &str,
    timeout: Duration,
) -> Result<String> {
    writeln!(log, "$ {line}")?;
    let response = esp32_companion::send_command(&companion.port, line, timeout)?;
    writeln!(log, "{response}")?;
    Ok(response)
}

fn companion_session_command(
    log: &mut File,
    commands: &mut esp32_companion::CommandSession,
    line: &str,
    timeout: Duration,
) -> Result<String> {
    writeln!(log, "$ {line}")?;
    let response = commands.send_command(line, timeout)?;
    writeln!(log, "{response}")?;
    Ok(response)
}

fn ensure_ok_response<'a>(response: &'a str, command: &str) -> Result<&'a str> {
    let ok_line = response
        .lines()
        .rev()
        .find(|line| line.starts_with("OK "))
        .ok_or_else(|| anyhow!("companion command `{command}` failed: {response}"))?;
    Ok(ok_line)
}

fn is_retryable_raw80211_no_mem(response: &str) -> bool {
    response.lines().rev().any(|line| {
        line.starts_with("ERR ")
            && line.contains("code=raw80211-tx")
            && line.contains("detail=ESP_ERR_NO_MEM")
    })
}

fn response_field<'a>(response: &'a str, key: &str) -> Option<&'a str> {
    let line = response
        .lines()
        .rev()
        .find(|line| line.starts_with("OK ") || line.starts_with("ERR "))?;
    line.split_whitespace().find_map(|token| {
        let (candidate, value) = token.split_once('=')?;
        (candidate == key).then_some(value)
    })
}

fn command_field<'a>(command: &'a str, key: &str) -> Option<&'a str> {
    command.split_whitespace().find_map(|token| {
        let (candidate, value) = token.split_once('=')?;
        (candidate == key).then_some(value)
    })
}

fn companion_command_timeout(command: &str, default: Duration) -> Duration {
    command_field(command, "timeout_ms")
        .and_then(|value| value.parse::<u64>().ok())
        .and_then(|millis| Duration::from_millis(millis).checked_add(Duration::from_secs(10)))
        .map_or(default, |timeout| timeout.max(default))
}

fn bounded_timeout(requested: Duration, remaining: Duration) -> Option<Duration> {
    const MIN_TIMEOUT: Duration = Duration::from_millis(250);

    (remaining > MIN_TIMEOUT).then(|| requested.min(remaining))
}

fn bounded_companion_command_timeout(
    command: &str,
    default: Duration,
    remaining: Duration,
) -> Option<Duration> {
    bounded_timeout(companion_command_timeout(command, default), remaining)
}

fn companion_join_with_retry(
    log: &mut File,
    companion: &CompanionRig,
    command: &str,
    attempts: usize,
    expected_ip_prefix: Option<&str>,
) -> Result<String> {
    let mut last_error = None;
    let mut commands = esp32_companion::CommandSession::open(&companion.port)?;
    ensure_companion_session_ready(log, &mut commands)?;
    let has_channel_hint = command_field(command, "channel").is_some();
    let scan_command = if !has_channel_hint {
        command_field(command, "ssid").map(|ssid| format!("SCAN_FIND ssid={ssid}"))
    } else {
        None
    };
    let command_timeout = companion_command_timeout(command, Duration::from_secs(25));
    for attempt in 1..=attempts {
        let reset_command = if attempt == 1 || !has_channel_hint {
            "RESET_WIFI"
        } else {
            "STA_DISCONNECT"
        };
        let response =
            companion_session_command(log, &mut commands, reset_command, Duration::from_secs(5))
                .and_then(|_| {
                    if let Some(scan_command) = scan_command.as_deref() {
                        let scan_response = companion_session_command(
                            log,
                            &mut commands,
                            scan_command,
                            Duration::from_secs(10),
                        )?;
                        ensure_ok_response(&scan_response, "SCAN_FIND")?;
                    }
                    companion_session_command(log, &mut commands, command, command_timeout)
                });
        match response {
            Ok(response) => {
                if let Err(error) = ensure_ok_response(&response, "STA_JOIN") {
                    writeln!(log, "join attempt {attempt} failed: {error:#}")?;
                    last_error = Some(error);
                    thread::sleep(Duration::from_millis(750));
                    continue;
                }
                if let Some(prefix) = expected_ip_prefix {
                    let Some(ip) = response_field(&response, "ip") else {
                        let error =
                            anyhow!("companion join response did not include ip: {response}");
                        writeln!(log, "join attempt {attempt} failed: {error:#}")?;
                        last_error = Some(error);
                        thread::sleep(Duration::from_millis(750));
                        continue;
                    };
                    if !ip.starts_with(prefix) {
                        let error = anyhow!("unexpected companion lease `{ip}`");
                        writeln!(log, "join attempt {attempt} failed: {error:#}")?;
                        last_error = Some(error);
                        thread::sleep(Duration::from_millis(750));
                        continue;
                    }
                }
                return Ok(response);
            }
            Err(error) => {
                writeln!(log, "join attempt {attempt} failed: {error:#}")?;
                last_error = Some(error);
                thread::sleep(Duration::from_millis(750));
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow!("companion join attempts exhausted")))
}

fn companion_session_join_with_retry(
    session: &mut CompanionApSession,
    command: &str,
    ok_command: &str,
    attempts: usize,
    expected_ip_prefix: Option<&str>,
    deadline: Instant,
) -> Result<String> {
    let mut last_error = None;
    for attempt in 1..=attempts {
        let Some(command_timeout) = bounded_companion_command_timeout(
            command,
            Duration::from_secs(25),
            deadline.saturating_duration_since(Instant::now()),
        ) else {
            break;
        };
        let response = companion_session_command(
            &mut session.log,
            &mut session.commands,
            command,
            command_timeout,
        );
        match response {
            Ok(response) => {
                if let Err(error) = ensure_ok_response(&response, ok_command) {
                    writeln!(
                        session.log,
                        "session join attempt {attempt} failed: {error:#}"
                    )?;
                    last_error = Some(error);
                    if let Some(timeout) = bounded_timeout(
                        Duration::from_secs(5),
                        deadline.saturating_duration_since(Instant::now()),
                    ) {
                        let _ = companion_session_command(
                            &mut session.log,
                            &mut session.commands,
                            "STA_DISCONNECT",
                            timeout,
                        );
                    }
                    thread::sleep(Duration::from_millis(750));
                    continue;
                }
                if let Some(prefix) = expected_ip_prefix {
                    let Some(ip) = response_field(&response, "ip") else {
                        let error =
                            anyhow!("companion join response did not include ip: {response}");
                        writeln!(
                            session.log,
                            "session join attempt {attempt} failed: {error:#}"
                        )?;
                        last_error = Some(error);
                        if let Some(timeout) = bounded_timeout(
                            Duration::from_secs(5),
                            deadline.saturating_duration_since(Instant::now()),
                        ) {
                            let _ = companion_session_command(
                                &mut session.log,
                                &mut session.commands,
                                "STA_DISCONNECT",
                                timeout,
                            );
                        }
                        thread::sleep(Duration::from_millis(750));
                        continue;
                    };
                    if !ip.starts_with(prefix) {
                        let error = anyhow!("unexpected companion lease `{ip}`");
                        writeln!(
                            session.log,
                            "session join attempt {attempt} failed: {error:#}"
                        )?;
                        last_error = Some(error);
                        if let Some(timeout) = bounded_timeout(
                            Duration::from_secs(5),
                            deadline.saturating_duration_since(Instant::now()),
                        ) {
                            let _ = companion_session_command(
                                &mut session.log,
                                &mut session.commands,
                                "STA_DISCONNECT",
                                timeout,
                            );
                        }
                        thread::sleep(Duration::from_millis(750));
                        continue;
                    }
                }
                return Ok(response);
            }
            Err(error) => {
                writeln!(
                    session.log,
                    "session join attempt {attempt} failed: {error:#}"
                )?;
                last_error = Some(error);
                if let Some(timeout) = bounded_timeout(
                    Duration::from_secs(5),
                    deadline.saturating_duration_since(Instant::now()),
                ) {
                    let _ = companion_session_command(
                        &mut session.log,
                        &mut session.commands,
                        "STA_DISCONNECT",
                        timeout,
                    );
                }
                thread::sleep(Duration::from_millis(750));
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow!("companion session join attempts exhausted")))
}

fn run_ap_station_list_companion(
    run_dir: &Path,
    serial_port: &Path,
    companion: &CompanionRig,
) -> Result<()> {
    run_ap_station_list_flow(run_dir, serial_port, companion, false)
}

fn run_ap_rejoin_companion(
    run_dir: &Path,
    serial_port: &Path,
    companion: &CompanionRig,
) -> Result<()> {
    run_ap_station_list_flow(run_dir, serial_port, companion, true)
}

fn run_ap_station_list_flow(
    run_dir: &Path,
    serial_port: &Path,
    companion: &CompanionRig,
    rejoin_after_deauth: bool,
) -> Result<()> {
    const SUCCESS_MARKER: &str = "ap-station-list-ok";
    const FAILURE_MARKERS: &[&str] = &[
        "ap start err",
        "ap wait client timeout",
        "ap deauth err",
        "panic",
        "Guru Meditation",
    ];

    let uart_path = run_dir.join("uart.log");
    let mut uart_log = File::create(&uart_path)
        .with_context(|| format!("failed to create {}", uart_path.display()))?;
    let mut companion_log = open_companion_log(run_dir)?;

    let mut port = open_serial_port(serial_port)?;
    pulse_reset(port.as_mut())?;
    let start = Instant::now();
    let timeout = Duration::from_secs(70);
    let mut buffer = Vec::with_capacity(32 * 1024);
    let mut joined = false;

    while start.elapsed() < timeout {
        let mut chunk = [0_u8; 4096];
        match port.read(&mut chunk) {
            Ok(read) if read > 0 => {
                uart_log.write_all(&chunk[..read])?;
                buffer.extend_from_slice(&chunk[..read]);
                if buffer.len() > 32 * 1024 {
                    let excess = buffer.len() - 32 * 1024;
                    buffer.drain(0..excess);
                }

                let text = String::from_utf8_lossy(&buffer);
                if let Some(marker) = FAILURE_MARKERS
                    .iter()
                    .find(|marker| text.contains(**marker))
                {
                    bail!(
                        "uart reported failure marker `{marker}`; see {}",
                        uart_path.display()
                    );
                }
                if !joined && text.contains("ap stations:0") {
                    thread::sleep(Duration::from_secs(2));
                    let response = match companion_join_with_retry(
                        &mut companion_log,
                        companion,
                        "STA_JOIN ssid=esp8266-hal-ap pass=open timeout_ms=12000 wait_ip=0 channel=6",
                        10,
                        None,
                    ) {
                        Ok(response) => response,
                        Err(error) => {
                            let _ = drain_uart_for_failure(
                                port.as_mut(),
                                &mut uart_log,
                                &mut buffer,
                                Duration::from_secs(6),
                            );
                            if String::from_utf8_lossy(&buffer).contains(SUCCESS_MARKER) {
                                return Ok(());
                            }
                            return Err(error);
                        }
                    };
                    if response_field(&response, "connected") != Some("1") {
                        bail!("companion did not report link association: {response}");
                    }
                    joined = true;
                }
                if text.contains(SUCCESS_MARKER) {
                    if rejoin_after_deauth {
                        let response = companion_join_with_retry(
                            &mut companion_log,
                            companion,
                            "STA_JOIN ssid=esp8266-hal-ap pass=open timeout_ms=12000 wait_ip=0 channel=6",
                            6,
                            None,
                        )?;
                        if response_field(&response, "connected") != Some("1") {
                            bail!("companion did not report rejoin association: {response}");
                        }
                    }
                    return Ok(());
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error).context("failed while reading UART"),
        }
    }

    bail!(
        "timed out waiting for AP station-list companion flow; see {}",
        uart_path.display()
    )
}

fn run_ap_attach_companion(
    run_dir: &Path,
    serial_port: &Path,
    companion: &CompanionRig,
) -> Result<()> {
    const READY_MARKER: &str = "ap-link-ready";
    const FAILURE_MARKERS: &[&str] = &["ap start err", "ap-link err", "panic", "Guru Meditation"];

    let mut port = open_serial_port(serial_port)?;
    pulse_reset(port.as_mut())?;
    let uart_path = run_dir.join("uart.log");
    let mut uart_log = File::create(&uart_path)
        .with_context(|| format!("failed to create {}", uart_path.display()))?;
    let mut companion_log = open_companion_log(run_dir)?;
    let start = Instant::now();
    let timeout = Duration::from_secs(70);
    let mut buffer = Vec::with_capacity(32 * 1024);

    while start.elapsed() < timeout {
        let mut chunk = [0_u8; 4096];
        match port.read(&mut chunk) {
            Ok(read) if read > 0 => {
                uart_log.write_all(&chunk[..read])?;
                buffer.extend_from_slice(&chunk[..read]);
                if buffer.len() > 32 * 1024 {
                    let excess = buffer.len() - 32 * 1024;
                    buffer.drain(0..excess);
                }

                let text = String::from_utf8_lossy(&buffer);
                if let Some(marker) = FAILURE_MARKERS
                    .iter()
                    .find(|marker| text.contains(**marker))
                {
                    bail!(
                        "uart reported failure marker `{marker}`; see {}",
                        uart_path.display()
                    );
                }
                if text.contains(READY_MARKER) {
                    thread::sleep(Duration::from_secs(2));
                    let response = match companion_join_with_retry(
                        &mut companion_log,
                        companion,
                        "STA_JOIN ssid=esp8266-hal-ap pass=open timeout_ms=12000 wait_ip=0 channel=6",
                        6,
                        None,
                    ) {
                        Ok(response) => response,
                        Err(error) => {
                            let _ = drain_uart_for_failure(
                                port.as_mut(),
                                &mut uart_log,
                                &mut buffer,
                                Duration::from_secs(6),
                            );
                            return Err(error);
                        }
                    };
                    if response_field(&response, "connected") != Some("1") {
                        bail!("companion did not report link association: {response}");
                    }
                    return Ok(());
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error).context("failed while reading UART"),
        }
    }

    bail!(
        "timed out waiting for AP attach companion flow; see {}",
        uart_path.display()
    )
}

fn run_ap_net_attach_companion(
    run_dir: &Path,
    serial_port: &Path,
    companion: &CompanionRig,
) -> Result<()> {
    const READY_MARKER: &str = "ap-net-attach-ready";
    const FAILURE_MARKERS: &[&str] = &[
        "ap start err",
        "ap-net link err",
        "ap-net mac err",
        "panic",
        "Guru Meditation",
    ];

    let mut port = open_serial_port(serial_port)?;
    pulse_reset(port.as_mut())?;
    let uart_path = run_dir.join("uart.log");
    let mut uart_log = File::create(&uart_path)
        .with_context(|| format!("failed to create {}", uart_path.display()))?;
    let mut companion_log = open_companion_log(run_dir)?;
    let start = Instant::now();
    let timeout = Duration::from_secs(70);
    let mut buffer = Vec::with_capacity(32 * 1024);

    while start.elapsed() < timeout {
        let mut chunk = [0_u8; 4096];
        match port.read(&mut chunk) {
            Ok(read) if read > 0 => {
                uart_log.write_all(&chunk[..read])?;
                buffer.extend_from_slice(&chunk[..read]);
                if buffer.len() > 32 * 1024 {
                    let excess = buffer.len() - 32 * 1024;
                    buffer.drain(0..excess);
                }

                let text = String::from_utf8_lossy(&buffer);
                if let Some(marker) = FAILURE_MARKERS
                    .iter()
                    .find(|marker| text.contains(**marker))
                {
                    bail!(
                        "uart reported failure marker `{marker}`; see {}",
                        uart_path.display()
                    );
                }
                if text.contains(READY_MARKER) {
                    thread::sleep(Duration::from_secs(2));
                    let response = match companion_join_with_retry(
                        &mut companion_log,
                        companion,
                        "STA_JOIN ssid=esp8266-hal-ap pass=open timeout_ms=12000 wait_ip=0 channel=6",
                        6,
                        None,
                    ) {
                        Ok(response) => response,
                        Err(error) => {
                            let _ = drain_uart_for_failure(
                                port.as_mut(),
                                &mut uart_log,
                                &mut buffer,
                                Duration::from_secs(6),
                            );
                            return Err(error);
                        }
                    };
                    if response_field(&response, "connected") != Some("1") {
                        bail!("companion did not report link association: {response}");
                    }
                    return Ok(());
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error).context("failed while reading UART"),
        }
    }

    bail!(
        "timed out waiting for AP net-attach companion flow; see {}",
        uart_path.display()
    )
}

fn run_ap_dhcp_bind_companion(
    run_dir: &Path,
    serial_port: &Path,
    companion: &CompanionRig,
) -> Result<()> {
    const READY_MARKER: &str = "ap-dhcp-bind-ready";
    const FAILURE_MARKERS: &[&str] = &[
        "ap start err",
        "ap-net link err",
        "ap-net mac err",
        "ap-dhcp bind err",
        "panic",
        "Guru Meditation",
    ];

    let mut port = open_serial_port(serial_port)?;
    pulse_reset(port.as_mut())?;
    let uart_path = run_dir.join("uart.log");
    let mut uart_log = File::create(&uart_path)
        .with_context(|| format!("failed to create {}", uart_path.display()))?;
    let mut companion_log = open_companion_log(run_dir)?;
    let start = Instant::now();
    let timeout = Duration::from_secs(70);
    let mut buffer = Vec::with_capacity(32 * 1024);

    while start.elapsed() < timeout {
        let mut chunk = [0_u8; 4096];
        match port.read(&mut chunk) {
            Ok(read) if read > 0 => {
                uart_log.write_all(&chunk[..read])?;
                buffer.extend_from_slice(&chunk[..read]);
                if buffer.len() > 32 * 1024 {
                    let excess = buffer.len() - 32 * 1024;
                    buffer.drain(0..excess);
                }

                let text = String::from_utf8_lossy(&buffer);
                if let Some(marker) = FAILURE_MARKERS
                    .iter()
                    .find(|marker| text.contains(**marker))
                {
                    bail!(
                        "uart reported failure marker `{marker}`; see {}",
                        uart_path.display()
                    );
                }
                if text.contains(READY_MARKER) {
                    thread::sleep(Duration::from_secs(2));
                    let response = match companion_join_with_retry(
                        &mut companion_log,
                        companion,
                        "STA_JOIN ssid=esp8266-hal-ap pass=open timeout_ms=12000 wait_ip=0 channel=6",
                        6,
                        None,
                    ) {
                        Ok(response) => response,
                        Err(error) => {
                            let _ = drain_uart_for_failure(
                                port.as_mut(),
                                &mut uart_log,
                                &mut buffer,
                                Duration::from_secs(6),
                            );
                            return Err(error);
                        }
                    };
                    if response_field(&response, "connected") != Some("1") {
                        bail!("companion did not report link association: {response}");
                    }
                    return Ok(());
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error).context("failed while reading UART"),
        }
    }

    bail!(
        "timed out waiting for AP DHCP bind companion flow; see {}",
        uart_path.display()
    )
}

fn run_ap_dhcp_companion(
    run_dir: &Path,
    serial_port: &Path,
    companion: &CompanionRig,
) -> Result<()> {
    const SUCCESS_MARKER: &str = "ap-dhcp-ok";
    const FAILURE_MARKERS: &[&str] = &[
        "ap start err",
        "ap-net-timeout",
        "ap-dhcp bind err",
        "panic",
        "Guru Meditation",
    ];

    let mut port = open_serial_port(serial_port)?;
    pulse_reset(port.as_mut())?;
    let uart_path = run_dir.join("uart.log");
    let mut uart_log = File::create(&uart_path)
        .with_context(|| format!("failed to create {}", uart_path.display()))?;
    let mut companion_log = open_companion_log(run_dir)?;
    let start = Instant::now();
    let timeout = Duration::from_secs(90);
    let mut buffer = Vec::with_capacity(32 * 1024);
    let mut joined = false;

    while start.elapsed() < timeout {
        let mut chunk = [0_u8; 4096];
        match port.read(&mut chunk) {
            Ok(read) if read > 0 => {
                uart_log.write_all(&chunk[..read])?;
                buffer.extend_from_slice(&chunk[..read]);
                if buffer.len() > 32 * 1024 {
                    let excess = buffer.len() - 32 * 1024;
                    buffer.drain(0..excess);
                }

                let text = String::from_utf8_lossy(&buffer);
                if joined && text.contains(SUCCESS_MARKER) {
                    return Ok(());
                }
                if !joined && text.contains("ap-dhcp-ready") {
                    thread::sleep(Duration::from_secs(4));
                    let response = match companion_join_with_retry(
                        &mut companion_log,
                        companion,
                        "STA_JOIN ssid=esp8266-hal-ap pass=open timeout_ms=30000 wait_ip=1 channel=6",
                        10,
                        Some("192.168.4."),
                    ) {
                        Ok(response) => response,
                        Err(error) => {
                            let _ = drain_uart_for_failure(
                                port.as_mut(),
                                &mut uart_log,
                                &mut buffer,
                                Duration::from_secs(6),
                            );
                            return Err(error);
                        }
                    };
                    if response_field(&response, "ip").is_none() {
                        bail!("companion did not report a DHCP lease: {response}");
                    }
                    joined = true;
                }
                if joined && text.contains(SUCCESS_MARKER) {
                    return Ok(());
                }
                if let Some(marker) = FAILURE_MARKERS
                    .iter()
                    .find(|marker| text.contains(**marker))
                {
                    bail!(
                        "uart reported failure marker `{marker}`; see {}",
                        uart_path.display()
                    );
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error).context("failed while reading UART"),
        }
    }

    bail!(
        "timed out waiting for AP DHCP companion flow; see {}",
        uart_path.display()
    )
}

fn run_ap_sta_downstream_companion(
    run_dir: &Path,
    serial_port: &Path,
    companion: &mut CompanionApSession,
) -> Result<()> {
    const SUCCESS_MARKER: &str = "ap-sta-downstream-ok";
    const FAILURE_MARKERS: &[&str] = &[
        "ap_sta err connect-timeout",
        "ap_sta err connect-disconnected",
        "ap_sta err connect-backend",
        "ap_sta err connect-other",
        "ap_sta err stations",
        "ap_sta err dhcp",
        "ap_sta err dhcp-timeout",
        "ap_sta err downstream-timeout",
        "panic",
        "Guru Meditation",
    ];

    let mut port = open_serial_port(serial_port)?;
    pulse_reset(port.as_mut())?;
    let uart_path = run_dir.join("uart.log");
    let mut uart_log = File::create(&uart_path)
        .with_context(|| format!("failed to create {}", uart_path.display()))?;
    let start = Instant::now();
    let timeout = Duration::from_secs(70);
    let deadline = start + timeout;
    let mut buffer = Vec::with_capacity(32 * 1024);
    let mut joined = false;

    while start.elapsed() < timeout {
        let mut chunk = [0_u8; 4096];
        match port.read(&mut chunk) {
            Ok(read) if read > 0 => {
                uart_log.write_all(&chunk[..read])?;
                buffer.extend_from_slice(&chunk[..read]);
                if buffer.len() > 32 * 1024 {
                    let excess = buffer.len() - 32 * 1024;
                    buffer.drain(0..excess);
                }

                let text = String::from_utf8_lossy(&buffer);
                if let Some(marker) = FAILURE_MARKERS
                    .iter()
                    .find(|marker| text.contains(**marker))
                {
                    bail!(
                        "uart reported failure marker `{marker}`; see {}",
                        uart_path.display()
                    );
                }
                if !joined && text.contains("ap-sta-dhcp-ready") {
                    let response = match companion_session_join_with_retry(
                        companion,
                        "STA_JOIN_KEEP_AP ssid=esp8266-hal-apsta pass=open timeout_ms=30000 wait_ip=1 channel=6",
                        "STA_JOIN_KEEP_AP",
                        6,
                        Some("192.168.4."),
                        deadline,
                    ) {
                        Ok(response) => response,
                        Err(error) => {
                            let _ = drain_uart_for_failure(
                                port.as_mut(),
                                &mut uart_log,
                                &mut buffer,
                                Duration::from_secs(6),
                            );
                            if String::from_utf8_lossy(&buffer).contains(SUCCESS_MARKER) {
                                return Ok(());
                            }
                            return Err(error);
                        }
                    };
                    if response_field(&response, "ip").is_none() {
                        bail!("companion did not report a mixed-mode DHCP lease: {response}");
                    }
                    joined = true;
                }
                if joined && text.contains(SUCCESS_MARKER) {
                    return Ok(());
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error).context("failed while reading UART"),
        }
    }

    bail!(
        "timed out waiting for AP+STA downstream companion flow; see {}",
        uart_path.display()
    )
}

#[allow(dead_code)]
fn run_wps_companion(run_dir: &Path, serial_port: &Path, companion: &CompanionRig) -> Result<()> {
    let mut companion_log = open_companion_log(run_dir)?;
    let mut companion_commands = esp32_companion::CommandSession::open(&companion.port)?;
    ensure_companion_session_ready(&mut companion_log, &mut companion_commands)?;
    let wps_response = companion_session_command(
        &mut companion_log,
        &mut companion_commands,
        &format!(
            "WPS_AP_START ssid=esp8266-hal-wps pass=esp8266wps channel=6 mode=pin pin={WPS_PIN}"
        ),
        Duration::from_secs(10),
    )?;
    ensure_ok_response(&wps_response, "WPS_AP_START")?;
    thread::sleep(Duration::from_millis(2_000));

    let mut port = open_serial_port(serial_port)?;
    pulse_reset(port.as_mut())?;

    let uart_path = run_dir.join("uart.log");
    let mut uart_log = File::create(&uart_path)
        .with_context(|| format!("failed to create {}", uart_path.display()))?;
    let start = Instant::now();
    let mut buffer = Vec::with_capacity(32 * 1024);
    let registrar_started = true;

    while start.elapsed() < Duration::from_secs(150) {
        let mut chunk = [0_u8; 4096];
        match port.read(&mut chunk) {
            Ok(read) if read > 0 => {
                uart_log.write_all(&chunk[..read])?;
                buffer.extend_from_slice(&chunk[..read]);
                if buffer.len() > 32 * 1024 {
                    let excess = buffer.len() - 32 * 1024;
                    buffer.drain(0..excess);
                }

                let text = String::from_utf8_lossy(&buffer);

                if text.contains("wps-ok") {
                    let response = companion_session_command(
                        &mut companion_log,
                        &mut companion_commands,
                        "WPS_AP_WAIT timeout_ms=10000",
                        Duration::from_secs(12),
                    )?;
                    ensure_ok_response(&response, "WPS_AP_WAIT")?;
                    return Ok(());
                }
                if let Some(marker) = [
                    "wps sta start err",
                    "wps session err",
                    "wps start err",
                    "wps-failed",
                    "wps-timeout",
                    "panic",
                    "Guru Meditation",
                ]
                .iter()
                .find(|marker| text.contains(**marker))
                {
                    let registrar = companion_session_command(
                        &mut companion_log,
                        &mut companion_commands,
                        "WPS_AP_WAIT timeout_ms=1000",
                        Duration::from_secs(3),
                    )
                    .unwrap_or_else(|error| format!("WPS_AP_WAIT query failed: {error:#}"));
                    bail!(
                        "uart reported failure marker `{marker}`; registrar `{registrar}`; see {}",
                        uart_path.display()
                    );
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error).context("failed while reading UART"),
        }
    }

    if registrar_started {
        bail!(
            "timed out waiting for WPS PIN enrollment; see {}",
            uart_path.display()
        );
    }
    bail!(
        "timed out waiting for ESP8266 WPS output; see {}",
        uart_path.display()
    )
}

#[allow(dead_code)]
fn extract_wps_pin(text: &str) -> Option<&str> {
    let start = text.find("wps pin=")? + "wps pin=".len();
    let pin = text.get(start..start + 8)?;
    pin.as_bytes()
        .iter()
        .all(|byte| byte.is_ascii_digit())
        .then_some(pin)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EspNowMode {
    Unicast,
    Encrypted,
    BadKey,
}

fn run_esp_now_companion(
    run_dir: &Path,
    serial_port: &Path,
    companion: &CompanionRig,
    mode: EspNowMode,
) -> Result<()> {
    const TX_ATTEMPTS: usize = 5;
    const TX_RETRY_BACKOFF: Duration = Duration::from_millis(750);
    const FAILURE_MARKERS: &[&str] = &[
        "esp-now start err",
        "esp-now session err",
        "esp-now channel err",
        "esp-now peer err",
        "panic",
        "Guru Meditation",
    ];

    let dut = esp32_companion::probe_device(serial_port)
        .with_context(|| format!("failed to probe DUT on {}", serial_port.display()))?;
    let dut_mac = dut
        .mac
        .ok_or_else(|| anyhow!("DUT did not report a MAC via esptool"))?;

    let mut port = open_serial_port(serial_port)?;
    pulse_reset(port.as_mut())?;
    let uart_path = run_dir.join("uart.log");
    let mut uart_log = File::create(&uart_path)
        .with_context(|| format!("failed to create {}", uart_path.display()))?;
    let mut companion_log = open_companion_log(run_dir)?;
    let mut companion_commands = esp32_companion::CommandSession::open(&companion.port)?;
    ensure_companion_session_ready(&mut companion_log, &mut companion_commands)?;

    let start = Instant::now();
    let timeout = Duration::from_secs(30);
    let mut buffer = Vec::with_capacity(32 * 1024);
    let mut sent = false;
    let mut send_deadline = None;

    while start.elapsed() < timeout {
        let mut chunk = [0_u8; 4096];
        match port.read(&mut chunk) {
            Ok(read) if read > 0 => {
                uart_log.write_all(&chunk[..read])?;
                buffer.extend_from_slice(&chunk[..read]);
                if buffer.len() > 32 * 1024 {
                    let excess = buffer.len() - 32 * 1024;
                    buffer.drain(0..excess);
                }

                let text = String::from_utf8_lossy(&buffer);
                if let Some(marker) = FAILURE_MARKERS
                    .iter()
                    .find(|marker| text.contains(**marker))
                {
                    bail!(
                        "uart reported failure marker `{marker}`; see {}",
                        uart_path.display()
                    );
                }
                if !sent && text.contains("esp-now rx ready") {
                    let command = match mode {
                        EspNowMode::Unicast => format!(
                            "ESPNOW_TX channel=6 peer={dut_mac} encrypt=0 lmk=none pmk=none payload=657370383236362d68616c2d6573702d6e6f77 timeout_ms=10000"
                        ),
                        EspNowMode::Encrypted => format!(
                            "ESPNOW_TX channel=6 peer={dut_mac} encrypt=1 lmk=42424242424242424242424242424242 pmk=none payload=657370383236362d68616c2d6573702d6e6f77 timeout_ms=10000"
                        ),
                        EspNowMode::BadKey => format!(
                            "ESPNOW_TX channel=6 peer={dut_mac} encrypt=1 lmk=41414141414141414141414141414141 pmk=none payload=6261646b6579 timeout_ms=10000"
                        ),
                    };
                    let mut response = String::new();
                    for attempt in 1..=TX_ATTEMPTS {
                        response = companion_session_command(
                            &mut companion_log,
                            &mut companion_commands,
                            &command,
                            Duration::from_secs(15),
                        )?;
                        if mode == EspNowMode::BadKey
                            || ensure_ok_response(&response, "ESPNOW_TX").is_ok()
                        {
                            break;
                        }
                        if attempt < TX_ATTEMPTS {
                            thread::sleep(TX_RETRY_BACKOFF);
                        }
                    }
                    if mode != EspNowMode::BadKey {
                        ensure_ok_response(&response, "ESPNOW_TX")?;
                    }
                    sent = true;
                    send_deadline = Some(Instant::now() + Duration::from_secs(5));
                }
                if text.contains("esp-now-rx-ok") {
                    if mode == EspNowMode::BadKey {
                        bail!("bad-key companion flow unexpectedly delivered an ESP-NOW frame");
                    }
                    return Ok(());
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error).context("failed while reading UART"),
        }

        if mode == EspNowMode::BadKey {
            if let Some(deadline) = send_deadline {
                if Instant::now() >= deadline {
                    return Ok(());
                }
            }
        }
    }

    bail!(
        "timed out waiting for ESP-NOW companion flow; see {}",
        uart_path.display()
    )
}

fn run_promisc_companion(
    run_dir: &Path,
    serial_port: &Path,
    companion: &CompanionRig,
    scenario_name: &str,
    channel: &str,
    success_marker: &str,
) -> Result<()> {
    const FAILURE_MARKERS: &[&str] = &[
        "promisc start err",
        "promisc set channel err",
        "promisc enable err",
        "promisc ctrl filter err",
        "promisc api state err",
        "promisc api restart err",
        "promisc stop err",
        "promisc session err",
        "promisc timeout",
        "panic",
        "Guru Meditation",
    ];

    let raw_kind = promisc_raw_kind(scenario_name);
    let raw_count = match scenario_name {
        "promisc_control_filters" => 32,
        "promisc_capture" | "promisc_filters" => 32,
        _ => 48,
    };
    let raw_interval_ms = match scenario_name {
        "promisc_capture" | "promisc_filters" => 8,
        _ => 5,
    };
    let destination = if scenario_name == "promisc_control_filters" {
        let dut = esp32_companion::probe_device(serial_port)
            .with_context(|| format!("failed to probe DUT on {}", serial_port.display()))?;
        Some(
            dut.mac
                .ok_or_else(|| anyhow!("DUT did not report a MAC via esptool"))?,
        )
    } else {
        None
    };

    let uart_path = run_dir.join("uart.log");
    let mut uart_log = File::create(&uart_path)
        .with_context(|| format!("failed to create {}", uart_path.display()))?;
    let mut companion_log = open_companion_log(run_dir)?;
    let mut companion_commands = esp32_companion::CommandSession::open(&companion.port)?;
    ensure_companion_session_ready(&mut companion_log, &mut companion_commands)?;
    let passive_ap_beacon = raw_kind == "ap-beacon";
    if passive_ap_beacon {
        let response = companion_session_command(
            &mut companion_log,
            &mut companion_commands,
            &format!("AP_START ssid=esp8266-hal-promisc pass=open channel={channel}"),
            Duration::from_secs(30),
        )?;
        ensure_ok_response(&response, "AP_START")?;
        thread::sleep(Duration::from_millis(250));
    }

    let mut port = open_serial_port(serial_port)?;
    pulse_reset(port.as_mut())?;

    let start = Instant::now();
    let timeout = match scenario_name {
        "promisc_capture" | "promisc_filters" => Duration::from_secs(75),
        _ => Duration::from_secs(45),
    };
    let mut buffer = Vec::with_capacity(32 * 1024);
    let mut promisc_started = false;
    let mut next_traffic = start;

    while start.elapsed() < timeout {
        let mut chunk = [0_u8; 4096];
        match port.read(&mut chunk) {
            Ok(read) if read > 0 => {
                uart_log.write_all(&chunk[..read])?;
                buffer.extend_from_slice(&chunk[..read]);
                if buffer.len() > 32 * 1024 {
                    let excess = buffer.len() - 32 * 1024;
                    buffer.drain(0..excess);
                }

                let text = String::from_utf8_lossy(&buffer);
                if let Some(marker) = FAILURE_MARKERS
                    .iter()
                    .find(|marker| text.contains(**marker))
                {
                    bail!(
                        "uart reported failure marker `{marker}`; see {}",
                        uart_path.display()
                    );
                }
                if text.contains("promisc started") {
                    promisc_started = true;
                }
                if text.contains(success_marker) {
                    return Ok(());
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error).context("failed while reading UART"),
        }

        if promisc_started && !passive_ap_beacon && Instant::now() >= next_traffic {
            let destination = destination
                .as_ref()
                .map(|mac| format!(" dest={mac}"))
                .unwrap_or_default();
            let traffic_response = companion_session_command(
                &mut companion_log,
                &mut companion_commands,
                &format!(
                    "RAW80211_TX kind={} channel={} count={} interval_ms={}{}",
                    raw_kind, channel, raw_count, raw_interval_ms, destination
                ),
                Duration::from_secs(30),
            )?;
            if let Err(error) = ensure_ok_response(&traffic_response, "RAW80211_TX") {
                if is_retryable_raw80211_no_mem(&traffic_response) {
                    writeln!(
                        companion_log,
                        "retrying RAW80211_TX after companion ESP_ERR_NO_MEM"
                    )?;
                    next_traffic = Instant::now() + Duration::from_millis(250);
                    continue;
                }
                return Err(error);
            }
            next_traffic = Instant::now() + Duration::from_millis(400);
        }
    }

    bail!(
        "timed out waiting for promiscuous companion flow; see {}",
        uart_path.display()
    )
}

fn command_available(command: &str) -> Result<()> {
    let status = Command::new("bash")
        .arg("-lc")
        .arg(format!("command -v {command}"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("failed to invoke shell while checking `{command}`"))?;
    if status.success() {
        Ok(())
    } else {
        bail!("required command `{command}` not found in PATH")
    }
}

fn prepare_run_dir(root: &Path, scenario: &str) -> Result<PathBuf> {
    fs::create_dir_all(root)
        .with_context(|| format!("failed to create artifacts root {}", root.display()))?;

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before epoch")
        .as_secs();
    let run_dir = root.join(format!("{scenario}-{stamp}"));
    fs::create_dir_all(&run_dir)
        .with_context(|| format!("failed to create run dir {}", run_dir.display()))?;
    Ok(run_dir)
}

fn write_manifest(run_dir: &Path, scenario: &Scenario, serial_port: &Path) -> Result<()> {
    let mut file = File::create(run_dir.join("manifest.txt")).with_context(|| {
        format!(
            "failed to create {}",
            run_dir.join("manifest.txt").display()
        )
    })?;
    writeln!(file, "scenario={}", scenario.name)?;
    writeln!(file, "description={}", scenario.description)?;
    writeln!(file, "example={}", scenario.example.unwrap_or("-"))?;
    writeln!(file, "profile={}", scenario.profile)?;
    writeln!(file, "features={}", scenario.features.join(","))?;
    writeln!(file, "runtime={}", scenario.runtime.label())?;
    writeln!(file, "matrix_lane={}", scenario_matrix_lane_label(scenario))?;
    writeln!(
        file,
        "resources={}",
        scenario_resource_labels(scenario).join(",")
    )?;
    writeln!(file, "serial_port={}", serial_port.display())?;
    Ok(())
}

fn build_example(
    run_dir: &Path,
    example: &str,
    scenario: &Scenario,
    extra_env: &[(&str, String)],
) -> Result<()> {
    let log_path = run_dir.join("build.log");
    let mut log = File::create(&log_path)
        .with_context(|| format!("failed to create {}", log_path.display()))?;

    let feature_list = scenario.features.join(",");
    let env_prefix = build_env_prefix(run_dir, scenario, extra_env)?;
    let command = format!(
        "set -euo pipefail; source .envrc && {}cargo build --{} --example {} --target {} {} --no-default-features --features {}",
        env_prefix,
        scenario.profile,
        example,
        TARGET,
        BUILD_STD,
        feature_list
    );
    writeln!(log, "$ {command}")?;

    let output = Command::new("bash")
        .arg("-lc")
        .arg(&command)
        .current_dir(repo_root()?)
        .output()
        .context("failed to build example")?;
    log.write_all(&output.stdout)?;
    log.write_all(&output.stderr)?;

    if !output.status.success() {
        bail!("build failed; see {}", log_path.display());
    }

    Ok(())
}

fn build_env_prefix(
    run_dir: &Path,
    scenario: &Scenario,
    extra_env: &[(&str, String)],
) -> Result<String> {
    let mut prefix = String::new();

    if scenario.name == "sta_static_ip" {
        let config = detect_static_ipv4_config()?;
        fs::write(
            static_ipv4_file(run_dir),
            format!(
                "address={}\nprefix_len={}\ngateway={}\n",
                config.address, config.prefix_len, config.gateway
            ),
        )
        .with_context(|| format!("failed to record static IPv4 config {}", config.address))?;
        prefix.push_str(&format!(
            "ESP8266_HAL_STATIC_IPV4={} ESP8266_HAL_STATIC_PREFIX={} ESP8266_HAL_STATIC_GATEWAY={} ",
            config.address, config.prefix_len, config.gateway
        ));
    }

    if env::var_os("ESP8266_HAL_WIFI_START_TRACE").is_some() {
        prefix.push_str("ESP8266_HAL_WIFI_START_TRACE=1 ");
    }

    if scenario.name == "promisc_filters" {
        prefix.push_str("ESP8266_HAL_PROMISC_FILTER=management ");
    }

    if scenario.name == "promisc_control_filter_api" {
        prefix.push_str("ESP8266_HAL_PROMISC_API_ONLY=1 ");
    }

    if !scenario.features.contains(&"wifi-ap") {
        prefix.push_str("ESP8266_HAL_AP_RX_FRAME_BYTES=256 ");
    }

    if scenario.name == "promisc_control_filters" {
        prefix.push_str("ESP8266_HAL_PROMISC_FILTER=control ESP8266_HAL_PROMISC_CTRL_FILTER=all ");
    }

    if scenario.name == "promisc_data_companion" {
        prefix.push_str("ESP8266_HAL_PROMISC_FILTER=data ");
    }

    if matches!(scenario.name, "eap_sta_assoc" | "eap_wrong_password") {
        prefix.push_str("ESP8266_HAL_EAP_SSID=esp8266-router-eap ESP8266_HAL_EAP_IDENTITY=esp8266-user ESP8266_HAL_EAP_USERNAME=esp8266-user ");
        if scenario.name == "eap_wrong_password" {
            prefix.push_str(
                "ESP8266_HAL_EAP_PASSWORD=esp8266-wrong-password ESP8266_HAL_EAP_EXPECT_FAILURE=1 ",
            );
        } else {
            prefix.push_str("ESP8266_HAL_EAP_PASSWORD=esp8266-password ");
        }
        prefix.push_str("ESP8266_HAL_EAP_CHANNEL=11 ");
    }

    if scenario.name == "smartconfig" {
        for key in [
            "ESP8266_HAL_SMARTCONFIG_ENABLE_LOG",
            "ESP8266_HAL_SMARTCONFIG_EVENT_TRACE",
            "ESP8266_HAL_SMARTCONFIG_SNIFFER_TRACE",
            "ESP8266_HAL_SMARTCONFIG_DIAG",
            "ESP8266_HAL_SMARTCONFIG_DIAG_WAIT",
            "ESP8266_HAL_SMARTCONFIG_RX_TRACE",
            "ESP8266_HAL_SMARTCONFIG_RX_DIAG",
            "ESP8266_HAL_SMARTCONFIG_STATIC_ACK",
        ] {
            if env::var_os(key).is_some() {
                prefix.push_str(key);
                prefix.push_str("=1 ");
            }
        }
    }

    if matches!(
        scenario.name,
        "ap_station_list" | "ap_station_list_companion" | "ap_rejoin_companion"
    ) {
        prefix.push_str("ESP8266_HAL_AP_WAIT_CLIENT=1 ESP8266_HAL_AP_RX_FRAME_BYTES=640 ");
    }

    if scenario.name == "ap_attach_companion" {
        prefix.push_str("ESP8266_HAL_AP_ATTACH_ONLY=1 ESP8266_HAL_AP_RX_RELEASE_ONLY=1 ESP8266_HAL_AP_RX_FRAME_BYTES=640 ");
    }

    if scenario.name == "ap_net_attach_companion" {
        prefix.push_str(
            "ESP8266_HAL_AP_DHCP=1 ESP8266_HAL_AP_DELAY_BEFORE_NET_MS=0 ESP8266_HAL_AP_DHCP_GRACE_MS=3000 ESP8266_HAL_AP_NET_ATTACH_ONLY=1 ESP8266_HAL_AP_RX_FRAME_BYTES=640 ",
        );
    }

    if scenario.name == "ap_dhcp_bind_companion" {
        prefix.push_str(
            "ESP8266_HAL_AP_DHCP=1 ESP8266_HAL_AP_DELAY_BEFORE_NET_MS=0 ESP8266_HAL_AP_DHCP_GRACE_MS=3000 ESP8266_HAL_AP_DHCP_BIND_ONLY=1 ESP8266_HAL_AP_RX_FRAME_BYTES=640 ",
        );
    }

    if scenario.name == "ap_dhcp" || scenario.name == "ap_dhcp_companion" {
        let delay_ms =
            env::var("ESP8266_HAL_AP_DELAY_BEFORE_NET_MS").unwrap_or_else(|_| "5000".to_owned());
        let dhcp_grace = env::var("ESP8266_HAL_AP_DHCP_GRACE_MS").ok();
        let dhcp_poll = env::var("ESP8266_HAL_AP_DHCP_POLL_MS").ok();
        prefix.push_str(
            &format!(
                "ESP8266_HAL_AP_DHCP=1 ESP8266_HAL_AP_DELAY_BEFORE_NET_MS={} ESP8266_HAL_AP_RX_FRAME_BYTES=512 ",
                delay_ms
            ),
        );
        if let Some(dhcp_grace) = dhcp_grace {
            prefix.push_str(&shell_env_assignment(
                "ESP8266_HAL_AP_DHCP_GRACE_MS",
                &dhcp_grace,
            ));
            prefix.push(' ');
        }
        if let Some(dhcp_poll) = dhcp_poll {
            prefix.push_str(&shell_env_assignment(
                "ESP8266_HAL_AP_DHCP_POLL_MS",
                &dhcp_poll,
            ));
            prefix.push(' ');
        }
    }

    if scenario.name == "ap_sta" || scenario.name == "ap_sta_downstream_companion" {
        prefix.push_str("ESP8266_HAL_AP_STA_WAIT_CLIENT=1 ESP8266_HAL_AP_RX_FRAME_BYTES=640 ");
    }

    if matches!(
        scenario.name,
        "esp_now_unicast"
            | "esp_now_encrypted"
            | "esp_now_unicast_companion"
            | "esp_now_encrypted_companion"
            | "esp_now_bad_key_companion"
    ) {
        let peer = extra_env
            .iter()
            .find_map(|(key, value)| (*key == "ESP8266_HAL_ESPNOW_PEER").then_some(value.clone()))
            .or_else(|| env::var("ESP8266_HAL_ESPNOW_PEER").ok())
            .context("ESP8266_HAL_ESPNOW_PEER=aa:bb:cc:dd:ee:ff is required")?;
        prefix.push_str(&format!("ESP8266_HAL_ESPNOW_PEER={} ", peer));
    }

    if matches!(
        scenario.name,
        "esp_now_encrypted" | "esp_now_encrypted_companion" | "esp_now_bad_key_companion"
    ) {
        prefix.push_str("ESP8266_HAL_ESPNOW_ENCRYPTED=1 ");
    }

    for (key, value) in extra_env {
        prefix.push_str(&shell_env_assignment(key, value));
        prefix.push(' ');
    }

    let RuntimePlan::TcpClient { port, .. } = scenario.runtime else {
        return Ok(prefix);
    };

    if extra_env
        .iter()
        .any(|(key, _)| *key == "ESP8266_HAL_TCP_CLIENT_HOST")
    {
        return Ok(prefix);
    }

    let host = detect_host_ipv4()?;
    fs::write(tcp_client_host_file(run_dir), host.to_string())
        .with_context(|| format!("failed to record TCP client host {}", host))?;
    prefix.push_str(&format!(
        "ESP8266_HAL_TCP_CLIENT_HOST={} ESP8266_HAL_TCP_CLIENT_PORT={} ",
        host, port
    ));
    Ok(prefix)
}

fn shell_env_assignment(key: &str, value: &str) -> String {
    let escaped = value.replace('\'', "'\"'\"'");
    format!("{key}='{escaped}'")
}

fn elf2image(run_dir: &Path, example: &str, scenario: &Scenario) -> Result<Vec<FlashImage>> {
    let log_path = run_dir.join("elf2image.log");
    let mut log = File::create(&log_path)
        .with_context(|| format!("failed to create {}", log_path.display()))?;

    let repo = repo_root()?;
    let example_path = repo
        .join("target")
        .join(TARGET)
        .join(scenario.profile)
        .join("examples")
        .join(example);
    let esptool = esptool_command();
    let out_prefix = run_dir.join(format!("{example}.current.bin"));
    let args = elf2image_args(&out_prefix, &example_path);

    let mut command = Command::new(&esptool);
    command
        .args(&args)
        .current_dir(&repo)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    writeln!(
        log,
        "$ {} {}",
        esptool.to_string_lossy(),
        args.iter()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
    )?;

    let output = command.output().context("failed to invoke elf2image")?;
    log.write_all(&output.stdout)?;
    log.write_all(&output.stderr)?;

    if !output.status.success() {
        bail!("elf2image failed; see {}", log_path.display());
    }

    let images = discover_flash_images(&out_prefix)?;
    if images.len() != 2 || images.first().map(|image| image.address) != Some(0) {
        bail!(
            "elf2image did not produce the expected split v1 images in {}",
            run_dir.display()
        );
    }
    Ok(images)
}

fn discover_flash_images(out_prefix: &Path) -> Result<Vec<FlashImage>> {
    let parent = out_prefix
        .parent()
        .ok_or_else(|| anyhow!("elf2image output prefix has no parent"))?;
    let prefix = out_prefix
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("elf2image output prefix is not valid UTF-8"))?;
    let mut images = Vec::new();

    for entry in
        fs::read_dir(parent).with_context(|| format!("failed to list {}", parent.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(address) = flash_image_address(prefix, name) else {
            continue;
        };
        images.push(FlashImage { address, path });
    }

    images.sort_by_key(|image| image.address);
    Ok(images)
}

fn flash_image_address(prefix: &str, name: &str) -> Option<u32> {
    let rest = name.strip_prefix(prefix)?;
    let hex = rest.strip_prefix("0x")?.strip_suffix(".bin")?;
    u32::from_str_radix(hex, 16).ok()
}

fn env_flag(name: &str) -> bool {
    env::var(name)
        .map(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

fn flash_skip_status_from_value(value: Option<&str>) -> &'static str {
    match value {
        Some("1" | "true" | "yes" | "on") => "enabled",
        Some(_) => "disabled",
        None => "disabled",
    }
}

fn flash_skip_status() -> &'static str {
    flash_skip_status_from_value(env::var(SKIP_UNCHANGED_FLASH_ENV).ok().as_deref())
}

fn path_presence(path: &Path) -> &'static str {
    if path.exists() {
        "present"
    } else {
        "missing"
    }
}

fn skip_unchanged_flash_enabled() -> bool {
    env_flag(SKIP_UNCHANGED_FLASH_ENV)
}

fn flash_images_fingerprint(images: &[FlashImage]) -> Result<String> {
    let mut fingerprint = FlashFingerprint::new();
    let mut buffer = [0_u8; 8192];

    for image in images {
        fingerprint.update(&image.address.to_le_bytes());

        let metadata = fs::metadata(&image.path)
            .with_context(|| format!("failed to stat flash image {}", image.path.display()))?;
        fingerprint.update(&metadata.len().to_le_bytes());

        let mut file = File::open(&image.path)
            .with_context(|| format!("failed to open flash image {}", image.path.display()))?;
        loop {
            let read = file
                .read(&mut buffer)
                .with_context(|| format!("failed to read flash image {}", image.path.display()))?;
            if read == 0 {
                break;
            }
            fingerprint.update(&buffer[..read]);
        }
    }

    Ok(fingerprint.finish())
}

fn flash_cache_key(serial_port: &Path) -> String {
    let key: String = serial_port
        .to_string_lossy()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();

    if key.is_empty() {
        "serial".to_owned()
    } else {
        key
    }
}

fn flash_cache_path(serial_port: &Path) -> Result<PathBuf> {
    Ok(repo_root()?
        .join("target")
        .join("wifi-hw-flash-cache")
        .join(format!("{}.fingerprint", flash_cache_key(serial_port))))
}

fn read_flash_cache(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(value) => Ok(Some(value.trim().to_owned())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn write_flash_cache(path: &Path, fingerprint: &str) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("flash cache path has no parent"))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create flash cache dir {}", parent.display()))?;
    fs::write(path, format!("{fingerprint}\n"))
        .with_context(|| format!("failed to write {}", path.display()))
}

fn flash_images(run_dir: &Path, images: &[FlashImage], serial_port: &Path) -> Result<()> {
    let log_path = run_dir.join("flash.log");
    let mut log = File::create(&log_path)
        .with_context(|| format!("failed to create {}", log_path.display()))?;

    let fingerprint = flash_images_fingerprint(images)?;
    let skip_enabled = skip_unchanged_flash_enabled();
    let cache_path = flash_cache_path(serial_port)?;
    writeln!(log, "image_fingerprint={fingerprint}")?;
    writeln!(log, "skip_unchanged_flash={skip_enabled}")?;
    writeln!(log, "cache_path={}", cache_path.display())?;
    append_manifest_line(run_dir, "flash_image_fingerprint", &fingerprint)?;

    if skip_enabled && read_flash_cache(&cache_path)?.as_deref() == Some(fingerprint.as_str()) {
        writeln!(log, "flash_action=skipped-unchanged")?;
        append_manifest_line(run_dir, "flash_action", "skipped-unchanged")?;
        eprintln!(
            "wifi-hw: skipping unchanged flash on {} ({fingerprint})",
            serial_port.display()
        );
        return Ok(());
    }

    let esptool = esptool_command();
    let mut command = Command::new(&esptool);
    command
        .arg("--chip")
        .arg("esp8266")
        .arg("--port")
        .arg(serial_port)
        .arg("--baud")
        .arg(DEFAULT_FLASH_BAUD.to_string())
        .arg("write-flash");

    let mut log_args = Vec::new();
    for image in images {
        let address = format!("0x{:05x}", image.address);
        command.arg(&address).arg(&image.path);
        log_args.push(address);
        log_args.push(image.path.display().to_string());
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());

    writeln!(
        log,
        "$ {} --chip esp8266 --port {} --baud {} write-flash {}",
        esptool.to_string_lossy(),
        serial_port.display(),
        DEFAULT_FLASH_BAUD,
        log_args.join(" ")
    )?;

    let output = command.output().context("failed to flash images")?;
    log.write_all(&output.stdout)?;
    log.write_all(&output.stderr)?;

    if !output.status.success() {
        bail!("flash failed; see {}", log_path.display());
    }

    if skip_enabled {
        write_flash_cache(&cache_path, &fingerprint)?;
    }
    writeln!(log, "flash_action=written")?;
    append_manifest_line(run_dir, "flash_action", "written")?;

    Ok(())
}

fn watch_uart(
    run_dir: &Path,
    serial_port: &Path,
    timeout: Duration,
    success_markers: &[&str],
    failure_markers: &[&str],
) -> Result<()> {
    let mut port = open_serial_port(serial_port)?;
    pulse_reset(port.as_mut())?;

    let log_path = run_dir.join("uart.log");
    let mut uart_log = File::create(&log_path)
        .with_context(|| format!("failed to create {}", log_path.display()))?;
    let start = Instant::now();
    let mut buffer = Vec::with_capacity(32 * 1024);

    while start.elapsed() < timeout {
        let mut chunk = [0_u8; 4096];
        match port.read(&mut chunk) {
            Ok(read) if read > 0 => {
                uart_log.write_all(&chunk[..read])?;
                buffer.extend_from_slice(&chunk[..read]);
                if buffer.len() > 32 * 1024 {
                    let excess = buffer.len() - 32 * 1024;
                    buffer.drain(0..excess);
                }

                let text = String::from_utf8_lossy(&buffer);
                if success_markers.iter().any(|marker| text.contains(marker)) {
                    return Ok(());
                }
                if let Some(marker) = failure_markers
                    .iter()
                    .find(|marker| text.contains(**marker))
                {
                    bail!(
                        "uart reported failure marker `{marker}`; see {}",
                        log_path.display()
                    );
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error).context("failed while reading UART"),
        }
    }

    bail!(
        "timed out waiting for UART success markers; see {}",
        log_path.display()
    )
}

fn watch_tcp_echo(
    run_dir: &Path,
    serial_port: &Path,
    timeout: Duration,
    port_number: u16,
    dhcp_regex: &str,
    listen_marker: &str,
    failure_markers: &[&str],
    payload: &[u8],
) -> Result<()> {
    let mut port = open_serial_port(serial_port)?;
    pulse_reset(port.as_mut())?;

    let uart_path = run_dir.join("uart.log");
    let mut uart_log = File::create(&uart_path)
        .with_context(|| format!("failed to create {}", uart_path.display()))?;
    let conn_path = run_dir.join("connect.log");
    let mut conn_log = File::create(&conn_path)
        .with_context(|| format!("failed to create {}", conn_path.display()))?;

    let dhcp_re = Regex::new(dhcp_regex).context("invalid DHCP regex")?;
    let start = Instant::now();
    let mut buffer = Vec::with_capacity(32 * 1024);
    let mut ip_address = None;
    let mut armed = false;

    while start.elapsed() < timeout {
        let mut chunk = [0_u8; 4096];
        match port.read(&mut chunk) {
            Ok(read) if read > 0 => {
                uart_log.write_all(&chunk[..read])?;
                buffer.extend_from_slice(&chunk[..read]);
                if buffer.len() > 32 * 1024 {
                    let excess = buffer.len() - 32 * 1024;
                    buffer.drain(0..excess);
                }

                if ip_address.is_none() {
                    if let Some(captures) = dhcp_re.captures(&buffer) {
                        let ip = captures
                            .get(1)
                            .map(|match_| String::from_utf8_lossy(match_.as_bytes()).into_owned())
                            .ok_or_else(|| anyhow!("DHCP regex missing address capture"))?;
                        writeln!(
                            conn_log,
                            "{:.3}:uart:dhcp_ip:{ip}",
                            start.elapsed().as_secs_f32()
                        )?;
                        ip_address = Some(ip);
                    }
                }

                if !armed && String::from_utf8_lossy(&buffer).contains(listen_marker) {
                    writeln!(
                        conn_log,
                        "{:.3}:uart:{listen_marker}",
                        start.elapsed().as_secs_f32()
                    )?;
                    armed = true;
                }

                if armed {
                    if let Some(ip) = ip_address.as_deref() {
                        run_tcp_echo_probe(&mut conn_log, start, ip, port_number, payload)?;
                        return Ok(());
                    }
                }

                let text = String::from_utf8_lossy(&buffer);
                if let Some(marker) = failure_markers
                    .iter()
                    .find(|marker| text.contains(**marker))
                {
                    bail!(
                        "uart reported failure marker `{marker}` while waiting for TCP echo; see {}",
                        uart_path.display()
                    );
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error).context("failed while reading UART"),
        }
    }

    bail!(
        "timed out waiting for TCP echo readiness; see {}",
        conn_path.display()
    )
}

fn watch_companion_tcp_echo(
    run_dir: &Path,
    serial_port: &Path,
    companion: &mut CompanionApSession,
    timeout: Duration,
    port_number: u16,
    dhcp_regex: &str,
    listen_marker: &str,
    failure_markers: &[&str],
    payload: &[u8],
) -> Result<()> {
    let mut port = open_serial_port(serial_port)?;
    pulse_reset(port.as_mut())?;

    let uart_path = run_dir.join("uart.log");
    let mut uart_log = File::create(&uart_path)
        .with_context(|| format!("failed to create {}", uart_path.display()))?;
    let conn_path = run_dir.join("connect.log");
    let mut conn_log = File::create(&conn_path)
        .with_context(|| format!("failed to create {}", conn_path.display()))?;

    let dhcp_re = Regex::new(dhcp_regex).context("invalid DHCP regex")?;
    let start = Instant::now();
    let mut buffer = Vec::with_capacity(32 * 1024);
    let mut ip_address = None;
    let mut armed = false;

    while start.elapsed() < timeout {
        let mut chunk = [0_u8; 4096];
        match port.read(&mut chunk) {
            Ok(read) if read > 0 => {
                uart_log.write_all(&chunk[..read])?;
                buffer.extend_from_slice(&chunk[..read]);
                if buffer.len() > 32 * 1024 {
                    let excess = buffer.len() - 32 * 1024;
                    buffer.drain(0..excess);
                }

                if ip_address.is_none() {
                    if let Some(captures) = dhcp_re.captures(&buffer) {
                        let ip = captures
                            .get(1)
                            .map(|match_| String::from_utf8_lossy(match_.as_bytes()).into_owned())
                            .ok_or_else(|| anyhow!("DHCP regex missing address capture"))?;
                        writeln!(
                            conn_log,
                            "{:.3}:uart:dhcp_ip:{ip}",
                            start.elapsed().as_secs_f32()
                        )?;
                        ip_address = Some(ip);
                    }
                }

                if !armed && String::from_utf8_lossy(&buffer).contains(listen_marker) {
                    writeln!(
                        conn_log,
                        "{:.3}:uart:{listen_marker}",
                        start.elapsed().as_secs_f32()
                    )?;
                    armed = true;
                }

                if armed {
                    if let Some(ip) = ip_address.as_deref() {
                        if let Err(error) = run_companion_tcp_connect_probe(
                            &mut conn_log,
                            companion,
                            start,
                            ip,
                            port_number,
                            payload,
                        ) {
                            drain_uart_to_log(port.as_mut(), &mut uart_log, Duration::from_secs(1))
                                .context("failed to drain UART after companion TCP probe")?;
                            return Err(error);
                        }
                        return Ok(());
                    }
                }

                let text = String::from_utf8_lossy(&buffer);
                if let Some(marker) = failure_markers
                    .iter()
                    .find(|marker| text.contains(**marker))
                {
                    bail!(
                        "uart reported failure marker `{marker}` while waiting for companion TCP echo; see {}",
                        uart_path.display()
                    );
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error).context("failed while reading UART"),
        }
    }

    bail!(
        "timed out waiting for companion TCP echo readiness; see {}",
        conn_path.display()
    )
}

fn drain_uart_to_log(
    port: &mut dyn serialport::SerialPort,
    uart_log: &mut File,
    duration: Duration,
) -> Result<()> {
    let deadline = Instant::now() + duration;
    let mut chunk = [0_u8; 4096];
    while Instant::now() < deadline {
        match port.read(&mut chunk) {
            Ok(read) if read > 0 => uart_log.write_all(&chunk[..read])?,
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error).context("failed while draining UART"),
        }
    }
    Ok(())
}

fn watch_tcp_client(
    run_dir: &Path,
    serial_port: &Path,
    timeout: Duration,
    port_number: u16,
    success_markers: &[&str],
    failure_markers: &[&str],
    response: &[u8],
) -> Result<()> {
    let host = read_tcp_client_host(run_dir)?;
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port_number))
        .with_context(|| format!("failed to listen for TCP client on port {port_number}"))?;
    listener
        .set_nonblocking(true)
        .context("failed to configure TCP client listener as nonblocking")?;

    let mut port = open_serial_port(serial_port)?;
    pulse_reset(port.as_mut())?;

    let uart_path = run_dir.join("uart.log");
    let mut uart_log = File::create(&uart_path)
        .with_context(|| format!("failed to create {}", uart_path.display()))?;
    let conn_path = run_dir.join("connect.log");
    let mut conn_log = File::create(&conn_path)
        .with_context(|| format!("failed to create {}", conn_path.display()))?;
    writeln!(conn_log, "host={host}")?;
    writeln!(conn_log, "listen=0.0.0.0:{port_number}")?;

    let start = Instant::now();
    let mut buffer = Vec::with_capacity(32 * 1024);
    let mut served = false;

    while start.elapsed() < timeout {
        let mut chunk = [0_u8; 4096];
        match port.read(&mut chunk) {
            Ok(read) if read > 0 => {
                uart_log.write_all(&chunk[..read])?;
                buffer.extend_from_slice(&chunk[..read]);
                if buffer.len() > 32 * 1024 {
                    let excess = buffer.len() - 32 * 1024;
                    buffer.drain(0..excess);
                }

                let text = String::from_utf8_lossy(&buffer);
                if let Some(marker) = failure_markers
                    .iter()
                    .find(|marker| text.contains(**marker))
                {
                    bail!(
                        "uart reported failure marker `{marker}`; see {}",
                        uart_path.display()
                    );
                }
                if served && success_markers.iter().all(|marker| text.contains(*marker)) {
                    return Ok(());
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error).context("failed while reading UART"),
        }

        if !served {
            match listener.accept() {
                Ok((mut stream, peer)) => {
                    writeln!(
                        conn_log,
                        "{:.3}:accepted:{peer}",
                        start.elapsed().as_secs_f32()
                    )?;
                    serve_tcp_client(&mut conn_log, start, &mut stream, response)?;
                    served = true;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error).context("failed while accepting TCP client"),
            }
        }
    }

    bail!(
        "timed out waiting for TCP client exchange; see {}",
        conn_path.display()
    )
}

fn watch_companion_tcp_client(
    run_dir: &Path,
    serial_port: &Path,
    companion: &mut CompanionApSession,
    timeout: Duration,
    port_number: u16,
    success_markers: &[&str],
    failure_markers: &[&str],
    response: &[u8],
) -> Result<()> {
    let conn_path = run_dir.join("connect.log");
    let mut conn_log = File::create(&conn_path)
        .with_context(|| format!("failed to create {}", conn_path.display()))?;
    writeln!(conn_log, "host=192.168.4.1")?;
    writeln!(conn_log, "companion-listen=192.168.4.1:{port_number}")?;

    let response_hex = hex_encode(response);
    let command = format!("TCP_SERVE_ONCE port={port_number} response={response_hex}");
    let server_response = companion_session_command(
        &mut companion.log,
        &mut companion.commands,
        &command,
        Duration::from_secs(5),
    )?;
    ensure_ok_response(&server_response, "TCP_SERVE_ONCE")?;
    writeln!(conn_log, "0.000:companion-server:start")?;

    let mut port = open_serial_port(serial_port)?;
    pulse_reset(port.as_mut())?;

    let uart_path = run_dir.join("uart.log");
    let mut uart_log = File::create(&uart_path)
        .with_context(|| format!("failed to create {}", uart_path.display()))?;

    let start = Instant::now();
    let mut buffer = Vec::with_capacity(32 * 1024);

    while start.elapsed() < timeout {
        let mut chunk = [0_u8; 4096];
        match port.read(&mut chunk) {
            Ok(read) if read > 0 => {
                uart_log.write_all(&chunk[..read])?;
                buffer.extend_from_slice(&chunk[..read]);
                if buffer.len() > 32 * 1024 {
                    let excess = buffer.len() - 32 * 1024;
                    buffer.drain(0..excess);
                }

                let text = String::from_utf8_lossy(&buffer);
                if let Some(marker) = failure_markers
                    .iter()
                    .find(|marker| text.contains(**marker))
                {
                    bail!(
                        "uart reported failure marker `{marker}`; see {}",
                        uart_path.display()
                    );
                }
                if success_markers.iter().all(|marker| text.contains(*marker)) {
                    return Ok(());
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error).context("failed while reading UART"),
        }
    }

    bail!(
        "timed out waiting for companion TCP client exchange; see {}",
        conn_path.display()
    )
}

fn serve_tcp_client(
    conn_log: &mut File,
    start: Instant,
    stream: &mut TcpStream,
    response: &[u8],
) -> Result<()> {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .context("failed to set TCP client read timeout")?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .context("failed to set TCP client write timeout")?;

    let mut request = [0_u8; 256];
    let read = stream
        .read(&mut request)
        .context("failed to read TCP client request")?;
    writeln!(
        conn_log,
        "{:.3}:request:{:?}",
        start.elapsed().as_secs_f32(),
        &request[..read]
    )?;
    if read == 0 {
        bail!("TCP client closed before sending a request");
    }

    stream
        .write_all(response)
        .with_context(|| format!("failed to write {} response bytes", response.len()))?;
    stream
        .flush()
        .context("failed to flush TCP client response")?;
    writeln!(
        conn_log,
        "{:.3}:response:{}",
        start.elapsed().as_secs_f32(),
        response.len()
    )?;
    stream.shutdown(std::net::Shutdown::Both).ok();
    Ok(())
}

fn run_tcp_echo_probe(
    conn_log: &mut File,
    start: Instant,
    ip: &str,
    port_number: u16,
    payload: &[u8],
) -> Result<()> {
    writeln!(
        conn_log,
        "{:.3}:connect:start:{}",
        start.elapsed().as_secs_f32(),
        ip
    )?;
    let mut stream = TcpStream::connect_timeout(
        &format!("{ip}:{port_number}")
            .parse()
            .with_context(|| format!("invalid socket address for {ip}:{port_number}"))?,
        Duration::from_secs(2),
    )
    .with_context(|| format!("failed to connect to {ip}:{port_number}"))?;
    stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .context("failed to set TCP read timeout")?;
    stream
        .set_write_timeout(Some(Duration::from_millis(500)))
        .context("failed to set TCP write timeout")?;

    writeln!(conn_log, "{:.3}:connected", start.elapsed().as_secs_f32())?;
    stream
        .write_all(payload)
        .with_context(|| format!("failed to send {} bytes", payload.len()))?;
    writeln!(
        conn_log,
        "{:.3}:sendall:{}",
        start.elapsed().as_secs_f32(),
        payload.len()
    )?;

    let mut recv = [0_u8; 64];
    let read = stream
        .read(&mut recv)
        .context("failed to read echo payload")?;
    writeln!(
        conn_log,
        "{:.3}:recv:{:?}",
        start.elapsed().as_secs_f32(),
        &recv[..read]
    )?;

    if &recv[..read] != payload {
        bail!(
            "echo mismatch: sent {:?}, received {:?}",
            payload,
            &recv[..read]
        );
    }

    stream.shutdown(std::net::Shutdown::Both).ok();
    writeln!(conn_log, "{:.3}:closed", start.elapsed().as_secs_f32())?;
    Ok(())
}

fn run_companion_tcp_connect_probe(
    conn_log: &mut File,
    companion: &mut CompanionApSession,
    start: Instant,
    ip: &str,
    port_number: u16,
    payload: &[u8],
) -> Result<()> {
    let payload_hex = hex_encode(payload);
    writeln!(
        conn_log,
        "{:.3}:companion-connect:start:{}",
        start.elapsed().as_secs_f32(),
        ip
    )?;
    let command = format!(
        "TCP_CONNECT host={ip} port={port_number} payload={payload_hex} iface=ap timeout_ms={COMPANION_TCP_CONNECT_TIMEOUT_MS}"
    );
    let response = companion_session_command(
        &mut companion.log,
        &mut companion.commands,
        &command,
        Duration::from_secs(22),
    )?;
    let ok = ensure_ok_response(&response, "TCP_CONNECT")?;
    let received = response_field(ok, "recv")
        .ok_or_else(|| anyhow!("companion TCP response missing recv field"))?;
    if received != payload_hex {
        bail!("companion TCP echo mismatch: sent {payload_hex}, received {received}");
    }
    writeln!(
        conn_log,
        "{:.3}:companion-connect:ok",
        start.elapsed().as_secs_f32()
    )?;
    Ok(())
}

fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    output
}

fn tcp_client_host_file(run_dir: &Path) -> PathBuf {
    run_dir.join("tcp-client-host.txt")
}

fn static_ipv4_file(run_dir: &Path) -> PathBuf {
    run_dir.join("static-ipv4.txt")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StaticIpv4BuildConfig {
    address: Ipv4Addr,
    prefix_len: u8,
    gateway: Ipv4Addr,
}

fn read_tcp_client_host(run_dir: &Path) -> Result<Ipv4Addr> {
    let path = tcp_client_host_file(run_dir);
    let text = fs::read_to_string(&path)
        .with_context(|| format!("failed to read TCP client host from {}", path.display()))?;
    text.trim()
        .parse()
        .with_context(|| format!("invalid TCP client host in {}", path.display()))
}

fn detect_host_ipv4() -> Result<Ipv4Addr> {
    if let Some(value) = env::var_os("ESP8266_HAL_TCP_CLIENT_HOST") {
        let text = value
            .to_str()
            .ok_or_else(|| anyhow!("ESP8266_HAL_TCP_CLIENT_HOST is not valid UTF-8"))?;
        return text
            .parse()
            .with_context(|| format!("invalid ESP8266_HAL_TCP_CLIENT_HOST `{text}`"));
    }

    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))
        .context("failed to create UDP socket for host IPv4 detection")?;
    socket
        .connect(SocketAddrV4::new(Ipv4Addr::new(1, 1, 1, 1), 80))
        .context("failed to detect default-route IPv4; set ESP8266_HAL_TCP_CLIENT_HOST")?;
    match socket
        .local_addr()
        .context("failed to read default-route local address")?
        .ip()
    {
        IpAddr::V4(address) => Ok(address),
        IpAddr::V6(address) => bail!(
            "default route selected IPv6 address {address}; set ESP8266_HAL_TCP_CLIENT_HOST to an IPv4 address"
        ),
    }
}

fn detect_static_ipv4_config() -> Result<StaticIpv4BuildConfig> {
    let prefix_len = match env::var("ESP8266_HAL_STATIC_PREFIX") {
        Ok(value) => value
            .parse::<u8>()
            .with_context(|| format!("invalid ESP8266_HAL_STATIC_PREFIX `{value}`"))?,
        Err(env::VarError::NotPresent) => 24,
        Err(error) => return Err(error).context("failed to read ESP8266_HAL_STATIC_PREFIX"),
    };
    if prefix_len > 32 {
        bail!("ESP8266_HAL_STATIC_PREFIX must be <= 32");
    }

    let host = detect_host_ipv4()?;
    let gateway = match env::var("ESP8266_HAL_STATIC_GATEWAY") {
        Ok(value) => value
            .parse::<Ipv4Addr>()
            .with_context(|| format!("invalid ESP8266_HAL_STATIC_GATEWAY `{value}`"))?,
        Err(env::VarError::NotPresent) => {
            let mut octets = host.octets();
            octets[3] = 1;
            Ipv4Addr::from(octets)
        }
        Err(error) => return Err(error).context("failed to read ESP8266_HAL_STATIC_GATEWAY"),
    };
    let address = match env::var("ESP8266_HAL_STATIC_IPV4") {
        Ok(value) => value
            .parse::<Ipv4Addr>()
            .with_context(|| format!("invalid ESP8266_HAL_STATIC_IPV4 `{value}`"))?,
        Err(env::VarError::NotPresent) => {
            let mut octets = host.octets();
            octets[3] = if octets[3] == 116 { 117 } else { 116 };
            Ipv4Addr::from(octets)
        }
        Err(error) => return Err(error).context("failed to read ESP8266_HAL_STATIC_IPV4"),
    };

    Ok(StaticIpv4BuildConfig {
        address,
        prefix_len,
        gateway,
    })
}

fn open_serial_port(path: &Path) -> Result<Box<dyn serialport::SerialPort>> {
    let builder = serialport::new(path.to_string_lossy(), DEFAULT_UART_BAUD)
        .timeout(Duration::from_millis(50));
    let mut port = builder
        .open()
        .with_context(|| format!("failed to open serial port {}", path.display()))?;
    port.write_data_terminal_ready(false)
        .context("failed to clear DTR")?;
    port.write_request_to_send(false)
        .context("failed to clear RTS")?;
    thread::sleep(Duration::from_millis(50));
    Ok(port)
}

fn drain_uart_for_failure(
    port: &mut dyn serialport::SerialPort,
    uart_log: &mut File,
    buffer: &mut Vec<u8>,
    duration: Duration,
) -> Result<()> {
    let start = Instant::now();
    while start.elapsed() < duration {
        let mut chunk = [0_u8; 4096];
        match port.read(&mut chunk) {
            Ok(read) if read > 0 => {
                uart_log.write_all(&chunk[..read])?;
                buffer.extend_from_slice(&chunk[..read]);
                if buffer.len() > 32 * 1024 {
                    let excess = buffer.len() - 32 * 1024;
                    buffer.drain(0..excess);
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error).context("failed while draining UART after failure"),
        }
    }

    Ok(())
}

fn pulse_reset(port: &mut dyn serialport::SerialPort) -> Result<()> {
    port.clear(serialport::ClearBuffer::All)
        .context("failed to clear serial buffers")?;
    port.write_request_to_send(true)
        .context("failed to assert RTS for reset")?;
    thread::sleep(Duration::from_millis(50));
    port.write_request_to_send(false)
        .context("failed to deassert RTS for reset")?;
    Ok(())
}

fn esptool_command() -> OsString {
    if command_available("esptool").is_ok() {
        OsString::from("esptool")
    } else {
        OsString::from("esptool.py")
    }
}

fn repo_root() -> Result<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("xtask manifest dir has no parent"))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use super::{
        bounded_companion_command_timeout, command_field, flash_cache_key,
        flash_images_fingerprint, flash_skip_status_from_value, focused_run_attempt_limit,
        matrix_secondary_lane_candidate, parse_matrix_options, partition_matrix_lanes,
        path_presence, promisc_raw_kind, scenario_by_name, scenario_matrix_lane_label,
        scenario_resource_labels, scenario_uses_companion_resource, scenario_uses_router,
        wrong_channel_hint, FlashImage, WpsRouterMode, DEFAULT_MATRIX_ATTEMPTS,
    };

    fn unique_test_dir(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "esp8266-hal-wifi-hw-{name}-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("failed to create test dir");
        dir
    }

    #[test]
    fn command_field_extracts_ssid_from_join_command() {
        let command = "STA_JOIN ssid=esp8266-hal-ap pass=open timeout_ms=12000 wait_ip=0";
        assert_eq!(command_field(command, "ssid"), Some("esp8266-hal-ap"));
    }

    #[test]
    fn command_field_ignores_non_key_value_tokens() {
        let command = "STA_JOIN open timeout_ms=12000";
        assert_eq!(command_field(command, "ssid"), None);
        assert_eq!(command_field(command, "timeout_ms"), Some("12000"));
    }

    #[test]
    fn companion_command_timeout_is_capped_by_remaining_budget() {
        let command = "STA_JOIN_KEEP_AP timeout_ms=30000";

        assert_eq!(
            bounded_companion_command_timeout(
                command,
                Duration::from_secs(25),
                Duration::from_secs(3)
            ),
            Some(Duration::from_secs(3))
        );
        assert_eq!(
            bounded_companion_command_timeout(
                command,
                Duration::from_secs(25),
                Duration::from_millis(250)
            ),
            None
        );
    }

    #[test]
    fn promiscuous_scenarios_select_expected_stimulus() {
        assert_eq!(promisc_raw_kind("promisc_capture"), "ap-beacon");
        assert_eq!(promisc_raw_kind("promisc_filters"), "beacon");
        assert_eq!(promisc_raw_kind("promisc_data_companion"), "data");
    }

    #[test]
    fn wps_router_mode_parses_supported_modes() {
        assert_eq!(WpsRouterMode::from_str("pbc").unwrap(), WpsRouterMode::Pbc);
        assert_eq!(WpsRouterMode::from_str("pin").unwrap(), WpsRouterMode::Pin);
        assert!(WpsRouterMode::from_str("push").is_err());
    }

    #[test]
    fn flash_fingerprint_tracks_address_and_contents() {
        let dir = unique_test_dir("flash-fingerprint");
        let image_path = dir.join("image.bin");
        fs::write(&image_path, [1_u8, 2, 3, 4]).expect("failed to write image");

        let image = FlashImage {
            address: 0,
            path: image_path.clone(),
        };
        let fingerprint = flash_images_fingerprint(&[image.clone()]).unwrap();
        assert_eq!(
            fingerprint,
            flash_images_fingerprint(&[image.clone()]).unwrap()
        );

        let moved_image = FlashImage {
            address: 0x10000,
            path: image_path.clone(),
        };
        assert_ne!(
            fingerprint,
            flash_images_fingerprint(&[moved_image]).unwrap()
        );

        fs::write(&image_path, [1_u8, 2, 3, 5]).expect("failed to rewrite image");
        assert_ne!(fingerprint, flash_images_fingerprint(&[image]).unwrap());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn flash_cache_key_is_stable_for_serial_paths() {
        assert_eq!(flash_cache_key(Path::new("/dev/ttyUSB0")), "_dev_ttyUSB0");
        assert_eq!(
            flash_cache_key(Path::new("/dev/serial/by-id/usb chip")),
            "_dev_serial_by-id_usb_chip"
        );
    }

    #[test]
    fn flash_skip_status_accepts_explicit_truthy_values_only() {
        for value in [Some("1"), Some("true"), Some("yes"), Some("on")] {
            assert_eq!(flash_skip_status_from_value(value), "enabled");
        }

        for value in [None, Some(""), Some("0"), Some("false"), Some("off")] {
            assert_eq!(flash_skip_status_from_value(value), "disabled");
        }
    }

    #[test]
    fn path_presence_reports_present_or_missing() {
        let dir = unique_test_dir("path-presence");
        let present = dir.join("present");
        fs::write(&present, b"ok").expect("failed to create present file");

        assert_eq!(path_presence(&present), "present");
        assert_eq!(path_presence(&dir.join("missing")), "missing");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn focused_wps_runs_retry_the_router_armed_scenario() {
        let wps = scenario_by_name("wps").expect("wps scenario");
        let wps_pin = scenario_by_name("wps_pin").expect("wps pin scenario");
        let sta_connect = scenario_by_name("sta_connect").expect("sta connect scenario");

        assert_eq!(focused_run_attempt_limit(wps), DEFAULT_MATRIX_ATTEMPTS);
        assert_eq!(focused_run_attempt_limit(wps_pin), DEFAULT_MATRIX_ATTEMPTS);
        assert_eq!(focused_run_attempt_limit(sta_connect), 1);
    }

    #[test]
    fn matrix_options_parse_parallel_lane_arguments() {
        let options = parse_matrix_options(&[
            "--root".to_owned(),
            "target/custom".to_owned(),
            "--serial-port".to_owned(),
            "/dev/ttyUSB9".to_owned(),
            "--companion-port".to_owned(),
            "/dev/ttyACM9".to_owned(),
            "--jobs".to_owned(),
            "2".to_owned(),
            "--secondary-serial-port".to_owned(),
            "/dev/ttyUSB8".to_owned(),
        ])
        .expect("parallel matrix options should parse");

        assert_eq!(options.root.as_deref(), Some(Path::new("target/custom")));
        assert_eq!(options.serial_port, PathBuf::from("/dev/ttyUSB9"));
        assert_eq!(options.companion_port, PathBuf::from("/dev/ttyACM9"));
        assert_eq!(
            options.secondary_serial_port,
            Some(PathBuf::from("/dev/ttyUSB8"))
        );
        assert_eq!(options.jobs, 2);
        assert!(!options.dry_run);
    }

    #[test]
    fn matrix_options_reject_invalid_parallel_args() {
        assert!(parse_matrix_options(&[
            "--jobs".to_owned(),
            "0".to_owned(),
            "--secondary-serial-port".to_owned(),
            "/dev/ttyUSB1".to_owned(),
        ])
        .is_err());
        assert!(parse_matrix_options(&[
            "--serial-port".to_owned(),
            "/dev/ttyUSB0".to_owned(),
            "--jobs".to_owned(),
            "2".to_owned(),
            "--secondary-serial-port".to_owned(),
            "/dev/ttyUSB0".to_owned(),
        ])
        .is_err());
        assert!(parse_matrix_options(&["sta_connect".to_owned()]).is_err());
    }

    #[test]
    fn matrix_options_parse_dry_run_without_requiring_second_lane() {
        let options = parse_matrix_options(&["--dry-run".to_owned()])
            .expect("single-lane dry-run should parse");

        assert!(options.dry_run);
        assert_eq!(options.jobs, 1);
        assert!(options.secondary_serial_port.is_none());
    }

    #[test]
    fn matrix_options_allow_unused_matching_secondary_port_in_single_lane_mode() {
        let options = parse_matrix_options(&[
            "--serial-port".to_owned(),
            "/dev/ttyUSB0".to_owned(),
            "--secondary-serial-port".to_owned(),
            "/dev/ttyUSB0".to_owned(),
        ])
        .expect("single-lane matrix should ignore an unused secondary serial port");

        assert_eq!(options.jobs, 1);
    }

    #[test]
    fn parallel_matrix_secondary_lane_is_router_enterprise_only() {
        let eap = scenario_by_name("eap_sta_assoc").expect("eap scenario");
        let eap_negative = scenario_by_name("eap_wrong_password").expect("negative eap scenario");
        let wps = scenario_by_name("wps").expect("wps scenario");
        let sta_connect = scenario_by_name("sta_connect").expect("sta connect scenario");

        assert!(matrix_secondary_lane_candidate(eap));
        assert!(matrix_secondary_lane_candidate(eap_negative));
        assert!(!matrix_secondary_lane_candidate(wps));
        assert!(!matrix_secondary_lane_candidate(sta_connect));
    }

    #[test]
    fn parallel_matrix_resource_classification_tracks_shared_fixtures() {
        assert!(scenario_uses_router("eap_sta_assoc"));
        assert!(scenario_uses_router("eap_wrong_password"));
        assert!(scenario_uses_router("wps"));
        assert!(scenario_uses_router("smartconfig"));
        assert!(!scenario_uses_router("sta_connect"));

        assert!(scenario_uses_companion_resource("sta_connect"));
        assert!(scenario_uses_companion_resource("sta_scan_connect"));
        assert!(scenario_uses_companion_resource("sta_wpa2_connect"));
        assert!(scenario_uses_companion_resource("sta_stale_channel_hint"));
        assert!(scenario_uses_companion_resource("sta_hidden_ssid"));
        assert!(scenario_uses_companion_resource("tcp_server"));
        assert!(scenario_uses_companion_resource("smartconfig"));
        assert!(scenario_uses_companion_resource("promisc_data_companion"));
        assert!(!scenario_uses_companion_resource("eap_sta_assoc"));
    }

    #[test]
    fn scenario_audit_labels_match_parallel_matrix_contract() {
        let eap = scenario_by_name("eap_sta_assoc").expect("eap scenario");
        let sta_connect = scenario_by_name("sta_connect").expect("sta connect scenario");
        let esp_now = scenario_by_name("esp_now_unicast").expect("esp-now scenario");

        assert_eq!(scenario_matrix_lane_label(eap), "secondary-candidate");
        assert_eq!(scenario_resource_labels(eap), ["router"]);

        assert_eq!(scenario_matrix_lane_label(sta_connect), "primary");
        assert_eq!(scenario_resource_labels(sta_connect), ["esp32-companion"]);

        assert_eq!(scenario_matrix_lane_label(esp_now), "primary");
        assert_eq!(scenario_resource_labels(esp_now), ["esp32-companion"]);
    }

    #[test]
    fn matrix_lane_partition_matches_audit_labels() {
        let implemented = ["sta_connect", "eap_sta_assoc", "eap_wrong_password", "wps"]
            .map(|name| scenario_by_name(name).expect("scenario"));
        let (primary, secondary) = partition_matrix_lanes(&implemented);

        assert_eq!(
            primary
                .iter()
                .map(|scenario| scenario.name)
                .collect::<Vec<_>>(),
            ["sta_connect", "wps"]
        );
        assert_eq!(
            secondary
                .iter()
                .map(|scenario| scenario.name)
                .collect::<Vec<_>>(),
            ["eap_sta_assoc", "eap_wrong_password"]
        );
    }

    #[test]
    fn wrong_channel_hint_stays_valid_and_different() {
        for channel in 1..=14 {
            let wrong = wrong_channel_hint(channel);
            assert!((1..=14).contains(&wrong));
            assert_ne!(wrong, channel);
        }
    }
}
