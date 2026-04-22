use esp8266_hal::{
    wifi::{Configured as FacadeConfigured, Uninitialized as FacadeUninitialized},
    wifi_core::{Configured, Uninitialized, WifiResources},
};

#[test]
fn wifi_core_module_is_public() {
    let _ = core::mem::size_of::<WifiResources<8, 4, 2>>();
    let _ = core::mem::size_of::<Configured<()>>();
    let _ = core::mem::size_of::<Uninitialized>();
}

#[test]
fn wifi_facade_exposes_typed_core_shape() {
    let _ = core::mem::size_of::<FacadeConfigured<()>>();
    let _ = core::mem::size_of::<FacadeUninitialized>();
}
