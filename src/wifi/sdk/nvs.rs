//! Native configuration NVS boundary for a driver without persistent storage.
//! Native Wi-Fi owns its live RAM configuration; absent records select defaults.
//! PHY calibration is performed once on first attach and keeps its own DC table.

use core::ffi::c_void;
use esp8266_wifi_sys as sys;

const NVS_NOT_FOUND: i32 = 0x1102;

#[no_mangle]
pub unsafe extern "C" fn nvs_open(namespace: *const u8, _open_mode: u32, output: *mut u32) -> i32 {
    if namespace.is_null() || output.is_null() {
        return sys::ESP_ERR_INVALID_ARG;
    }
    // SAFETY: the native caller supplies writable handle storage.
    unsafe { output.write(0) };
    sys::ESP_OK
}

#[no_mangle]
pub extern "C" fn nvs_close(_handle: u32) {}

#[no_mangle]
pub extern "C" fn nvs_commit(_handle: u32) -> i32 {
    sys::ESP_OK
}

macro_rules! scalar_accessors {
    ($get:ident, $set:ident, $ty:ty) => {
        #[no_mangle]
        pub unsafe extern "C" fn $get(_handle: u32, _key: *const u8, _output: *mut $ty) -> i32 {
            NVS_NOT_FOUND
        }

        #[no_mangle]
        pub unsafe extern "C" fn $set(_handle: u32, _key: *const u8, _value: $ty) -> i32 {
            sys::ESP_OK
        }
    };
}

scalar_accessors!(nvs_get_i8, nvs_set_i8, i8);
scalar_accessors!(nvs_get_u8, nvs_set_u8, u8);
scalar_accessors!(nvs_get_u16, nvs_set_u16, u16);

#[no_mangle]
pub unsafe extern "C" fn nvs_get_blob(
    _handle: u32,
    key: *const u8,
    _output: *mut c_void,
    _length: *mut usize,
) -> i32 {
    if key.is_null() {
        sys::ESP_ERR_INVALID_ARG
    } else {
        NVS_NOT_FOUND
    }
}

#[no_mangle]
pub unsafe extern "C" fn nvs_set_blob(
    _handle: u32,
    _key: *const u8,
    _input: *const c_void,
    _length: usize,
) -> i32 {
    sys::ESP_ERR_INVALID_ARG
}
