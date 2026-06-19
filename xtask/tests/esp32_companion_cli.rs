use std::path::Path;

use xtask::esp32_companion::{chip_kind_from_esptool_output, repo_local_paths};
use xtask::wifi_hw::{scenario_by_name, RuntimePlan, ScenarioState};

#[test]
fn companion_paths_stay_inside_repo() {
    let repo = Path::new("/repo/esp8266-hal");
    let paths = repo_local_paths(repo);

    for path in [
        &paths.tool_root,
        &paths.idf_checkout,
        &paths.idf_tools,
        &paths.home,
        &paths.cache,
        &paths.pip_cache,
        &paths.build_dir,
        &paths.project_dir,
    ] {
        assert!(
            path.starts_with(repo),
            "path should stay repo-local: {}",
            path.display()
        );
    }
}

#[test]
fn companion_env_redirects_tooling_state_into_repo() {
    let repo = Path::new("/repo/esp8266-hal");
    let paths = repo_local_paths(repo);
    let env = paths.idf_environment();

    for key in ["IDF_TOOLS_PATH", "HOME", "XDG_CACHE_HOME", "PIP_CACHE_DIR"] {
        let value = env
            .iter()
            .find_map(|(candidate, value)| (candidate == key).then_some(value))
            .unwrap_or_else(|| panic!("missing env var {key}"));
        assert!(Path::new(value).starts_with(repo), "{key}={value}");
    }
}

#[test]
fn chip_kind_parser_identifies_esp32_pico_d4() {
    let output = "Chip type:          ESP32-PICO-D4 (revision v1.1)\nMAC: 4c:c3:82:82:32:80\n";
    assert_eq!(chip_kind_from_esptool_output(output), Some("ESP32-PICO-D4"));
}

#[test]
fn stable_companion_scenarios_are_in_the_matrix() {
    for name in [
        "esp_now_unicast_companion",
        "esp_now_encrypted_companion",
        "esp_now_bad_key_companion",
        "promisc_data_companion",
    ] {
        let scenario = scenario_by_name(name).expect("companion scenario");
        assert_eq!(scenario.state, ScenarioState::Implemented);
        assert!(matches!(scenario.runtime, RuntimePlan::CompanionRig));
    }
}

#[test]
fn diagnostic_companion_scenarios_are_in_the_matrix() {
    for name in [
        "ap_station_list_companion",
        "ap_rejoin_companion",
        "ap_attach_companion",
        "ap_net_attach_companion",
        "ap_dhcp_bind_companion",
        "ap_dhcp_companion",
        "ap_sta_downstream_companion",
    ] {
        let scenario = scenario_by_name(name).expect("companion scenario");
        assert_eq!(scenario.state, ScenarioState::Implemented);
        assert!(matches!(scenario.runtime, RuntimePlan::CompanionRig));
    }
}
