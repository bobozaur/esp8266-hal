//! Single-core exclusion for Rust state shared with interrupts.
//!
//! Wi-Fi NMI paths allocate from the same application heap as tasks. Masking
//! ordinary CPU interrupts alone cannot protect that heap. Keep these critical
//! sections bounded and never wait for an interrupt while holding one.

use core::arch::asm;
use core::sync::atomic::{compiler_fence, Ordering};

#[cfg(feature = "wifi")]
const NMI_ENABLE: *mut u32 = 0x3ff0_0000 as *mut u32;

// These are the exact Rust-ABI entrypoints emitted by critical_section::set_impl!.
// Define them directly so both entrypoints and their literal pools stay in IRAM
// even without inlining; the upstream macro cannot attach section attributes.
#[no_mangle]
#[inline(always)]
#[link_section = ".rwtext"]
unsafe fn _critical_section_1_0_acquire() -> critical_section::RawRestoreState {
    let saved_ps: u32;
    unsafe {
        asm!("rsil {0}, 3", out(reg) saved_ps, options(nostack));
    }
    #[cfg(feature = "wifi")]
    let saved_nmi = unsafe {
        let saved = NMI_ENABLE.read_volatile();
        NMI_ENABLE.write_volatile(0);
        asm!("memw", options(nostack));
        saved
    };
    #[cfg(not(feature = "wifi"))]
    let saved_nmi = 0u32;
    compiler_fence(Ordering::Acquire);
    (u64::from(saved_nmi) << 32) | u64::from(saved_ps)
}

#[no_mangle]
#[inline(always)]
#[link_section = ".rwtext"]
unsafe fn _critical_section_1_0_release(token: critical_section::RawRestoreState) {
    compiler_fence(Ordering::Release);
    #[cfg(feature = "wifi")]
    unsafe {
        // Commit protected writes before NMI can observe them, then restore
        // the complete register, including an outer section's zero. NMI
        // context must preserve that same disabled state without skipping.
        asm!("memw", options(nostack));
        NMI_ENABLE.write_volatile((token >> 32) as u32);
        asm!("memw", options(nostack));
    }
    unsafe {
        asm!(
            "wsr {0}, ps",
            "rsync",
            in(reg) token as u32,
            options(nostack)
        );
    }
}
