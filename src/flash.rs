//! SPI0 flash commands with bounded DRAM staging and cache-safe execution.
//!
//! Addresses are byte offsets in the flash chip, not mapped CPU addresses.
//! Operations must target the installed chip's capacity and must preserve any
//! executing firmware, application storage and device calibration partitions.
//! Erase/program briefly suspend interrupts, including Wi-Fi NMI; callers should
//! account for this latency when scheduling radio or other time-sensitive work.
//!
//! Readback is provided by [`ESPFlash::read`]. Coherence of independently mapped
//! flash accesses after mutation is outside this interface's guarantees.

use crate::ram;
use esp8266::SPI0;

mod cache;
#[cfg(all(feature = "rt", target_arch = "xtensa"))]
pub(crate) use cache::cache_enable;

pub trait SPI0Ext {
    fn flash(self) -> ESPFlash;
}
impl SPI0Ext for SPI0 {
    fn flash(self) -> ESPFlash {
        ESPFlash { _spi: self }
    }
}

pub struct ESPFlash {
    _spi: SPI0,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FlashError {
    AddressOutOfRange,
    UnalignedSector,
}

const ADDRESS_LIMIT: u32 = 1 << 24;
const SECTOR_SIZE: usize = 4096;
const PAGE_SIZE: usize = 256;
const FIFO_BYTES: usize = 64;
const READ_BYTES: usize = 60; // Preserve the verified full-FIFO read workaround.
const READ: u32 = 1 << 31;
const PROGRAM: u32 = 1 << 25;
const ERASE_SECTOR: u32 = 1 << 24;
const ERASE_CHIP: u32 = 1 << 22;

fn check_range(address: u32, length: usize) -> Result<(), FlashError> {
    let length = u32::try_from(length).map_err(|_| FlashError::AddressOutOfRange)?;
    if address > ADDRESS_LIMIT || length > ADDRESS_LIMIT - address {
        Err(FlashError::AddressOutOfRange)
    } else {
        Ok(())
    }
}

fn program_chunk(address: u32, remaining: usize) -> usize {
    remaining
        .min(FIFO_BYTES)
        .min(PAGE_SIZE - address as usize % PAGE_SIZE)
}

impl ESPFlash {
    /// Erase successive 4 KiB sectors beginning at an aligned byte offset.
    ///
    /// # Safety
    /// The entire range must fit the installed chip and be exclusively owned
    /// writable storage, outside all code/data used by this booted program.
    /// No live references may point into its mapped flash contents. Preserve
    /// device calibration/configuration partitions as required by the board.
    /// Call from ordinary task context with the flash cache enabled, never NMI.
    pub unsafe fn erase_sectors(&mut self, address: u32, count: usize) -> Result<(), FlashError> {
        if address as usize % SECTOR_SIZE != 0 {
            return Err(FlashError::UnalignedSector);
        }
        let bytes = count
            .checked_mul(SECTOR_SIZE)
            .ok_or(FlashError::AddressOutOfRange)?;
        check_range(address, bytes)?;
        let mut words = [0; 16];
        for offset in (0..bytes).step_by(SECTOR_SIZE) {
            // SAFETY: range is checked; buffer lives in DRAM; each command is
            // fully serialized against SDK and interrupt flash users.
            unsafe {
                execute(ERASE_SECTOR, address + offset as u32, &mut words, 0);
            }
        }
        Ok(())
    }

    /// Erase the entire chip, including firmware and device-specific data.
    ///
    /// # Safety
    /// The whole chip must be exclusively owned. No code or data needed for
    /// subsequent execution may reside on it, including return addresses,
    /// literals and interrupt handlers. No references may point into mapped
    /// flash. Calibration/configuration data will also be erased. Call with
    /// cache enabled from ordinary task context, never NMI.
    pub unsafe fn erase_all(&mut self) -> Result<(), FlashError> {
        let mut words = [0; 16];
        unsafe {
            execute(ERASE_CHIP, 0, &mut words, 0);
        }
        Ok(())
    }

    /// Program bytes without crossing a flash page in a single command.
    /// Source data may be flash-resident: it is staged while cache is enabled.
    ///
    /// # Safety
    /// The entire destination must fit the installed chip and be exclusively
    /// owned writable storage outside all code/data used by this booted program.
    /// No live references, including the source slice, may point into the
    /// changed mapped flash range. Preserve board calibration/configuration.
    /// Call from ordinary task context with cache enabled, never NMI.
    pub unsafe fn write_bytes(
        &mut self,
        mut address: u32,
        mut data: &[u8],
    ) -> Result<(), FlashError> {
        check_range(address, data.len())?;
        while !data.is_empty() {
            let length = program_chunk(address, data.len());
            let mut words = [0u32; 16];
            for (i, &byte) in data[..length].iter().enumerate() {
                words[i / 4] |= u32::from(byte) << ((i % 4) * 8);
            }
            // SAFETY: command length <=64 and bounded by the destination page;
            // all bytes to be read while cache is off are in this DRAM buffer.
            unsafe {
                execute(PROGRAM, address, &mut words, length);
            }
            address += length as u32;
            data = &data[length..];
        }
        Ok(())
    }

    pub fn read(&mut self, mut address: u32, mut data: &mut [u8]) -> Result<(), FlashError> {
        check_range(address, data.len())?;
        while !data.is_empty() {
            let length = data.len().min(READ_BYTES);
            let mut words = [0; 16];
            // SAFETY: checked address and bounded DRAM destination. User buffer
            // access occurs only after execute has restored the cache.
            unsafe {
                execute(READ, address, &mut words, length);
            }
            for (i, byte) in data[..length].iter_mut().enumerate() {
                *byte = (words[i / 4] >> ((i % 4) * 8)) as u8;
            }
            address += length as u32;
            data = &mut data[length..];
        }
        Ok(())
    }
}

/// One command with no calls to flash-resident code or access to caller data.
///
/// # Safety
/// `words` must be in DRAM, length <=64, and address/command must be valid.
/// Do not call from NMI or while the flash cache is already disabled.
#[ram]
unsafe fn execute(command: u32, address: u32, words: &mut [u32; 16], length: usize) {
    const CMD: *mut u32 = 0x6000_0200 as *mut u32;
    const ADDR: *mut u32 = 0x6000_0204 as *mut u32;
    const STATUS: *const u32 = 0x6000_0210 as *const u32;
    const FIFO: *mut u32 = 0x6000_0240 as *mut u32;
    // SAFETY: every operation until guard destruction uses only registers and
    // bounded raw access to the provided DRAM array. No allocation or panics.
    unsafe {
        let _paused = cache::Paused::new();
        if command != READ {
            ADDR.write_volatile(0);
            CMD.write_volatile(1 << 30); // Write-enable latch is per command.
            while CMD.read_volatile() != 0 {}
        }
        if command == PROGRAM {
            let mut index = 0;
            let count = length.wrapping_add(3) >> 2;
            while index < count {
                FIFO.add(index)
                    .write_volatile(words.as_ptr().add(index).read());
                index = index.wrapping_add(1);
            }
        }
        ADDR.write_volatile(address | ((length as u32) << 24));
        CMD.write_volatile(command);
        while CMD.read_volatile() != 0 {}
        if command == READ {
            let mut index = 0;
            let count = length.wrapping_add(3) >> 2;
            while index < count {
                words
                    .as_mut_ptr()
                    .add(index)
                    .write(FIFO.add(index).read_volatile());
                index = index.wrapping_add(1);
            }
        } else {
            loop {
                ADDR.write_volatile(0);
                CMD.write_volatile(1 << 27); // Read status until WIP clears.
                while CMD.read_volatile() != 0 {}
                if STATUS.read_volatile() & 1 == 0 {
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn range_rejects_truncation_and_overflow() {
        assert_eq!(check_range(0x00ff_ffff, 1), Ok(()));
        assert_eq!(check_range(ADDRESS_LIMIT, 0), Ok(()));
        assert_eq!(
            check_range(ADDRESS_LIMIT, 1),
            Err(FlashError::AddressOutOfRange)
        );
        assert_eq!(check_range(u32::MAX, 1), Err(FlashError::AddressOutOfRange));
        assert_eq!(
            check_range(0, usize::MAX),
            Err(FlashError::AddressOutOfRange)
        );
    }
    #[test]
    fn commands_respect_destination_pages_and_fifo_capacity() {
        for start in 0..PAGE_SIZE as u32 {
            let mut address = start;
            let mut remaining = PAGE_SIZE * 3 + 5;
            while remaining != 0 {
                let chunk = program_chunk(address, remaining);
                assert!(chunk > 0 && chunk <= FIFO_BYTES && chunk <= remaining);
                assert_eq!(
                    address / PAGE_SIZE as u32,
                    (address + chunk as u32 - 1) / PAGE_SIZE as u32
                );
                address += chunk as u32;
                remaining -= chunk;
            }
            assert_eq!(address - start, (PAGE_SIZE * 3 + 5) as u32);
        }
    }
}
