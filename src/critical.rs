use core::arch::asm;
use core::sync::atomic::{compiler_fence, Ordering};

struct XtensaCriticalSection;

critical_section::set_impl!(XtensaCriticalSection);

unsafe impl critical_section::Impl for XtensaCriticalSection {
    unsafe fn acquire() -> critical_section::RawRestoreState {
        let saved: u32;
        unsafe {
            asm!("rsil {0}, 3", out(reg) saved, options(nostack));
        }
        compiler_fence(Ordering::Acquire);
        saved as critical_section::RawRestoreState
    }

    unsafe fn release(token: critical_section::RawRestoreState) {
        compiler_fence(Ordering::Release);
        unsafe {
            asm!(
                "wsr {0}, ps",
                "rsync",
                in(reg) token as u32,
                options(nostack)
            );
        }
    }
}
