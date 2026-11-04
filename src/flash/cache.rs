//! Cache and interrupt exclusion for a single flash command.
//!
//! Every function here executes from IRAM, including its literal pool. The
//! transaction holds both CPU interrupts and Wi-Fi NMI masked until cache and
//! AHB access have been restored. No scheduler-owned nesting state is involved.

use crate::ram;
use core::sync::atomic::{compiler_fence, Ordering};
#[cfg(all(feature = "rt", target_arch = "xtensa"))]
use esp8266::SPI0;

const NMI_ENABLE: *mut u32 = 0x3ff0_0000 as *mut u32;
const CACHE_CTRL: *mut u32 = 0x3ff0_000c as *mut u32;
#[cfg(all(feature = "rt", target_arch = "xtensa"))]
const CACHE_TARGET: *mut u32 = 0x3ff0_0024 as *mut u32;
const SPI_CTRL: *mut u32 = 0x6000_0208 as *mut u32;
// Pinned SDK spi_register.h: SPI_EXT2(0) is base + 0xf8. Offset 0xf0
// selects EXT0 (programming timing), whose value need not return to zero.
const SPI_EXT2: *const u32 = esp8266::SPI0::PTR
    .cast::<u8>()
    .wrapping_add(0xf8)
    .cast::<u32>();
const CACHE_ENABLE: u32 = 1 << 8;
const AHB_ENABLE: u32 = 1 << 17;

/// Configure the initial boot mapping before entering flash-resident code.
#[cfg(all(feature = "rt", target_arch = "xtensa"))]
#[ram]
pub(crate) fn cache_enable(_spi: &mut SPI0, megabyte: u8) {
    // SAFETY: reset owns SPI0 and executes from IRAM with interrupts disabled.
    // The only caller selects mapping zero. No runtime/SDK state exists yet.
    unsafe {
        let mapping = (u32::from(megabyte / 2) << 16) | (u32::from(megabyte % 2) << 25);
        SPI_CTRL.write_volatile(SPI_CTRL.read_volatile() | AHB_ENABLE);
        CACHE_CTRL.write_volatile((CACHE_CTRL.read_volatile() & !0x0707_0000) | mapping);
        CACHE_TARGET.write_volatile(CACHE_TARGET.read_volatile() | (1 << 3));
        while CACHE_CTRL.read_volatile() & CACHE_ENABLE == 0 {
            CACHE_CTRL.write_volatile(CACHE_CTRL.read_volatile() | CACHE_ENABLE);
        }
    }
}

pub(super) struct Paused {
    #[cfg(target_arch = "xtensa")]
    ps: u32,
    nmi: u32,
    cache: u32,
    ahb: u32,
}

impl Paused {
    /// # Safety
    /// Caller must execute entirely in IRAM and keep command buffers in DRAM
    /// until this value is dropped. It must not call application code or panic.
    #[ram]
    pub(super) unsafe fn new() -> Self {
        #[cfg(target_arch = "xtensa")]
        let ps: u32;
        #[cfg(target_arch = "xtensa")]
        unsafe {
            core::arch::asm!("rsil {0}, 3", out(reg) ps, options(nostack));
        }
        compiler_fence(Ordering::Acquire);
        // SAFETY: the caller exclusively owns this cache-off interval; masking
        // NMI before disabling cache prevents SDK flash/radio entry in between.
        unsafe {
            let nmi = NMI_ENABLE.read_volatile();
            NMI_ENABLE.write_volatile(0);
            let cache = CACHE_CTRL.read_volatile() & CACHE_ENABLE;
            let ahb = SPI_CTRL.read_volatile() & AHB_ENABLE;
            CACHE_CTRL.write_volatile(CACHE_CTRL.read_volatile() & !CACHE_ENABLE);
            while SPI_EXT2.read_volatile() != 0 {}
            SPI_CTRL.write_volatile(SPI_CTRL.read_volatile() & !AHB_ENABLE);
            // Match the pinned SDK's live Cache_Read_Disable_2 sequence;
            // it does not toggle or poll the undocumented cache bits 0/1.
            Self {
                #[cfg(target_arch = "xtensa")]
                ps,
                nmi,
                cache,
                ahb,
            }
        }
    }
}

impl Drop for Paused {
    #[ram]
    fn drop(&mut self) {
        // SAFETY: the command has completed and no application or SDK code ran
        // during exclusion. Restore cache before permitting any flash fetch.
        unsafe {
            SPI_CTRL.write_volatile((SPI_CTRL.read_volatile() & !AHB_ENABLE) | self.ahb);
            CACHE_CTRL.write_volatile((CACHE_CTRL.read_volatile() & !CACHE_ENABLE) | self.cache);
            if self.cache != 0 {
                while CACHE_CTRL.read_volatile() & CACHE_ENABLE == 0 {}
            }
            NMI_ENABLE.write_volatile(self.nmi);
        }
        compiler_fence(Ordering::Release);
        #[cfg(target_arch = "xtensa")]
        unsafe {
            core::arch::asm!("wsr {0}, ps", "rsync", in(reg) self.ps, options(nostack));
        }
    }
}
