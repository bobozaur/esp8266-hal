use std::{
    env,
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, SocketAddrV4, TcpListener, TcpStream, UdpSocket},
    path::{Path, PathBuf},
    process::{Command, Stdio},
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
const DEFAULT_MATRIX_ATTEMPTS: usize = 3;
const MATRIX_RETRY_BACKOFF: Duration = Duration::from_secs(2);
const COMPANION_TCP_CONNECT_TIMEOUT_MS: u32 = 20_000;
const RIG_BLOCKED_PROMISC_CONTROL: &str =
    "ESP32 ESP-IDF raw TX cannot deterministically inject 802.11 control frames";
const RIG_BLOCKED_SMARTCONFIG: &str =
    "current ESP32 companion cannot emulate ESPTouch phone/STA-through-AP provisioning";
const RIG_BLOCKED_WPS: &str =
    "ESP-IDF AP registrar PBC/PIN attempts fail without sniffer evidence to identify the failing side";

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
    Pending,
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
            success_markers: &["wifi connected", "wifi:connected", "connected"],
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
        name: "ap_station_list_companion",
        description: "join the soft-AP with the ESP32 companion and validate station list/deauth",
        example: Some("wifi_ap"),
        features: &["interrupt", "rt", "wifi-ap"],
        profile: "release",
        state: ScenarioState::Manual,
        runtime: RuntimePlan::Pending,
    },
    Scenario {
        name: "ap_attach_companion",
        description: "join the soft-AP after attaching the AP RX path without embassy-net",
        example: Some("wifi_ap"),
        features: &["interrupt", "rt", "wifi-ap"],
        profile: "release",
        state: ScenarioState::Manual,
        runtime: RuntimePlan::Pending,
    },
    Scenario {
        name: "ap_net_attach_companion",
        description: "join the soft-AP after creating the AP network runtime but before entering the DHCP loop",
        example: Some("wifi_ap"),
        features: &["interrupt", "rt", "wifi-ap"],
        profile: "release",
        state: ScenarioState::Manual,
        runtime: RuntimePlan::Pending,
    },
    Scenario {
        name: "ap_dhcp_bind_companion",
        description: "join the soft-AP after binding the Rust DHCP socket but before entering the DHCP poll loop",
        example: Some("wifi_ap"),
        features: &["interrupt", "rt", "wifi-ap"],
        profile: "release",
        state: ScenarioState::Manual,
        runtime: RuntimePlan::Pending,
    },
    Scenario {
        name: "ap_dhcp_companion",
        description: "join the soft-AP with the ESP32 companion and validate Rust DHCP service",
        example: Some("wifi_ap"),
        features: &["interrupt", "rt", "wifi-ap"],
        profile: "release",
        state: ScenarioState::Manual,
        runtime: RuntimePlan::Pending,
    },
    Scenario {
        name: "ap_sta_downstream_companion",
        description: "validate AP+STA mixed mode with the ESP32 companion as downstream client",
        example: Some("wifi_ap_sta"),
        features: &["interrupt", "rt", "wifi-ap", "wifi-sta", "wifi-embassy-net"],
        profile: "release",
        state: ScenarioState::Manual,
        runtime: RuntimePlan::Pending,
    },
    Scenario {
        name: "esp_now_unicast_companion",
        description: "exchange ESP-NOW unicast payloads with the ESP32 companion",
        example: Some("wifi_esp_now_rx"),
        features: &["interrupt", "rt", "wifi-sta", "wifi-esp-now"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::Pending,
    },
    Scenario {
        name: "esp_now_encrypted_companion",
        description: "exchange encrypted ESP-NOW payloads with the ESP32 companion",
        example: Some("wifi_esp_now_rx"),
        features: &["interrupt", "rt", "wifi-sta", "wifi-esp-now"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::Pending,
    },
    Scenario {
        name: "esp_now_bad_key_companion",
        description: "validate encrypted ESP-NOW failure cases with the ESP32 companion",
        example: Some("wifi_esp_now_rx"),
        features: &["interrupt", "rt", "wifi-sta", "wifi-esp-now"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::Pending,
    },
    Scenario {
        name: "promisc_data_companion",
        description: "validate promiscuous data capture with repeatable ESP32 companion traffic",
        example: Some("wifi_promisc"),
        features: &["interrupt", "rt", "wifi-sta", "wifi-promiscuous"],
        profile: "release",
        state: ScenarioState::Implemented,
        runtime: RuntimePlan::Pending,
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
        description: "complete WPS enrollment on supported router hardware",
        example: Some("wifi_wps"),
        features: &["interrupt", "rt", "wifi-sta", "wifi-wps"],
        profile: "release",
        state: ScenarioState::RigBlocked {
            reason: RIG_BLOCKED_WPS,
        },
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
        description: "complete SmartConfig provisioning from a phone app",
        example: Some("wifi_smartconfig"),
        features: &["interrupt", "rt", "wifi-sta", "wifi-smartconfig"],
        profile: "release",
        state: ScenarioState::RigBlocked {
            reason: RIG_BLOCKED_SMARTCONFIG,
        },
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
        description: "validate promiscuous control-subtype filters with RF traffic",
        example: Some("wifi_promisc"),
        features: &["interrupt", "rt", "wifi-sta", "wifi-promiscuous"],
        profile: "release",
        state: ScenarioState::RigBlocked {
            reason: RIG_BLOCKED_PROMISC_CONTROL,
        },
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

pub const fn matrix_attempt_limit() -> usize {
    DEFAULT_MATRIX_ATTEMPTS
}

pub fn artifacts_root(explicit: Option<&Path>) -> PathBuf {
    explicit
        .map(Path::to_path_buf)
        .or_else(|| env::var_os("ESP8266_HAL_WIFI_HW_ROOT").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("/tmp/esp8266-hal-wifi-hw"))
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
            let options = parse_run_options(rest, false)?;
            run_matrix(
                options.root.as_deref(),
                &options.serial_port,
                &options.companion_port,
            )
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
    println!("  matrix [--root PATH] [--serial-port PATH] [--companion-port PATH]");
    println!("  companion <doctor|setup|build|flash|command>");
}

fn list(format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Text => {
            for scenario in WIFI_SCENARIOS {
                println!(
                    "{:<28} {:<16} example={} features={}{}",
                    scenario.name,
                    scenario.state.label(),
                    scenario.example.unwrap_or("-"),
                    scenario.features.join(","),
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
                println!(
                    "  {{\"name\":\"{}\",\"state\":\"{}\",\"example\":{},\"features\":[{}],\"reason\":{}}}{}",
                    scenario.name,
                    scenario.state.label(),
                    scenario
                        .example
                        .map(|value| format!("\"{value}\""))
                        .unwrap_or_else(|| "null".to_owned()),
                    scenario
                        .features
                        .iter()
                        .map(|feature| format!("\"{feature}\""))
                        .collect::<Vec<_>>()
                        .join(","),
                    scenario
                        .state
                        .rig_blocked_reason()
                        .map(|reason| format!("\"{}\"", json_escape(reason)))
                        .unwrap_or_else(|| "null".to_owned()),
                    comma
                );
            }
            println!("]");
        }
    }

    Ok(())
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
        println!("serial default: {DEFAULT_SERIAL_PORT}");
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

fn run_matrix(root: Option<&Path>, serial_port: &Path, companion_port: &Path) -> Result<()> {
    let root = artifacts_root(root);
    let implemented = WIFI_SCENARIOS
        .iter()
        .filter(|scenario| scenario.state == ScenarioState::Implemented);

    let mut failures = Vec::new();
    for scenario in implemented {
        if let Err(error) =
            run_scenario_with_matrix_retries(scenario, &root, serial_port, companion_port)
        {
            failures.push(format!("{}: {error:#}", scenario.name));
        }
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

    bail!(
        "wifi-hw matrix failed:\n{}",
        failures
            .into_iter()
            .map(|line| format!("- {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

fn run_scenario(
    name: &str,
    root: Option<&Path>,
    serial_port: &Path,
    companion_port: &Path,
) -> Result<()> {
    let scenario = scenario_by_name(name).ok_or_else(|| anyhow!("unknown scenario: {name}"))?;
    run_scenario_with_root(scenario, &artifacts_root(root), serial_port, companion_port)
}

fn run_scenario_with_matrix_retries(
    scenario: &Scenario,
    root: &Path,
    serial_port: &Path,
    companion_port: &Path,
) -> Result<()> {
    let mut failures = Vec::new();

    for attempt in 1..=DEFAULT_MATRIX_ATTEMPTS {
        match run_scenario_with_root(scenario, root, serial_port, companion_port) {
            Ok(()) => return Ok(()),
            Err(error) => {
                failures.push(format!("attempt {attempt}: {error:#}"));
                if attempt < DEFAULT_MATRIX_ATTEMPTS {
                    eprintln!(
                        "wifi-hw matrix: {} failed attempt {attempt}/{DEFAULT_MATRIX_ATTEMPTS}; retrying after {}s",
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
    if let ScenarioState::RigBlocked { reason } = scenario.state {
        bail!(
            "scenario `{}` is blocked by the current hardware rig: {reason}; see research.md",
            scenario.name,
        );
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
            | "wps"
            | "smartconfig"
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

fn promisc_success_marker(name: &str) -> &'static str {
    match name {
        "promisc_filters" => "promisc-management-ok",
        "promisc_control_filters" => "promisc-control-ok",
        _ => "promisc-ok",
    }
}

fn promisc_raw_kind(name: &str) -> &'static str {
    match name {
        "promisc_data_companion" => "data",
        _ => "ap-beacon",
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

fn start_companion_ap_for_scenario(
    run_dir: &Path,
    scenario: &Scenario,
    companion_port: &Path,
) -> Result<Option<CompanionApSession>> {
    if !matches!(
        scenario.name,
        "sta_connect" | "sta_scan" | "sta_dhcp" | "sta_static_ip" | "tcp_server" | "tcp_client"
    ) {
        return Ok(None);
    }

    let mut session = start_companion_ap_session(run_dir, companion_port, true)?;
    if scenario.name == "tcp_client" {
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

    let credentials = companion_ap_credentials()?;
    ensure_companion_command_token("ssid", &credentials.ssid)?;
    ensure_companion_command_token("pass", &credentials.pass)?;
    let channel = companion_ap_channel()?;
    let pass = if credentials.pass.is_empty() {
        "open"
    } else {
        credentials.pass.as_str()
    };
    let response = companion_session_command(
        &mut log,
        &mut commands,
        &format!(
            "AP_START ssid={} pass={} channel={}",
            credentials.ssid, pass, channel
        ),
        Duration::from_secs(10),
    )?;
    ensure_ok_response(&response, "AP_START")?;
    let build_env = vec![
        ("ESP8266_HAL_WIFI_SSID", credentials.ssid),
        ("ESP8266_HAL_WIFI_PASS", credentials.pass),
        ("ESP8266_HAL_WIFI_CHANNEL", channel.to_string()),
    ];

    Ok(CompanionApSession {
        log,
        commands,
        build_env,
    })
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
        "wps" => run_wps_companion(&run_dir, serial_port, &companion),
        "smartconfig" => run_smartconfig_companion(&run_dir, serial_port, &companion),
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

fn run_wps_companion(run_dir: &Path, serial_port: &Path, companion: &CompanionRig) -> Result<()> {
    let mut companion_log = open_companion_log(run_dir)?;
    let mut companion_commands = esp32_companion::CommandSession::open(&companion.port)?;
    ensure_companion_session_ready(&mut companion_log, &mut companion_commands)?;
    let wps_response = companion_session_command(
        &mut companion_log,
        &mut companion_commands,
        "WPS_AP_START ssid=esp8266-hal-wps pass=esp8266wps channel=6 mode=pin pin=12345670",
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

fn run_smartconfig_companion(
    run_dir: &Path,
    serial_port: &Path,
    companion: &CompanionRig,
) -> Result<()> {
    const FAILURE_MARKERS: &[&str] = &[
        "smartconfig sta start err",
        "smartconfig session err",
        "smartconfig start err",
        "smartconfig credentials err",
        "smartconfig configure err",
        "smartconfig connect err",
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
    let timeout = Duration::from_secs(150);
    let mut buffer = Vec::with_capacity(32 * 1024);
    let mut provisioned = false;

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
                if !provisioned && text.contains("smartconfig ready") {
                    let response = companion_command(
                        &mut companion_log,
                        companion,
                        "SMARTCONFIG_AP_PROVISION ssid=esp8266-hal-smart pass=smartpass channel=6 timeout_ms=90000",
                        Duration::from_secs(95),
                    )?;
                    ensure_ok_response(&response, "SMARTCONFIG_AP_PROVISION")?;
                    provisioned = true;
                }
                if text.contains("smartconfig-ok") {
                    return Ok(());
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error).context("failed while reading UART"),
        }
    }

    bail!(
        "timed out waiting for SmartConfig companion flow; see {}",
        uart_path.display()
    )
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
                    let response = companion_session_command(
                        &mut companion_log,
                        &mut companion_commands,
                        &command,
                        Duration::from_secs(15),
                    )?;
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
        "promisc_control_filters" => 64,
        _ => 24,
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
    let passive_ap_beacon = raw_kind == "ap-beacon";
    if passive_ap_beacon {
        let response = companion_command(
            &mut companion_log,
            companion,
            &format!("AP_START ssid=esp8266-hal-promisc pass=open channel={channel}"),
            Duration::from_secs(30),
        )?;
        ensure_ok_response(&response, "AP_START")?;
        thread::sleep(Duration::from_millis(250));
    }

    let mut port = open_serial_port(serial_port)?;
    pulse_reset(port.as_mut())?;

    let start = Instant::now();
    let timeout = Duration::from_secs(45);
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
            let traffic_response = companion_command(
                &mut companion_log,
                companion,
                &format!(
                    "RAW80211_TX kind={} channel={} count={} interval_ms=5{}",
                    raw_kind, channel, raw_count, destination
                ),
                Duration::from_secs(30),
            )?;
            ensure_ok_response(&traffic_response, "RAW80211_TX")?;
            next_traffic = Instant::now() + Duration::from_millis(750);
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
    writeln!(file, "features={}", scenario.features.join(","))?;
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
        prefix.push_str("ESP8266_HAL_PROMISC_FILTER=control ");
    }

    if scenario.name == "promisc_data_companion" {
        prefix.push_str("ESP8266_HAL_PROMISC_FILTER=data ");
    }

    if scenario.name == "wps" {
        prefix.push_str("ESP8266_HAL_WPS_PIN=12345670 ");
    }

    if scenario.name == "ap_station_list" || scenario.name == "ap_station_list_companion" {
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

fn elf2image(run_dir: &Path, example: &str, scenario: &Scenario) -> Result<[PathBuf; 2]> {
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

    let first = PathBuf::from(format!("{}0x00000.bin", out_prefix.display()));
    let second = PathBuf::from(format!("{}0x10000.bin", out_prefix.display()));
    if !first.exists() || !second.exists() {
        bail!(
            "elf2image did not produce split v1 images in {}",
            run_dir.display()
        );
    }
    Ok([first, second])
}

fn flash_images(run_dir: &Path, images: &[PathBuf; 2], serial_port: &Path) -> Result<()> {
    let log_path = run_dir.join("flash.log");
    let mut log = File::create(&log_path)
        .with_context(|| format!("failed to create {}", log_path.display()))?;

    let esptool = esptool_command();
    let mut command = Command::new(&esptool);
    command
        .arg("--chip")
        .arg("esp8266")
        .arg("--port")
        .arg(serial_port)
        .arg("--baud")
        .arg(DEFAULT_FLASH_BAUD.to_string())
        .arg("write-flash")
        .arg("0x00000")
        .arg(&images[0])
        .arg("0x10000")
        .arg(&images[1])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    writeln!(
        log,
        "$ {} --chip esp8266 --port {} --baud {} write-flash 0x00000 {} 0x10000 {}",
        esptool.to_string_lossy(),
        serial_port.display(),
        DEFAULT_FLASH_BAUD,
        images[0].display(),
        images[1].display()
    )?;

    let output = command.output().context("failed to flash images")?;
    log.write_all(&output.stdout)?;
    log.write_all(&output.stderr)?;

    if !output.status.success() {
        bail!("flash failed; see {}", log_path.display());
    }

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
    use std::time::Duration;

    use super::{bounded_companion_command_timeout, command_field, promisc_raw_kind};

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
    fn promiscuous_management_scenarios_use_beacon_stimulus() {
        assert_eq!(promisc_raw_kind("promisc_capture"), "ap-beacon");
        assert_eq!(promisc_raw_kind("promisc_filters"), "ap-beacon");
        assert_eq!(promisc_raw_kind("promisc_data_companion"), "data");
    }
}
