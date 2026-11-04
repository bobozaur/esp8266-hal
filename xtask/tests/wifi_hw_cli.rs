use std::path::Path;
use std::time::Duration;

use xtask::wifi_hw::{
    artifacts_root, elf2image_args, matrix_attempt_limit, scenario_by_name,
    scenario_matrix_lane_label, scenario_names, scenario_resource_labels, RuntimePlan,
    ScenarioState,
};

#[test]
fn scenario_registry_includes_required_full_matrix_entries() {
    let names = scenario_names();

    for required in [
        "sta_connect",
        "sta_scan_connect",
        "sta_wpa2_connect",
        "sta_wrong_password",
        "sta_stale_channel_hint",
        "sta_hidden_ssid",
        "sta_scan",
        "sta_dhcp",
        "sta_static_ip",
        "tcp_server",
        "tcp_client",
        "eap_sta_assoc",
        "eap_wrong_password",
        "eap_client_cert",
        "ap_station_list_companion",
        "ap_rejoin_companion",
        "ap_attach_companion",
        "ap_net_attach_companion",
        "ap_dhcp_bind_companion",
        "ap_dhcp_companion",
        "ap_sta_downstream_companion",
        "ap_start",
        "ap_station_list",
        "ap_dhcp",
        "ap_sta",
        "wps",
        "wps_pin",
        "smartconfig",
        "esp_now_unicast_companion",
        "esp_now_encrypted_companion",
        "esp_now_bad_key_companion",
        "esp_now_unicast",
        "esp_now_broadcast",
        "esp_now_encrypted",
        "promisc_data_companion",
        "promisc_capture",
        "promisc_filters",
        "promisc_control_filters",
    ] {
        assert!(
            names.iter().any(|name| *name == required),
            "missing required scenario: {required}"
        );
    }
}

#[test]
fn scenario_registry_names_are_unique() {
    let names = scenario_names();

    for (index, name) in names.iter().enumerate() {
        assert!(
            !names[index + 1..].iter().any(|candidate| candidate == name),
            "duplicate scenario name: {name}"
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
fn no_registered_wifi_scenario_is_left_manual() {
    for name in scenario_names() {
        let scenario = scenario_by_name(name).expect("registered scenario");
        assert_ne!(scenario.state, ScenarioState::Manual, "{name}");
    }
}

#[test]
fn implemented_scenarios_have_executable_runtime_metadata() {
    for name in scenario_names() {
        let scenario = scenario_by_name(name).expect("registered scenario");
        if scenario.state == ScenarioState::Implemented {
            assert!(
                !matches!(scenario.runtime, RuntimePlan::Pending),
                "implemented scenario `{name}` must not use the pending runtime placeholder"
            );
        }
    }
}

#[test]
fn runtime_labels_are_stable_for_audit_outputs() {
    assert_eq!(
        scenario_by_name("sta_connect")
            .expect("sta_connect")
            .runtime
            .label(),
        "uart-watch"
    );
    assert_eq!(
        scenario_by_name("tcp_server")
            .expect("tcp_server")
            .runtime
            .label(),
        "tcp-echo"
    );
    assert_eq!(
        scenario_by_name("tcp_client")
            .expect("tcp_client")
            .runtime
            .label(),
        "tcp-client"
    );
    assert_eq!(
        scenario_by_name("esp_now_unicast_companion")
            .expect("esp-now companion")
            .runtime
            .label(),
        "companion-rig"
    );
    assert_eq!(RuntimePlan::Pending.label(), "pending");
}

#[test]
fn scenario_audit_metadata_exposes_fixture_resources_and_parallel_lane() {
    let eap = scenario_by_name("eap_sta_assoc").expect("eap scenario");
    assert_eq!(scenario_matrix_lane_label(eap), "secondary-candidate");
    assert_eq!(scenario_resource_labels(eap), ["router"]);

    let sta = scenario_by_name("sta_connect").expect("sta_connect scenario");
    assert_eq!(scenario_matrix_lane_label(sta), "primary");
    assert_eq!(scenario_resource_labels(sta), ["esp32-companion"]);

    let wps = scenario_by_name("wps").expect("wps scenario");
    assert_eq!(scenario_matrix_lane_label(wps), "primary");
    assert_eq!(scenario_resource_labels(wps), ["router"]);

    let eap_client_cert = scenario_by_name("eap_client_cert").expect("eap client-cert scenario");
    assert_eq!(scenario_matrix_lane_label(eap_client_cert), "primary");
    assert_eq!(scenario_resource_labels(eap_client_cert), ["router"]);
}

#[test]
fn rig_blocked_scenarios_have_reasons() {
    for name in scenario_names() {
        let scenario = scenario_by_name(name).expect("registered scenario");
        if let Some(reason) = scenario.state.rig_blocked_reason() {
            assert_eq!(scenario.state.label(), "rig-blocked");
            assert!(!reason.is_empty(), "{name}");
        }
    }
}

#[test]
fn promiscuous_control_capture_is_implemented() {
    let scenario =
        scenario_by_name("promisc_control_filters").expect("promiscuous control scenario");
    let RuntimePlan::UartWatch {
        success_markers,
        failure_markers,
        ..
    } = scenario.runtime
    else {
        panic!("promiscuous control capture should use UART watch runtime");
    };

    assert_eq!(scenario.state, ScenarioState::Implemented);
    assert_eq!(scenario.example, Some("wifi_promisc"));
    assert!(success_markers.contains(&"promisc-control-ok"));
    assert!(failure_markers.contains(&"promisc timeout"));
}

#[test]
fn promiscuous_control_filter_api_is_not_a_runtime_scenario() {
    assert!(scenario_by_name("promisc_control_filter_api").is_none());
}

#[test]
fn scenario_lookup_exposes_expected_examples_for_existing_flows() {
    let tcp_server = scenario_by_name("tcp_server").expect("tcp_server scenario");
    assert_eq!(tcp_server.example, Some("tcp_server"));

    let tcp_client = scenario_by_name("tcp_client").expect("tcp_client scenario");
    assert_eq!(tcp_client.example, Some("tcp_client"));

    let sta_connect = scenario_by_name("sta_connect").expect("sta_connect scenario");
    assert_eq!(sta_connect.example, Some("wifi_core_connect"));

    let sta_scan_connect = scenario_by_name("sta_scan_connect").expect("sta_scan_connect scenario");
    assert_eq!(sta_scan_connect.example, Some("wifi_core_connect"));

    let sta_wpa2_connect = scenario_by_name("sta_wpa2_connect").expect("sta_wpa2_connect scenario");
    assert_eq!(sta_wpa2_connect.example, Some("wifi_core_connect"));

    let sta_scan = scenario_by_name("sta_scan").expect("sta_scan scenario");
    assert_eq!(sta_scan.example, Some("raw_scan"));
}

#[test]
fn sta_wrong_password_is_an_implemented_negative_connect_scenario() {
    let scenario = scenario_by_name("sta_wrong_password").expect("sta wrong password scenario");
    let RuntimePlan::UartWatch {
        timeout,
        success_markers,
        failure_markers,
    } = scenario.runtime
    else {
        panic!("sta_wrong_password should use UART watch runtime");
    };

    assert_eq!(scenario.state, ScenarioState::Implemented);
    assert_eq!(scenario.example, Some("wifi_core_connect"));
    assert!(success_markers.contains(&"wifi core connect err"));
    assert!(failure_markers.contains(&"wifi core connected"));
    assert!(
        timeout >= Duration::from_secs(45),
        "wrong-password timeout must cover the shortened connect wait and WPA retry events"
    );
}

#[test]
fn sta_stale_channel_hint_is_an_implemented_positive_connect_scenario() {
    let scenario = scenario_by_name("sta_stale_channel_hint").expect("sta stale channel scenario");
    let RuntimePlan::UartWatch {
        timeout,
        success_markers,
        failure_markers,
    } = scenario.runtime
    else {
        panic!("sta_stale_channel_hint should use UART watch runtime");
    };

    assert_eq!(scenario.state, ScenarioState::Implemented);
    assert_eq!(scenario.example, Some("wifi_core_connect"));
    assert!(success_markers.contains(&"wifi core connected"));
    assert!(failure_markers.contains(&"wifi core connect err"));
    assert!(
        timeout >= Duration::from_secs(60),
        "stale-channel timeout must cover scan fallback, association, and connect"
    );
}

#[test]
fn sta_hidden_ssid_is_an_implemented_positive_connect_scenario() {
    let scenario = scenario_by_name("sta_hidden_ssid").expect("sta hidden ssid scenario");
    let RuntimePlan::UartWatch {
        timeout,
        success_markers,
        failure_markers,
    } = scenario.runtime
    else {
        panic!("sta_hidden_ssid should use UART watch runtime");
    };

    assert_eq!(scenario.state, ScenarioState::Implemented);
    assert_eq!(scenario.example, Some("wifi_core_connect"));
    assert!(success_markers.contains(&"wifi core connected"));
    assert!(failure_markers.contains(&"wifi core connect err"));
    assert!(
        timeout >= Duration::from_secs(60),
        "hidden-SSID timeout must cover station start and active scan/association"
    );
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
fn sta_scan_connect_is_an_implemented_positive_connect_scenario() {
    let scenario = scenario_by_name("sta_scan_connect").expect("sta scan-connect scenario");
    let RuntimePlan::UartWatch {
        timeout,
        success_markers,
        failure_markers,
    } = scenario.runtime
    else {
        panic!("sta_scan_connect should use UART watch runtime");
    };

    assert_eq!(scenario.state, ScenarioState::Implemented);
    assert_eq!(scenario.example, Some("wifi_core_connect"));
    assert!(success_markers.contains(&"wifi core connected"));
    assert!(failure_markers.contains(&"wifi core connect err"));
    assert!(
        timeout >= Duration::from_secs(60),
        "scan-connect timeout must cover all-channel discovery and association"
    );
}

#[test]
fn sta_wpa2_connect_is_an_implemented_positive_connect_scenario() {
    let scenario = scenario_by_name("sta_wpa2_connect").expect("sta wpa2-connect scenario");
    let RuntimePlan::UartWatch {
        timeout,
        success_markers,
        failure_markers,
    } = scenario.runtime
    else {
        panic!("sta_wpa2_connect should use UART watch runtime");
    };

    assert_eq!(scenario.state, ScenarioState::Implemented);
    assert_eq!(scenario.example, Some("wifi_core_connect"));
    assert!(success_markers.contains(&"wifi core connected"));
    assert!(failure_markers.contains(&"wifi core connect err"));
    assert!(
        timeout >= Duration::from_secs(60),
        "WPA2 connect timeout must cover handshake and association"
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
fn artifacts_root_defaults_to_repo_target_wifi_hw() {
    let root = artifacts_root(None);
    assert!(root.ends_with(Path::new("target/wifi-hw-artifacts")));
    assert!(!root.starts_with(Path::new("/tmp")));
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
fn eap_wrong_password_is_an_implemented_negative_enterprise_scenario() {
    let scenario = scenario_by_name("eap_wrong_password").expect("eap negative scenario");
    let RuntimePlan::UartWatch {
        timeout,
        success_markers,
        failure_markers,
    } = scenario.runtime
    else {
        panic!("eap_wrong_password should use UART watch runtime");
    };

    assert_eq!(scenario.state, ScenarioState::Implemented);
    assert_eq!(scenario.example, Some("wifi_eap"));
    assert!(scenario.features.contains(&"wifi-enterprise"));
    assert!(success_markers.contains(&"eap-negative-disconnected"));
    assert!(failure_markers.contains(&"wifi eap connect err"));
    assert!(failure_markers.contains(&"eap-ok"));
    assert!(failure_markers.contains(&"wifi eap associated"));
    assert!(
        timeout >= Duration::from_secs(90),
        "EAP negative timeout must cover Enterprise rejection and retry events"
    );
}

#[test]
fn eap_client_certificate_validation_is_implemented_as_opt_in_runtime() {
    let scenario = scenario_by_name("eap_client_cert").expect("eap client-cert scenario");

    assert_eq!(scenario.state.label(), "implemented");
    assert!(scenario.state.rig_blocked_reason().is_none());
    assert_eq!(scenario.example, Some("wifi_eap"));
    assert!(scenario.features.contains(&"wifi-enterprise"));
    let RuntimePlan::UartWatch {
        timeout,
        success_markers,
        failure_markers,
    } = scenario.runtime
    else {
        panic!("eap_client_cert should have an opt-in UART runtime");
    };
    assert!(timeout >= Duration::from_secs(120));
    assert!(success_markers.contains(&"eap-ok"));
    assert!(failure_markers.contains(&"wifi eap client cert incomplete"));
}

#[test]
fn ap_start_is_an_implemented_uart_scenario() {
    let ap_start = scenario_by_name("ap_start").expect("ap_start scenario");

    assert_eq!(ap_start.state, ScenarioState::Implemented);
    assert_eq!(ap_start.example, Some("wifi_ap"));
}

#[test]
fn ap_companion_stage_scenarios_are_implemented() {
    for name in [
        "ap_station_list_companion",
        "ap_rejoin_companion",
        "ap_attach_companion",
        "ap_net_attach_companion",
        "ap_dhcp_bind_companion",
        "ap_dhcp_companion",
        "ap_sta_downstream_companion",
    ] {
        let scenario = scenario_by_name(name).expect("AP companion stage scenario");

        assert_eq!(scenario.state, ScenarioState::Implemented);
        assert!(scenario.example == Some("wifi_ap") || scenario.example == Some("wifi_ap_sta"));
        assert!(matches!(scenario.runtime, RuntimePlan::CompanionRig));
    }
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
        "promisc_control_filters",
        "promisc_data_companion",
    ] {
        let scenario = scenario_by_name(name).expect("promiscuous scenario");

        assert_eq!(scenario.state, ScenarioState::Implemented);
        assert_eq!(scenario.example, Some("wifi_promisc"));
        if name == "promisc_data_companion" {
            assert!(matches!(scenario.runtime, RuntimePlan::CompanionRig));
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
    for name in ["wps", "wps_pin"] {
        let scenario = scenario_by_name(name).expect("wps scenario");

        assert_eq!(scenario.state, ScenarioState::Implemented);
        assert_eq!(scenario.example, Some("wifi_wps"));
        assert!(matches!(scenario.runtime, RuntimePlan::UartWatch { .. }));
    }
}

#[test]
fn smartconfig_scenario_is_implemented() {
    let scenario = scenario_by_name("smartconfig").expect("smartconfig scenario");

    assert_eq!(scenario.state, ScenarioState::Implemented);
    assert_eq!(scenario.example, Some("wifi_smartconfig"));
    assert!(scenario.description.contains("ESPTouch-compatible sender"));
    assert!(matches!(scenario.runtime, RuntimePlan::UartWatch { .. }));
}
