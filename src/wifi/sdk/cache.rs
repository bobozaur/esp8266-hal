//! IRAM-placed flash cache control functions.
//!
//! Called by `libphy`'s `fix_cache_bug` while the flash cache is disabled.
//! Both the function code AND its literal pool must be in IRAM.
//!
//! Replaces `sdk_overrides/spi_flash_raw.c`.

use crate::ram;

// Register addresses verified against SDK headers:
//   eagle_soc.h:206  CACHE_FLASH_CTRL_REG = 0x3ff00000 + 0x0c
//   eagle_soc.h:207  CACHE_READ_EN_BIT = BIT8
//   spi_register.h:221  SPI_ENABLE_AHB = BIT17
//   spi_register.h:23   REG_SPI_BASE(0) = 0x60000200
//   spi_register.h:42   SPI_CTRL(0) = REG_SPI_BASE(0) + 0x8
const CACHE_FLASH_CTRL_REG: u32 = 0x3FF0_000C;
const CACHE_READ_EN_BIT: u32 = 1 << 8;
const SPI_EXT2_ADDR: u32 = 0x6000_02F0; // REG_SPI_BASE(0) + 0xF0
const SPI_CTRL_ADDR: u32 = 0x6000_0208; // SPI_CTRL(0)
const SPI_ENABLE_AHB: u32 = 1 << 17;

/// Disable flash read cache. Waits for SPI idle, then disables AHB.
#[no_mangle]
#[ram]
pub unsafe extern "C" fn Cache_Read_Disable_2() {
    unsafe {
        let reg = CACHE_FLASH_CTRL_REG as *mut u32;
        let spi_ext2 = SPI_EXT2_ADDR as *const u32;
        let spi_ctrl = SPI_CTRL_ADDR as *mut u32;

        // Clear cache-read-enable bit
        reg.write_volatile(reg.read_volatile() & !CACHE_READ_EN_BIT);
        // Wait for SPI idle
        while spi_ext2.read_volatile() != 0 {}
        // Disable AHB access to SPI flash
        spi_ctrl.write_volatile(spi_ctrl.read_volatile() & !SPI_ENABLE_AHB);
    }
}

/// Enable flash read cache.
#[no_mangle]
#[ram]
pub unsafe extern "C" fn Cache_Read_Enable_2() {
    unsafe {
        let spi_ctrl = SPI_CTRL_ADDR as *mut u32;
        let reg = CACHE_FLASH_CTRL_REG as *mut u32;

        // Re-enable AHB access
        spi_ctrl.write_volatile(spi_ctrl.read_volatile() | SPI_ENABLE_AHB);
        // Set cache-read-enable bit
        reg.write_volatile(reg.read_volatile() | CACHE_READ_EN_BIT);
    }
}

/// Alias used by some SDK code paths.
#[no_mangle]
#[ram]
pub unsafe extern "C" fn Cache_Read_Enable_New() {
    unsafe { Cache_Read_Enable_2() }
}
