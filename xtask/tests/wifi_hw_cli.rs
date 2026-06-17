use std::path::Path;
use std::time::Duration;

use xtask::wifi_hw::{
    artifacts_root, elf2image_args, matrix_attempt_limit, scenario_by_name, scenario_names,
    RuntimePlan, ScenarioState,
};

#[test]
fn scenario_registry_includes_required_full_matrix_entries() {
    let names = scenario_names();

    for required in [
        "sta_connect",
        "sta_scan",
        "sta_dhcp",
        "sta_static_ip",
        "tcp_server",
        "eap_sta_assoc",
        "ap_start",
        "wps",
        "esp_now_unicast",
        "esp_now_broadcast",
        "esp_now_encrypted",
    ] {
        assert!(
            names.iter().any(|name| *name == required),
            "missing required scenario: {required}"
        );
    }
}

#[test]
fn no_registered_wifi_scenario_is_left_pending() {
    for name in scenario_names() {
        let scenario = scenario_by_name(name).expect("registered scenario");
        assert_ne!(scenario.state, ScenarioState::PendingExample, "{name}");
    }
}

#[test]
fn rig_blocked_scenarios_have_reasons() {
    let mut count = 0;
    for name in scenario_names() {
        let scenario = scenario_by_name(name).expect("registered scenario");
        if let Some(reason) = scenario.state.rig_blocked_reason() {
            count += 1;
            assert_eq!(scenario.state.label(), "rig-blocked");
            assert!(!reason.is_empty(), "{name}");
        }
    }
    assert!(
        count >= 2,
        "expected current rig limitations to be explicit"
    );
}

#[test]
fn promiscuous_control_capture_is_rig_blocked() {
    let scenario =
        scenario_by_name("promisc_control_filters").expect("promiscuous control scenario");

    let ScenarioState::RigBlocked { reason } = scenario.state else {
        panic!("promiscuous control capture should be rig-blocked");
    };
    assert!(reason.contains("control frames"));
    assert_eq!(scenario.example, Some("wifi_promisc"));
    assert!(matches!(scenario.runtime, RuntimePlan::UartWatch { .. }));
}

#[test]
fn promiscuous_control_filter_api_is_implemented() {
    let scenario = scenario_by_name("promisc_control_filter_api").expect("control API scenario");

    assert_eq!(scenario.state, ScenarioState::Implemented);
    assert_eq!(scenario.example, Some("wifi_promisc"));
    assert!(matches!(scenario.runtime, RuntimePlan::UartWatch { .. }));
}

#[test]
fn scenario_lookup_exposes_expected_examples_for_existing_flows() {
    let tcp_server = scenario_by_name("tcp_server").expect("tcp_server scenario");
    assert_eq!(tcp_server.example, Some("tcp_server"));

    let tcp_client = scenario_by_name("tcp_client").expect("tcp_client scenario");
    assert_eq!(tcp_client.example, Some("tcp_client"));

    let sta_connect = scenario_by_name("sta_connect").expect("sta_connect scenario");
    assert_eq!(sta_connect.example, Some("wifi_core_connect"));

    let sta_scan = scenario_by_name("sta_scan").expect("sta_scan scenario");
    assert_eq!(sta_scan.example, Some("raw_scan"));
}

#[test]
fn sta_connect_timeout_has_headroom_over_connect_wait() {
    let sta_connect = scenario_by_name("sta_connect").expect("sta_connect scenario");
    let RuntimePlan::UartWatch { timeout, .. } = sta_connect.runtime else {
        panic!("sta_connect should use UART watch runtime");
    };

    assert!(
        timeout >= Duration::from_secs(20),
        "sta_connect timeout must leave headroom over the example's station connect wait"
    );
}

#[test]
fn matrix_retry_policy_allows_one_transient_failure() {
    assert!(
        matrix_attempt_limit() >= 3,
        "hardware matrix should retry transient RF/setup failures before reporting them"
    );
}

#[test]
fn artifacts_root_defaults_to_tmp_wifi_hw() {
    let root = artifacts_root(None);
    assert!(root.starts_with(Path::new("/tmp")));
    assert!(root.ends_with("esp8266-hal-wifi-hw"));
}

#[test]
fn elf2image_args_include_chip_for_esptool_v5() {
    let args = elf2image_args(Path::new("/tmp/out.bin"), Path::new("/tmp/example"));
    let args: Vec<_> = args.iter().map(|arg| arg.to_string_lossy()).collect();

    assert!(args
        .windows(2)
        .any(|pair| pair[0] == "--chip" && pair[1] == "esp8266"));
}

#[test]
fn tcp_server_timeout_covers_wifi_dhcp_and_echo() {
    let tcp_server = scenario_by_name("tcp_server").expect("tcp_server scenario");
    let RuntimePlan::TcpEcho { timeout, .. } = tcp_server.runtime else {
        panic!("tcp_server should use TCP echo runtime");
    };

    assert!(
        timeout >= Duration::from_secs(25),
        "tcp_server timeout must exceed the 10s WiFi connect wait plus DHCP and host echo time"
    );
}

#[test]
fn tcp_client_uses_host_served_tcp_runtime() {
    let tcp_client = scenario_by_name("tcp_client").expect("tcp_client scenario");
    let RuntimePlan::TcpClient { timeout, port, .. } = tcp_client.runtime else {
        panic!("tcp_client should use host-served TCP runtime");
    };

    assert_eq!(tcp_client.state, ScenarioState::Implemented);
    assert_eq!(port, 4321);
    assert!(
        timeout >= Duration::from_secs(25),
        "tcp_client timeout must cover WiFi connect, DHCP, outbound connect, receive, and close"
    );
}

#[test]
fn eap_sta_assoc_uses_router_radius_uart_runtime() {
    let scenario = scenario_by_name("eap_sta_assoc").expect("eap scenario");
    let RuntimePlan::UartWatch { timeout, .. } = scenario.runtime else {
        panic!("eap_sta_assoc should use UART watch runtime");
    };

    assert_eq!(scenario.state, ScenarioState::Implemented);
    assert_eq!(scenario.example, Some("wifi_eap"));
    assert!(scenario.features.contains(&"wifi-enterprise"));
    assert!(
        timeout >= Duration::from_secs(90),
        "EAP scenario timeout must cover Enterprise association and RADIUS exchange"
    );
}

#[test]
fn ap_start_is_an_implemented_uart_scenario() {
    let ap_start = scenario_by_name("ap_start").expect("ap_start scenario");

    assert_eq!(ap_start.state, ScenarioState::Implemented);
    assert_eq!(ap_start.example, Some("wifi_ap"));
}

#[test]
fn ap_station_list_is_implemented() {
    let scenario = scenario_by_name("ap_station_list").expect("ap_station_list scenario");

    assert_eq!(scenario.state, ScenarioState::Implemented);
    assert_eq!(scenario.example, Some("wifi_ap"));
    assert!(matches!(scenario.runtime, RuntimePlan::UartWatch { .. }));
}

#[test]
fn ap_dhcp_is_implemented() {
    let scenario = scenario_by_name("ap_dhcp").expect("ap_dhcp scenario");

    assert_eq!(scenario.state, ScenarioState::Implemented);
    assert_eq!(scenario.example, Some("wifi_ap"));
    assert!(matches!(scenario.runtime, RuntimePlan::UartWatch { .. }));
}

#[test]
fn ap_sta_is_implemented() {
    let ap_sta = scenario_by_name("ap_sta").expect("ap_sta scenario");

    assert_eq!(ap_sta.state, ScenarioState::Implemented);
    assert_eq!(ap_sta.example, Some("wifi_ap_sta"));
}

#[test]
fn ap_sta_timeout_covers_connect_and_dhcp_budget() {
    let ap_sta = scenario_by_name("ap_sta").expect("ap_sta scenario");
    let RuntimePlan::UartWatch { timeout, .. } = ap_sta.runtime else {
        panic!("ap_sta should use UART watch runtime");
    };

    assert!(
        timeout >= Duration::from_secs(35),
        "ap_sta timeout must exceed the example's 20s connect wait plus IPv4/DHCP wait"
    );
}

#[test]
fn promiscuous_scenarios_are_implemented() {
    for name in [
        "promisc_capture",
        "promisc_filters",
        "promisc_data_companion",
    ] {
        let scenario = scenario_by_name(name).expect("promiscuous scenario");

        assert_eq!(scenario.state, ScenarioState::Implemented);
        assert_eq!(scenario.example, Some("wifi_promisc"));
        if name == "promisc_data_companion" {
            assert!(matches!(scenario.runtime, RuntimePlan::Pending));
        } else {
            assert!(matches!(scenario.runtime, RuntimePlan::UartWatch { .. }));
        }
    }
}

#[test]
fn esp_now_broadcast_is_an_implemented_uart_scenario() {
    let scenario = scenario_by_name("esp_now_broadcast").expect("esp_now_broadcast scenario");

    assert_eq!(scenario.state, ScenarioState::Implemented);
    assert_eq!(scenario.example, Some("wifi_esp_now_tx"));
    assert!(matches!(scenario.runtime, RuntimePlan::UartWatch { .. }));
}

#[test]
fn esp_now_two_board_scenarios_are_implemented() {
    for name in ["esp_now_unicast", "esp_now_encrypted"] {
        let scenario = scenario_by_name(name).expect("esp-now rig scenario");

        assert_eq!(scenario.state, ScenarioState::Implemented);
        assert_eq!(scenario.example, Some("wifi_esp_now_rx"));
        assert!(matches!(scenario.runtime, RuntimePlan::UartWatch { .. }));
    }
}

#[test]
fn wps_scenario_is_implemented() {
    let scenario = scenario_by_name("wps").expect("wps scenario");

    assert_eq!(scenario.state, ScenarioState::Implemented);
    assert_eq!(scenario.example, Some("wifi_wps"));
    assert!(matches!(scenario.runtime, RuntimePlan::UartWatch { .. }));
}

#[test]
fn smartconfig_scenario_is_rig_blocked() {
    let scenario = scenario_by_name("smartconfig").expect("smartconfig scenario");

    let ScenarioState::RigBlocked { reason } = scenario.state else {
        panic!("smartconfig should be rig-blocked");
    };
    assert!(!reason.is_empty());
    assert_eq!(scenario.example, Some("wifi_smartconfig"));
    assert!(matches!(scenario.runtime, RuntimePlan::UartWatch { .. }));
}
