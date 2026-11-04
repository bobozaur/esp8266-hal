//! Small volatile NVS implementation for SDK state that must survive a Wi-Fi
//! stop/start cycle.
//!
//! The bare-metal driver has no flash-backed IDF NVS partition. The ESP8266
//! live PHY path requires calibration and DC-table records. Keeping them in a fixed Rust
//! store avoids pulling in the IDF NVS component and its filesystem/runtime
//! dependencies.

use core::{cell::UnsafeCell, ffi::c_void};

use esp8266_wifi_sys as sys;

const NVS_NOT_FOUND: i32 = 0x1102;
const PHY_HANDLE: u32 = 1;

struct Blob<const N: usize> {
    bytes: [u8; N],
    len: usize,
    valid: bool,
}

impl<const N: usize> Blob<N> {
    const fn empty() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
            valid: false,
        }
    }

    unsafe fn read(&self, output: *mut c_void, length: *mut usize) -> i32 {
        if length.is_null() {
            return sys::ESP_ERR_INVALID_ARG;
        }
        if !self.valid {
            return NVS_NOT_FOUND;
        }

        let capacity = unsafe { length.read() };
        unsafe { length.write(self.len) };
        if output.is_null() {
            return sys::ESP_OK;
        }
        if capacity < self.len {
            return sys::ESP_ERR_INVALID_SIZE;
        }

        unsafe { core::ptr::copy_nonoverlapping(self.bytes.as_ptr(), output.cast(), self.len) };
        sys::ESP_OK
    }

    unsafe fn write(&mut self, input: *const c_void, len: usize) -> i32 {
        if len > N || (input.is_null() && len != 0) {
            return sys::ESP_ERR_INVALID_ARG;
        }

        if len != 0 {
            unsafe { core::ptr::copy_nonoverlapping(input.cast(), self.bytes.as_mut_ptr(), len) };
        }
        self.len = len;
        self.valid = true;
        sys::ESP_OK
    }
}

struct NvsStore {
    phy_calibration: Blob<128>,
    phy_dc_table: Blob<500>,
}

impl NvsStore {
    const fn empty() -> Self {
        Self {
            phy_calibration: Blob::empty(),
            phy_dc_table: Blob::empty(),
        }
    }
}

struct Store(UnsafeCell<NvsStore>);

// Every access is serialized by the single-core critical-section
// implementation below. The SDK may call NVS from more than one compat task,
// so the synchronization is part of the ABI contract rather than an
// initialization-time assumption.
unsafe impl Sync for Store {}

static STORE: Store = Store(UnsafeCell::new(NvsStore::empty()));

fn with_store<R>(f: impl FnOnce(&mut NvsStore) -> R) -> R {
    critical_section::with(|_| unsafe { f(&mut *STORE.0.get()) })
}

unsafe fn c_str_eq(mut actual: *const u8, expected: &[u8]) -> bool {
    if actual.is_null() {
        return false;
    }
    for &byte in expected {
        if unsafe { actual.read() } != byte {
            return false;
        }
        actual = unsafe { actual.add(1) };
    }
    unsafe { actual.read() == 0 }
}

#[no_mangle]
pub extern "C" fn nvs_flash_init() -> i32 {
    sys::ESP_OK
}

#[no_mangle]
pub extern "C" fn nvs_flash_erase() -> i32 {
    with_store(|store| *store = NvsStore::empty());
    sys::ESP_OK
}

#[no_mangle]
pub unsafe extern "C" fn nvs_open(namespace: *const u8, _open_mode: u32, output: *mut u32) -> i32 {
    if namespace.is_null() || output.is_null() {
        return sys::ESP_ERR_INVALID_ARG;
    }

    let handle = if unsafe { c_str_eq(namespace, b"phy") } {
        PHY_HANDLE
    } else {
        // The SDK treats an unknown optional namespace as an empty store.
        0
    };
    unsafe { output.write(handle) };
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
    handle: u32,
    key: *const u8,
    output: *mut c_void,
    length: *mut usize,
) -> i32 {
    if key.is_null() {
        return sys::ESP_ERR_INVALID_ARG;
    }

    with_store(|store| unsafe {
        if handle == PHY_HANDLE && c_str_eq(key, b"cal_data") {
            store.phy_calibration.read(output, length)
        } else if handle == PHY_HANDLE && c_str_eq(key, b"dc_table") {
            store.phy_dc_table.read(output, length)
        } else {
            NVS_NOT_FOUND
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn nvs_set_blob(
    handle: u32,
    key: *const u8,
    input: *const c_void,
    len: usize,
) -> i32 {
    if key.is_null() {
        return sys::ESP_ERR_INVALID_ARG;
    }

    with_store(|store| unsafe {
        if handle == PHY_HANDLE && c_str_eq(key, b"cal_data") {
            store.phy_calibration.write(input, len)
        } else if handle == PHY_HANDLE && c_str_eq(key, b"dc_table") {
            store.phy_dc_table.write(input, len)
        } else {
            sys::ESP_ERR_INVALID_ARG
        }
    })
}
