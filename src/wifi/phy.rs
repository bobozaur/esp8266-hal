//! PHY (physical layer) initialization for the ESP8266 WiFi radio.
//!
//! Adapted from the ESP32's `esp-phy` crate approach: call the closed-source
//! blob functions directly from Rust, without going through the SDK's C
//! wrapper functions that require a full C runtime.
//!
//! The blob functions live in `libphy.a`, `librtc.a`, and `libcore.a` — all
//! linked by `esp8266-wifi-sys`.

use crate::ram;
use esp8266_wifi_sys as sys;

// NMI handler: provided by nmi_handler.S in esp8266-wifi-sys, which overrides
// xtensa-lx-rt's default handler and redirects to wDev_ProcessFiq (the WiFi
// blob's NMI processing). This is essential for register_chipv6_phy's sleep/wake
// cycle to complete.

// ---------------------------------------------------------------------------
// Blob function declarations
// ---------------------------------------------------------------------------

extern "C" {
    fn chip_boot();
    fn rtc_init() -> i32;
    fn rtc_init_clk(init_data: *const u8);
    fn register_chipv6_phy(init_data: *const u8) -> i32;
    fn phy_afterwake_set_rfoption(option: u8);
    fn phy_set_powerup_option(option: u8);
    fn phy_disable_agc();
    fn write_data_to_rtc(cal_data: *const u8);
    fn get_data_from_rtc(cal_data: *mut u8);
}

// ---------------------------------------------------------------------------
// Default PHY init data (128 bytes)
//
// From ESP8266_RTOS_SDK/components/esp8266/include/esp_private/phy_init_data.h
// with CONFIG_ESP8266_PHY_MAX_WIFI_TX_POWER=20 (default) and 26MHz crystal.
// ---------------------------------------------------------------------------

/// Default PHY init data for 26 MHz crystal, 20 dBm max TX power.
#[rustfmt::skip]
const PHY_INIT_DATA_DEFAULT: [u8; 128] = [
    0x05, 0x00, 0x04, 0x02, 0x05, 0x05, 0x05, 0x02,
    0x05, 0x00, 0x04, 0x05, 0x05, 0x04, 0x05, 0x05,
    0x04, 0xfe, 0xfd, 0xff, 0xf0, 0xf0, 0xf0, 0xe0,
    0xe0, 0xe0, 0xe1, 0x0a, 0xff, 0xff, 0xf8, 0x00,
    0xf8, 0xf8,
    0x50, // max_tx_power * 4 capped to 0x52 (20*4=80=0x50)
    0x4e, 0x4a, 0x44, 0x40, 0x38,
    0x00, 0x00, 0x01, 0x01, 0x02, 0x03, 0x04, 0x05,
    0x01, // byte 48: XTAL flag (1 = 26MHz crystal)
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
// UART helpers (needed because clock changes affect baud rate)
// ---------------------------------------------------------------------------

const UART0_BASE: u32 = 0x6000_0000;
const UART1_BASE: u32 = 0x6000_F000;
const UART_STATUS_OFF: u32 = 0x1C;
const UART_CLKDIV_OFF: u32 = 0x14;
const APB_CLK_FREQ: u32 = 80_000_000;
const UART_BAUD: u32 = 115_200;

/// Must be in IRAM — called around clock changes that may disrupt flash cache.
#[ram]
unsafe fn uart_tx_wait_idle(uart_base: u32) {
    unsafe {
        let status = (uart_base + UART_STATUS_OFF) as *const u32;
        while (status.read_volatile() >> 16) & 0xFF != 0 {}
    }
}

/// Write a single byte to UART FIFO (blocks until space available).
/// Must be in IRAM.
#[ram]
unsafe fn uart_write_byte(uart_base: u32, byte: u8) {
    unsafe {
        let status = (uart_base + UART_STATUS_OFF) as *const u32;
        let fifo = uart_base as *mut u32;
        while (status.read_volatile() >> 16) & 0xFF >= 128 {}
        fifo.write_volatile(byte as u32);
    }
}

/// Must be in IRAM — called immediately after clock change.
#[ram]
unsafe fn uart_set_baud(uart_base: u32, baud: u32) {
    unsafe {
        let clkdiv = (uart_base + UART_CLKDIV_OFF) as *mut u32;
        clkdiv.write_volatile(APB_CLK_FREQ / baud);
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the RTC and PLL clocks using the PHY blob functions.
///
/// After this call, the APB clock runs at 80 MHz. UART baud rate divisors
/// are reconfigured for 115200 baud.
///
/// # Safety
///
/// Must be called early in boot, before any WiFi operations.
/// Reconfigure UART baud rates after PLL clock change.
/// Does NOT call rtc_init — that was already done by esp_phy_init_clk in hardware_init.
#[ram]
pub unsafe fn init_clocks() {
    unsafe {
        uart_tx_wait_idle(UART0_BASE);
        uart_set_baud(UART0_BASE, UART_BAUD);
        uart_tx_wait_idle(UART1_BASE);
        uart_set_baud(UART1_BASE, UART_BAUD);
    }
}

/// Perform full PHY/RF initialization.
///
/// Calls the closed-source blob function `register_chipv6_phy` with default
/// init data and zeroed calibration data (triggers full RF calibration).
///
/// # Safety
///
/// Must be called after [`init_clocks`]. The heap must be initialized.
pub unsafe fn init_rf() {
    unsafe {
        // Build 256-byte buffer: [128 init_data | 128 cal_data (zeros = full cal)]
        let mut buf = [0u8; 256];
        buf[..128].copy_from_slice(&PHY_INIT_DATA_DEFAULT);
        // cal_data is all zeros → full calibration will be performed.

        crate::wifi_debugln!("phy: rf init start");

        // Set RF wakeup option.
        phy_afterwake_set_rfoption(1);
        crate::wifi_debugln!("phy: afterwake done");

        // Full calibration (option 3 = cal data invalid → recalibrate).
        phy_set_powerup_option(3);
        crate::wifi_debugln!("phy: powerup option done");

        // Flush UARTs — register_chipv6_phy may touch clock config.
        uart_tx_wait_idle(UART0_BASE);
        uart_tx_wait_idle(UART1_BASE);

        // The main blob call — configures all RF hardware.
        crate::wifi_debugln!("phy: register_chipv6_phy...");
        register_chipv6_phy(buf.as_ptr());
        crate::wifi_debugln!("phy: register_chipv6_phy done");

        // Disable AGC after init (SDK does this).
        phy_disable_agc();
        crate::wifi_debugln!("phy: agc disabled");

        // Read back calibration results from RTC memory.
        let mut cal_out = [0u8; 628];
        get_data_from_rtc(cal_out.as_mut_ptr());
        crate::wifi_debugln!("phy: rf init complete");
    }
}

/// Get the MAC address from efuse registers.
pub fn get_default_mac() -> [u8; 6] {
    // The SDK reads MAC from efuse via esp_efuse_mac_get_default.
    // We can read it directly from the efuse registers or use the sys binding.
    let mut mac = [0u8; 6];
    unsafe {
        sys::esp_read_mac(mac.as_mut_ptr(), sys::ESP_MAC_WIFI_STA);
    }
    mac
}
