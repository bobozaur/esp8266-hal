//! System API: MAC address management, chip info, CPU frequency.
//!
//! Replaces `sdk_overrides/system_api.c`. These functions are called by
//! both Rust code (via FFI) and the WiFi blob libraries (by symbol name).

use core::ffi::c_void;
use esp8266_wifi_sys as sys;

// ---------------------------------------------------------------------------
// EFUSE register addresses
// ---------------------------------------------------------------------------

const EFUSE_DATA0_REG: *const u32 = 0x3FF0_0050 as *const u32;
const EFUSE_DATA1_REG: *const u32 = 0x3FF0_0054 as *const u32;
const EFUSE_DATA2_REG: *const u32 = 0x3FF0_0058 as *const u32;
const EFUSE_DATA3_REG: *const u32 = 0x3FF0_005C as *const u32;

const EFUSE_IS_48BITS_MAC: u32 = 1 << 12; // SDK: efuse_register.h
const EFUSE_VERSION_S: u32 = 24;
const EFUSE_VERSION_V: u32 = 0xF;
const EFUSE_VERSION_1: u32 = 1;
const EFUSE_VERSION_2: u32 = 2;

const DPORT_CTL_REG: *const u32 = 0x3FF0_0014 as *const u32;
const DPORT_CTL_DOUBLE_CLK: u32 = 1;
const CPU_CLK_FREQ: u32 = 80_000_000;
const CONFIG_FREERTOS_HZ: u32 = 100;

// ---------------------------------------------------------------------------
// Globals
// ---------------------------------------------------------------------------

static mut BASE_MAC_ADDR: [u8; 6] = [0; 6];
static mut READ_MAC_TRACE_COUNT: u8 = 0;

#[inline(always)]
fn hex_nibble(value: u8) -> u8 {
    match value & 0x0f {
        0..=9 => b'0' + (value & 0x0f),
        nibble => b'a' + (nibble - 10),
    }
}

#[inline(always)]
fn trace_mac_bytes(call_idx: u8, mac_type: u32, mac: &[u8; 6]) {
    crate::logger::write_byte(b'~');
    crate::logger::write_byte(hex_nibble(call_idx));
    crate::logger::write_byte(hex_nibble(mac_type as u8));
    for &byte in mac {
        crate::logger::write_byte(hex_nibble(byte >> 4));
        crate::logger::write_byte(hex_nibble(byte));
    }
    crate::logger::write_byte(b'~');
}

/// CPU clock ticks per microsecond. Used by `esp_timer_get_time` and
/// `ets_delay_us` (ROM functions). Updated by `os_update_cpu_frequency`.
#[no_mangle]
pub static mut g_esp_ticks_per_us: u32 = 80;

/// System info struct placed in a fixed linker section that the ROM reads.
#[repr(C)]
pub struct EspSysInfo {
    pub version: u32,
}

#[no_mangle]
#[link_section = ".SystemInfoVector.text"]
pub static g_esp_sys_info: EspSysInfo = EspSysInfo {
    // ESP_IDF_VERSION from the SDK — for compatibility, use a fixed value.
    version: 0x0003_0400, // v3.4.0
};

// ---------------------------------------------------------------------------
// CRC8 (from ROM)
// ---------------------------------------------------------------------------

extern "C" {
    fn esp_crc8(data: *const u8, len: u32) -> u8;
    fn nvs_flash_init() -> i32;
    fn nvs_open(name: *const u8, mode: u32, handle: *mut u32) -> i32;
    fn nvs_close(handle: u32);
    fn nvs_get_blob(handle: u32, key: *const u8, out: *mut c_void, len: *mut usize) -> i32;
    fn nvs_set_blob(handle: u32, key: *const u8, data: *const c_void, len: usize) -> i32;
    fn nvs_commit(handle: u32) -> i32;
    static mut _xt_tick_divisor: u32;
}

// NVS constants
const NVS_READONLY: u32 = 0;
const NVS_READWRITE: u32 = 1;

// ---------------------------------------------------------------------------
// MAC address functions
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn esp_base_mac_addr_set(mac: *const u8) -> i32 {
    unsafe {
        if mac.is_null() {
            return sys::ESP_ERR_INVALID_ARG;
        }
        core::ptr::copy_nonoverlapping(mac, BASE_MAC_ADDR.as_mut_ptr(), 6);
        sys::ESP_OK
    }
}

#[no_mangle]
pub unsafe extern "C" fn esp_base_mac_addr_get(mac: *mut u8) -> i32 {
    unsafe {
        if BASE_MAC_ADDR.iter().all(|&b| b == 0) {
            return 0x1102; // ESP_ERR_INVALID_MAC
        }
        core::ptr::copy_nonoverlapping(BASE_MAC_ADDR.as_ptr(), mac, 6);
        sys::ESP_OK
    }
}

/// Read the default MAC address from EFUSE registers.
///
/// Validates CRC checksums and falls back to the Espressif OUI
/// `18:FE:34` if the EFUSE data is invalid.
#[no_mangle]
pub unsafe extern "C" fn esp_efuse_mac_get_default(mac: *mut u8) -> i32 {
    unsafe {
        let efuse = [
            EFUSE_DATA0_REG.read_volatile(),
            EFUSE_DATA1_REG.read_volatile(),
            EFUSE_DATA2_REG.read_volatile(),
            EFUSE_DATA3_REG.read_volatile(),
        ];

        let m = core::slice::from_raw_parts_mut(mac, 6);

        m[3] = (efuse[1] >> 8) as u8;
        m[4] = efuse[1] as u8;
        m[5] = (efuse[0] >> 24) as u8;

        let mut use_default = true;

        if efuse[2] & EFUSE_IS_48BITS_MAC != 0 {
            m[0] = (efuse[3] >> 16) as u8;
            m[1] = (efuse[3] >> 8) as u8;
            m[2] = efuse[3] as u8;
            use_default = false;

            // Validate upper MAC CRC
            let tmp = [m[2], m[1], m[0]];
            let efuse_crc = (efuse[2] >> 24) as u8;
            let calc_crc = esp_crc8(tmp.as_ptr(), 3);
            if efuse_crc != calc_crc {
                use_default = true;
            }

            if !use_default {
                let version = (efuse[1] >> EFUSE_VERSION_S) & EFUSE_VERSION_V;
                if version == EFUSE_VERSION_1 || version == EFUSE_VERSION_2 {
                    let tmp2 = [m[5], m[4], m[3], (efuse[1] >> 16) as u8];
                    let efuse_crc2 = (efuse[0] >> 16) as u8;
                    let calc_crc2 = esp_crc8(tmp2.as_ptr(), 4);
                    if efuse_crc2 != calc_crc2 {
                        use_default = true;
                    }
                }
            }
        }

        if use_default {
            m[0] = 0x18;
            m[1] = 0xFE;
            m[2] = 0x34;
        }

        sys::ESP_OK
    }
}

/// Derive a local MAC address from a universal MAC (for soft-AP).
#[no_mangle]
pub unsafe extern "C" fn esp_derive_mac(local_mac: *mut u8, universal_mac: *const u8) -> i32 {
    unsafe {
        if local_mac.is_null() || universal_mac.is_null() {
            return sys::ESP_ERR_INVALID_ARG;
        }
        core::ptr::copy_nonoverlapping(universal_mac, local_mac, 6);

        let local = core::slice::from_raw_parts_mut(local_mac, 6);
        let universal = core::slice::from_raw_parts(universal_mac, 6);

        for idx in 0..64u8 {
            local[0] = universal[0] | 0x02;
            local[0] ^= idx << 2;
            if local != universal {
                break;
            }
        }
        sys::ESP_OK
    }
}

/// Read MAC address by type (STA or SoftAP).
#[no_mangle]
pub unsafe extern "C" fn esp_read_mac(mac: *mut u8, mac_type: u32) -> i32 {
    unsafe {
        if mac.is_null() {
            return sys::ESP_ERR_INVALID_ARG;
        }
        if mac_type > sys::ESP_MAC_WIFI_SOFTAP {
            return sys::ESP_ERR_INVALID_ARG;
        }

        let mut efuse_mac = [0u8; 6];
        let ret = esp_efuse_mac_get_default(efuse_mac.as_mut_ptr());
        if ret != sys::ESP_OK {
            return ret;
        }

        match mac_type {
            sys::ESP_MAC_WIFI_STA => {
                core::ptr::copy_nonoverlapping(efuse_mac.as_ptr(), mac, 6);
            }
            sys::ESP_MAC_WIFI_SOFTAP => {
                esp_derive_mac(mac, efuse_mac.as_ptr());
            }
            _ => {}
        }

        if READ_MAC_TRACE_COUNT < 3 {
            let trace_idx = READ_MAC_TRACE_COUNT;
            READ_MAC_TRACE_COUNT += 1;
            trace_mac_bytes(trace_idx, mac_type, &efuse_mac);
        }

        sys::ESP_OK
    }
}

/// Initialize MAC address subsystem: read from EFUSE, store backup in NVS.
#[no_mangle]
pub unsafe extern "C" fn esp_mac_init() -> i32 {
    unsafe {
        let ret = nvs_flash_init();
        if ret != sys::ESP_OK {
            return 0x1102; // ESP_ERR_INVALID_MAC
        }

        let mut efuse_mac = [0u8; 6];

        // Try loading from NVS backup first
        if load_backup_mac_data(efuse_mac.as_mut_ptr()) == sys::ESP_OK {
            return sys::ESP_OK;
        }

        // Read from EFUSE
        let ret = esp_efuse_mac_get_default(efuse_mac.as_mut_ptr());
        if ret != sys::ESP_OK {
            return 0x1102;
        }

        // Populate the global base MAC
        esp_base_mac_addr_set(efuse_mac.as_ptr());

        // Store backup
        let _ = store_backup_mac_data();

        sys::ESP_OK
    }
}

// ---------------------------------------------------------------------------
// NVS backup MAC helpers
// ---------------------------------------------------------------------------

const BACKUP_MAC_NAMESPACE: &[u8] = b"backup_mac\0";
const BACKUP_MAC_DATA_KEY: &[u8] = b"backup_mac_data\0";
const MAC_DATA_LEN_WITH_CRC: usize = 16; // 4 × u32

unsafe fn load_backup_mac_data(mac: *mut u8) -> i32 {
    unsafe {
        let mut handle: u32 = 0;
        let err = nvs_open(BACKUP_MAC_NAMESPACE.as_ptr(), NVS_READONLY, &mut handle);
        if err != sys::ESP_OK {
            return 0x1102;
        }

        let mut efuse = [0u32; 4];
        let mut length = MAC_DATA_LEN_WITH_CRC;
        let err = nvs_get_blob(
            handle,
            BACKUP_MAC_DATA_KEY.as_ptr(),
            efuse.as_mut_ptr() as *mut c_void,
            &mut length,
        );
        nvs_close(handle);
        if err != sys::ESP_OK || length != MAC_DATA_LEN_WITH_CRC {
            return 0x1102;
        }

        let m = core::slice::from_raw_parts_mut(mac, 6);
        m[3] = (efuse[1] >> 8) as u8;
        m[4] = efuse[1] as u8;
        m[5] = (efuse[0] >> 24) as u8;

        if efuse[2] & EFUSE_IS_48BITS_MAC != 0 {
            m[0] = (efuse[3] >> 16) as u8;
            m[1] = (efuse[3] >> 8) as u8;
            m[2] = efuse[3] as u8;

            // Validate CRC
            let tmp = [m[2], m[1], m[0]];
            let efuse_crc = (efuse[2] >> 24) as u8;
            let calc_crc = esp_crc8(tmp.as_ptr(), 3);
            if efuse_crc != calc_crc {
                return 0x1102;
            }

            let version = (efuse[1] >> EFUSE_VERSION_S) & EFUSE_VERSION_V;
            if version == EFUSE_VERSION_1 || version == EFUSE_VERSION_2 {
                let tmp2 = [m[5], m[4], m[3], (efuse[1] >> 16) as u8];
                let efuse_crc2 = (efuse[0] >> 16) as u8;
                let calc_crc2 = esp_crc8(tmp2.as_ptr(), 4);
                if efuse_crc2 != calc_crc2 {
                    return 0x1102;
                }
            }
        } else {
            m[0] = 0x18;
            m[1] = 0xFE;
            m[2] = 0x34;
        }

        sys::ESP_OK
    }
}

unsafe fn store_backup_mac_data() -> i32 {
    unsafe {
        let efuse = [
            EFUSE_DATA0_REG.read_volatile(),
            EFUSE_DATA1_REG.read_volatile(),
            EFUSE_DATA2_REG.read_volatile(),
            EFUSE_DATA3_REG.read_volatile(),
        ];

        let mut handle: u32 = 0;
        let err = nvs_open(BACKUP_MAC_NAMESPACE.as_ptr(), NVS_READWRITE, &mut handle);
        if err != sys::ESP_OK {
            return err;
        }

        let err = nvs_set_blob(
            handle,
            BACKUP_MAC_DATA_KEY.as_ptr(),
            efuse.as_ptr() as *const c_void,
            MAC_DATA_LEN_WITH_CRC,
        );
        if err != sys::ESP_OK {
            return err;
        }

        let err = nvs_commit(handle);
        nvs_close(handle);
        err
    }
}

// ---------------------------------------------------------------------------
// Chip info and system utilities
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn esp_get_idf_version() -> *const u8 {
    b"v3.4-dirty\0".as_ptr()
}

/// esp_chip_info_t equivalent (the SDK struct may not be in bindgen output).
#[repr(C)]
struct ChipInfo {
    model: u32,
    features: u32,
    cores: u8,
    revision: u8,
}

#[no_mangle]
pub unsafe extern "C" fn esp_chip_info(out_info: *mut ChipInfo) {
    unsafe {
        let info = &mut *out_info;
        core::ptr::write_bytes(out_info as *mut u8, 0, core::mem::size_of::<ChipInfo>());
        info.model = 1; // CHIP_ESP8266
        info.revision = 1;
        info.cores = 1;
        info.features = 1; // CHIP_FEATURE_WIFI_BGN
    }
}

/// Update CPU frequency tracking. Called when the DPORT double-clock bit changes.
///
/// Updates `g_esp_ticks_per_us` and `_xt_tick_divisor` to match the actual
/// CPU frequency. This is an alias for `ets_update_cpu_frequency`.
#[no_mangle]
pub unsafe extern "C" fn os_update_cpu_frequency(_ticks_per_us: u32) {
    unsafe {
        if DPORT_CTL_REG.read_volatile() & DPORT_CTL_DOUBLE_CLK != 0 {
            g_esp_ticks_per_us = CPU_CLK_FREQ * 2 / 1_000_000;
            _xt_tick_divisor = CPU_CLK_FREQ * 2 / CONFIG_FREERTOS_HZ;
        } else {
            g_esp_ticks_per_us = CPU_CLK_FREQ / 1_000_000;
            _xt_tick_divisor = CPU_CLK_FREQ / CONFIG_FREERTOS_HZ;
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn ets_update_cpu_frequency(ticks_per_us: u32) {
    unsafe { os_update_cpu_frequency(ticks_per_us) }
}

// ---------------------------------------------------------------------------
// Heap size (forwarded to our allocator)
// ---------------------------------------------------------------------------

// These are also defined in memory.rs. The C versions called
// heap_caps_get_free_size / heap_caps_get_minimum_free_size from the SDK.
// Our allocator provides the same info through the Rust API.
// The symbols are already provided by memory.rs (#[no_mangle] pub extern "C"),
// so we don't need to redefine them here.

// ---------------------------------------------------------------------------
// Old sysconf addr (ROM compatibility)
// ---------------------------------------------------------------------------

extern "C" {
    static rtc_sys_info: RtcSysInfoC;
}

#[repr(C)]
pub struct RtcSysInfoC {
    pub old_sysconf_addr: u32,
    // ... other fields we don't need
}

#[no_mangle]
pub unsafe extern "C" fn esp_get_old_sysconf_addr() -> u32 {
    unsafe { rtc_sys_info.old_sysconf_addr }
}
