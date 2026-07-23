//! PHY initialization and RF calibration.
//!
//! Replaces `sdk_overrides/phy_init.c`. Manages the PHY/RF init sequence,
//! calibration data persistence via NVS, and clock initialization.

use core::ffi::c_void;
use esp8266_wifi_sys as sys;

// ---------------------------------------------------------------------------
// PHY init data (128 bytes, from esp_private/phy_init_data.h)
// ---------------------------------------------------------------------------

/// Default PHY init data for 26 MHz crystal, 20 dBm max TX power.
#[rustfmt::skip]
const PHY_INIT_DATA: [u8; 128] = [
    0x05, 0x00, 0x04, 0x02, 0x05, 0x05, 0x05, 0x02,
    0x05, 0x00, 0x04, 0x05, 0x05, 0x04, 0x05, 0x05,
    0x04, 0xfe, 0xfd, 0xff, 0xf0, 0xf0, 0xf0, 0xe0,
    0xe0, 0xe0, 0xe1, 0x0a, 0xff, 0xff, 0xf8, 0x00,
    0xf8, 0xf8,
    0x50, // max_tx_power * 4
    0x4e, 0x4a, 0x44, 0x40, 0x38,
    0x00, 0x00, 0x01, 0x01, 0x02, 0x03, 0x04, 0x05,
    0x01, // byte 48: XTAL flag (1 = 26MHz)
    0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0xe1, 0x0a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x01, 0x93, 0x43, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00,
    0x00, // byte 107: VDD33 const (0 = auto)
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

// ---------------------------------------------------------------------------
// Blob function imports
// ---------------------------------------------------------------------------

extern "C" {
    fn register_chipv6_phy(init_data: *const u8) -> i32;
    fn phy_afterwake_set_rfoption(option: u8);
    fn phy_set_powerup_option(option: u8);
    fn phy_disable_agc();
    fn phy_check_data_table(table: *const u32, len: u32, mode: u32) -> u8;
    fn get_data_from_rtc(data: *mut u8);
    fn write_data_to_rtc(data: *const u8);
    fn rtc_init() -> i32;
    fn rtc_init_clk(init_data: *const u8);
    fn uart_tx_wait_idle(uart_no: u8);
    fn uart_div_modify(uart_no: u8, divisor: u32);

    fn nvs_open(name: *const u8, mode: u32, handle: *mut u32) -> i32;
    fn nvs_close(handle: u32);
    fn nvs_get_blob(handle: u32, key: *const u8, out: *mut c_void, len: *mut usize) -> i32;
    fn nvs_set_blob(handle: u32, key: *const u8, data: *const c_void, len: usize) -> i32;
    fn nvs_commit(handle: u32) -> i32;

    // Blob-internal pointer to the RX gain DC table
    static mut phy_rx_gain_dc_table: *mut u32;
}

// NVS constants
const NVS_READONLY: u32 = 0;
const NVS_READWRITE: u32 = 1;

// UART
const UART_CLK_FREQ: u32 = 80_000_000 / 1; // APB_CLK_FREQ
const UART_BAUDRATE: u32 = 115_200;

// ---------------------------------------------------------------------------
// Calibration data
// ---------------------------------------------------------------------------

/// RX gain DC table — persistent copy used by the PHY blob.
#[no_mangle]
pub static mut rx_gain_dc_table: [u32; 125] = [0; 125];

/// Validate RF calibration data checksum against chip ID.
unsafe fn phy_check_calibration_data(rf_cal_data: *mut u8) -> bool {
    const CHECK_NUM: usize = 26;
    const CHIP_ID_L: usize = 24;
    const CHIP_ID_H: usize = 25;

    unsafe {
        let cal_words = rf_cal_data as *mut u32;

        let chip_id_l =
            ((reg_read(0x3FF0_0058) & 0xF000) << 16) | (reg_read(0x3FF0_0054) & 0x0FFF_FFFF);
        let chip_id_h =
            (reg_read(0x3FF0_0050) & 0xFF00_0000) | (reg_read(0x3FF0_005C) & 0x00FF_FFFF);

        cal_words.add(CHIP_ID_L).write(chip_id_l);
        cal_words.add(CHIP_ID_H).write(chip_id_h);

        let mut check_sum: u32 = 0;
        for i in 0..CHECK_NUM {
            check_sum = check_sum.wrapping_add(cal_words.add(i).read());
        }

        cal_words.add(CHECK_NUM).read() != !check_sum
    }
}

#[inline(always)]
unsafe fn reg_read(addr: u32) -> u32 {
    unsafe { (addr as *const u32).read_volatile() }
}

// ---------------------------------------------------------------------------
// PHY RF init
// ---------------------------------------------------------------------------

/// Full PHY/RF initialization with calibration data.
///
/// This is the core function that sets up the radio hardware. It:
/// 1. Prepares init + calibration data buffers
/// 2. Configures RF wakeup and powerup options
/// 3. Flushes UARTs (clock change ahead)
/// 4. Calls `register_chipv6_phy` (the main blob init)
/// 5. Reads back calibration results
#[no_mangle]
pub unsafe extern "C" fn esp_phy_rf_init(
    init_data: *const u8, // esp_phy_init_data_t* (128 bytes of params)
    _mode: u32,           // esp_phy_calibration_mode_t
    cal_data: *mut u8,    // esp_phy_calibration_data_t*
    _module: u32,         // phy_rf_module_t
) -> i32 {
    unsafe {
        // Build 256-byte buffer: [128 init_data | 128 cal_data]
        let local_init_data = super::super::compat::memory::calloc(1, 256) as *mut u8;
        if local_init_data.is_null() {
            return sys::ESP_ERR_NO_MEM;
        }

        core::ptr::copy_nonoverlapping(init_data, local_init_data, 128);
        // cal_data starts at offset 0 in esp_phy_calibration_data_t.rf_cal_data
        core::ptr::copy_nonoverlapping(cal_data, local_init_data.add(128), 128);

        // Point the blob's DC table to the calibration data's table
        // (offset 128 in esp_phy_calibration_data_t = after rf_cal_data)
        let cal_dc_table = cal_data.add(128) as *mut u32;
        phy_rx_gain_dc_table = cal_dc_table;

        let cal_check = phy_check_calibration_data(cal_data)
            || phy_check_data_table(phy_rx_gain_dc_table, 125, 1) != 0;

        phy_afterwake_set_rfoption(1);

        if !cal_check {
            phy_set_powerup_option(1);
            write_data_to_rtc(cal_data);
        } else {
            phy_set_powerup_option(3);
        }
        let mut sta_mac = [0u8; 6];
        super::system::esp_efuse_mac_get_default(sta_mac.as_mut_ptr());

        uart_tx_wait_idle(0);
        uart_tx_wait_idle(1);

        let _ = register_chipv6_phy(local_init_data);

        phy_disable_agc();

        // Read back calibration results
        get_data_from_rtc(cal_data);

        // Copy RX gain DC table to persistent storage
        core::ptr::copy_nonoverlapping(cal_dc_table, rx_gain_dc_table.as_mut_ptr(), 125);
        phy_rx_gain_dc_table = rx_gain_dc_table.as_mut_ptr();

        super::super::compat::memory::free(local_init_data as *mut c_void);

        sys::ESP_OK
    }
}

/// PHY RF deinit (no-op).
#[no_mangle]
pub unsafe extern "C" fn esp_phy_rf_deinit(_module: u32) -> i32 {
    sys::ESP_OK
}

// ---------------------------------------------------------------------------
// PHY init data accessors
// ---------------------------------------------------------------------------

/// Return a pointer to the built-in PHY init data.
#[no_mangle]
pub unsafe extern "C" fn esp_phy_get_init_data() -> *const u8 {
    PHY_INIT_DATA.as_ptr()
}

/// Release PHY init data (no-op for built-in data).
#[no_mangle]
pub unsafe extern "C" fn esp_phy_release_init_data(_init_data: *const u8) {}

// ---------------------------------------------------------------------------
// NVS calibration data
// ---------------------------------------------------------------------------

const PHY_NAMESPACE: &[u8] = b"phy\0";
const PHY_CAL_DATA_KEY: &[u8] = b"cal_data\0";
const PHY_RX_GAIN_DC_TABLE_KEY: &[u8] = b"dc_table\0";

#[no_mangle]
pub unsafe extern "C" fn esp_phy_load_cal_data_from_nvs(out_cal_data: *mut u8) -> i32 {
    unsafe {
        let mut handle: u32 = 0;
        let err = nvs_open(PHY_NAMESPACE.as_ptr(), NVS_READONLY, &mut handle);
        if err != sys::ESP_OK {
            return err;
        }

        // rf_cal_data is 128 bytes at offset 0
        let mut length = 128usize;
        let err = nvs_get_blob(
            handle,
            PHY_CAL_DATA_KEY.as_ptr(),
            out_cal_data as *mut c_void,
            &mut length,
        );
        if err != sys::ESP_OK || length != 128 {
            nvs_close(handle);
            return if err != sys::ESP_OK {
                err
            } else {
                sys::ESP_ERR_INVALID_SIZE
            };
        }

        // rx_gain_dc_table is 500 bytes (125 × u32) at offset 128
        let mut length = 500usize;
        let err = nvs_get_blob(
            handle,
            PHY_RX_GAIN_DC_TABLE_KEY.as_ptr(),
            out_cal_data.add(128) as *mut c_void,
            &mut length,
        );
        nvs_close(handle);
        if err != sys::ESP_OK || length != 500 {
            return if err != sys::ESP_OK {
                err
            } else {
                sys::ESP_ERR_INVALID_SIZE
            };
        }

        sys::ESP_OK
    }
}

#[no_mangle]
pub unsafe extern "C" fn esp_phy_store_cal_data_to_nvs(cal_data: *const u8) -> i32 {
    unsafe {
        let mut handle: u32 = 0;
        let err = nvs_open(PHY_NAMESPACE.as_ptr(), NVS_READWRITE, &mut handle);
        if err != sys::ESP_OK {
            return err;
        }

        let err = nvs_set_blob(
            handle,
            PHY_CAL_DATA_KEY.as_ptr(),
            cal_data as *const c_void,
            128,
        );
        if err != sys::ESP_OK {
            nvs_close(handle);
            return err;
        }

        let err = nvs_set_blob(
            handle,
            PHY_RX_GAIN_DC_TABLE_KEY.as_ptr(),
            cal_data.add(128) as *const c_void,
            500,
        );
        if err != sys::ESP_OK {
            nvs_close(handle);
            return err;
        }

        let err = nvs_commit(handle);
        nvs_close(handle);
        err
    }
}

// ---------------------------------------------------------------------------
// Main PHY load + init entry point
// ---------------------------------------------------------------------------

/// Load calibration data from NVS and initialize the PHY.
///
/// This is called during WiFi startup. If NVS calibration data is available
/// and valid, a partial calibration is done (faster). Otherwise, full
/// calibration is performed.
#[no_mangle]
pub unsafe extern "C" fn esp_phy_load_cal_and_init(_module: u32) {
    unsafe {
        // esp_phy_calibration_data_t = rf_cal_data[128] + rx_gain_dc_table[500]
        let cal_data_size = 128 + 4 * 125; // 628 bytes
        let cal_data = super::super::compat::memory::calloc(1, cal_data_size) as *mut u8;
        if cal_data.is_null() {
            // Fatal: can't allocate calibration buffer
            loop {}
        }

        let init_data = esp_phy_get_init_data();
        if init_data.is_null() {
            loop {}
        }

        let mut mode: u32 = 0;
        let err = esp_phy_load_cal_data_from_nvs(cal_data);
        if err != sys::ESP_OK {
            mode = 2;
        }

        esp_phy_rf_init(init_data, mode, cal_data, _module);

        // Store calibration data back to NVS if we did a full cal
        if mode == 2 || err != sys::ESP_OK {
            let _ = esp_phy_store_cal_data_to_nvs(cal_data);
        }

        esp_phy_release_init_data(init_data);
        super::super::compat::memory::free(cal_data as *mut c_void);
    }
}

// ---------------------------------------------------------------------------
// VDD33 and TX power
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn esp_wifi_get_vdd33() -> u16 {
    extern "C" {
        fn phy_get_vdd33() -> u16;
    }
    unsafe {
        // byte 107 of init data must be 0xFF for VDD33 measurement
        if PHY_INIT_DATA[107] != 0xFF {
            return 0xFFFF;
        }
        let ret = phy_get_vdd33();
        if ret != 0xFFFF {
            ret * 12 / 11
        } else {
            ret
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn esp_wifi_set_max_tx_power_via_vdd33(vdd33: u16) {
    extern "C" {
        fn phy_vdd33_set_tpw(vdd33: u16);
    }
    unsafe { phy_vdd33_set_tpw(vdd33) }
}

/// `phy_printf` stub — the blob calls this for PHY debug output.
///
/// The real signature is `int phy_printf(const char *fmt, ...)` (C-variadic).
/// On Xtensa call0 ABI, extra arguments are simply ignored by the callee,
/// so a non-variadic `extern "C" fn` with the same symbol name is safe as
/// a no-op stub. The caller pushes args onto the stack; we never read them.
#[no_mangle]
pub unsafe extern "C" fn phy_printf() -> i32 {
    0
}

// ---------------------------------------------------------------------------
// Clock initialization
// ---------------------------------------------------------------------------

/// Initialize RTC and PLL clocks using PHY init data.
///
/// After this call, the APB clock runs at 80 MHz. UART baud rates are
/// reconfigured for 115200.
#[no_mangle]
pub unsafe extern "C" fn esp_phy_init_clk() {
    unsafe {
        let mut buf = [0u8; 128];
        buf.copy_from_slice(&PHY_INIT_DATA);

        uart_tx_wait_idle(0);
        uart_div_modify(0, UART_CLK_FREQ / UART_BAUDRATE);
        uart_tx_wait_idle(1);
        uart_div_modify(1, UART_CLK_FREQ / UART_BAUDRATE);

        rtc_init();
        rtc_init_clk(buf.as_ptr());
    }
}
