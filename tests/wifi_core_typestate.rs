use esp8266_hal::wifi_core::{ApMode, HasAp, HasSta, MixedMode, StaMode, WifiResources};

fn assert_has_sta<M: HasSta>() {}
fn assert_has_ap<M: HasAp>() {}

#[test]
fn mode_capabilities_are_encoded_in_traits() {
    assert_has_sta::<StaMode>();
    assert_has_sta::<MixedMode>();
    assert_has_ap::<ApMode>();
    assert_has_ap::<MixedMode>();
}

#[test]
fn resources_report_declared_capacities() {
    let resources = WifiResources::<8, 4>::new();
    assert_eq!(resources.event_capacity(), 8);
    assert_eq!(resources.rx_capacity(), 4);
}
