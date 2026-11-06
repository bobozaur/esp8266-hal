//! PHY initialization and RF calibration.
//!
//! The pinned native radio attaches once per boot. Calibration uses temporary
//! workspace; only its live DC table survives. The HAL reset path owns PLL setup.

use core::ptr;
use esp8266_wifi_sys as sys;
use portable_atomic::{AtomicI32, Ordering};

// The native load-calibration hook returns void. Reserve both temporary buffers
// before entering native initialization, where an OOM cannot be propagated.
const CALIBRATION_BYTES: usize = 128 + 4 * 125;
const RF_INIT_BYTES: usize = 256;
static mut WORKSPACE: *mut u8 = ptr::null_mut();
static INIT_RESULT: AtomicI32 = AtomicI32::new(sys::ESP_OK);

pub(crate) unsafe fn prepare_workspace() -> i32 {
    unsafe {
        INIT_RESULT.store(sys::ESP_OK, Ordering::Release);
        WORKSPACE =
            super::super::compat::memory::calloc(1, CALIBRATION_BYTES + RF_INIT_BYTES).cast();
        if WORKSPACE.is_null() {
            sys::ESP_ERR_NO_MEM
        } else {
            sys::ESP_OK
        }
    }
}

pub(crate) fn initialization_result() -> i32 {
    INIT_RESULT.load(Ordering::Acquire)
}

/// Called only after completed initialization or proven native cleanup.
pub(crate) unsafe fn release_workspace() {
    unsafe {
        let workspace = WORKSPACE;
        WORKSPACE = ptr::null_mut();
        super::super::compat::memory::free(workspace.cast());
    }
}

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
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // byte 107: default 3.3 V reference
    0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, // byte 114: RF calibration default
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
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
    fn uart_tx_wait_idle(uart_no: u8);

    // Blob-internal pointer to the RX gain DC table
    static mut phy_rx_gain_dc_table: *mut u32;
}

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
unsafe fn initialize_phy(cal_data: *mut u8) -> i32 {
    unsafe {
        // Build 256-byte buffer: [128 init_data | 128 cal_data]
        if WORKSPACE.is_null() {
            return sys::ESP_ERR_INVALID_STATE;
        }
        let local_init_data = WORKSPACE.add(CALIBRATION_BYTES);

        core::ptr::copy_nonoverlapping(PHY_INIT_DATA.as_ptr(), local_init_data, 128);
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

        sys::ESP_OK
    }
}

/// Initialize the PHY on the native radio's permanent first-attach path.
/// There is no persisted calibration store; the reserved workspace starts zeroed.
#[no_mangle]
pub unsafe extern "C" fn esp_phy_load_cal_and_init(_module: u32) {
    unsafe {
        let result = initialize_phy(WORKSPACE);
        INIT_RESULT.store(result, Ordering::Release);
        if result == sys::ESP_OK {
            // The PHY retained only the permanent DC table. Release temporary
            // buffers before native initialization creates its runtime tasks.
            release_workspace();
        }
    }
}
