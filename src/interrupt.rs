#[cfg(feature = "wifi")]
use core::arch::asm;

// AP/UART hardware checks used 432 bytes before heavier packet callbacks.
// Leave room for those paths and application handlers without borrowing a task stack.
#[cfg(feature = "wifi")]
const WIFI_INTERRUPT_STACK_BYTES: usize = 1024;

/// Level-one interrupt stack followed by the saved pre-interrupt stack pointer.
#[cfg(feature = "wifi")]
#[repr(C, align(16))]
struct WifiInterruptStack([u8; WIFI_INTERRUPT_STACK_BYTES + 4]);

#[cfg(feature = "wifi")]
#[used]
#[no_mangle]
#[link_section = ".bss"]
static mut _chip_interrupt_stk: WifiInterruptStack =
    WifiInterruptStack([0; WIFI_INTERRUPT_STACK_BYTES + 4]);

use xtensa_lx_rt::exception::Context;
#[cfg(not(feature = "wifi"))]
use xtensa_lx_rt::interrupt;

use crate::ram;

#[repr(u8)]
pub enum InterruptType {
    SLC = 1,
    SPI = 2,
    GPIO = 4,
    UART = 5,
    CCOMPARE = 6,
    SOFT = 7,
    WDT = 8,
    TIMER1 = 9,
}

impl InterruptType {
    const fn mask(self) -> u32 {
        1 << self as u8
    }
}

/// Default for application sources without a static handler. The level-one
/// runtime fallback has a different signature and cannot serve this ABI.
#[doc(hidden)]
#[no_mangle]
#[ram]
pub extern "C" fn __esp8266_hal_default_interrupt() {}

extern "C" {
    // Static application handlers exported by #[interrupt], or linker defaults.
    fn __slc_interrupt();
    fn __spi_interrupt();
    fn __gpio_interrupt();
    fn __uart_interrupt();
    fn __ccompare_interrupt();
    fn __soft_interrupt();
    fn __wdt_interrupt();
    fn __timer1_interrupt();
}

/// Call application handlers after the dispatcher clears CPU pending bits.
///
/// # Safety
/// Call only from the serialized level-one interrupt dispatcher, for sources
/// which have no registered SDK handler. Handlers must clear their peripheral
/// interrupt condition before returning.
#[ram]
pub(crate) unsafe fn dispatch_app_interrupts(mask: u32) {
    unsafe {
        if mask & InterruptType::SLC.mask() != 0 {
            __slc_interrupt();
        }
        if mask & InterruptType::SPI.mask() != 0 {
            __spi_interrupt();
        }
        if mask & InterruptType::GPIO.mask() != 0 {
            __gpio_interrupt();
        }
        if mask & InterruptType::UART.mask() != 0 {
            __uart_interrupt();
        }
        if mask & InterruptType::CCOMPARE.mask() != 0 {
            __ccompare_interrupt();
        }
        if mask & InterruptType::SOFT.mask() != 0 {
            __soft_interrupt();
        }
        if mask & InterruptType::WDT.mask() != 0 {
            __wdt_interrupt();
        }
        if mask & InterruptType::TIMER1.mask() != 0 {
            __timer1_interrupt();
        }
    }
}

#[cfg(not(feature = "wifi"))]
#[interrupt]
#[ram]
fn interrupt_trampoline(level: u32, frame: Context) {
    let _ = (level, frame);
    let mask = xtensa_lx::interrupt::get() & xtensa_lx::interrupt::get_mask();
    // SAFETY: the runtime invokes this once for the current level-one frame.
    unsafe {
        xtensa_lx::interrupt::clear(mask);
        dispatch_app_interrupts(mask);
    }
}

#[cfg(feature = "wifi")]
#[naked]
#[no_mangle]
#[link_section = ".rwtext"]
unsafe extern "C" fn __level_1_interrupt() {
    asm!(
        "movi a4, 0x23",
        "wsr.ps a4",
        "rsync",
        "movi a4, _chip_interrupt_stk + {stack_bytes}",
        "s32i a1, a4, 0",
        "mov a1, a4",
        "call0 __level_1_interrupt_body",
        "movi a4, g_level1_restore_sp",
        "l32i a5, a4, 0",
        "beqz a5, 1f",
        "movi a6, 0",
        "s32i a6, a4, 0",
        "j 2f",
        "1:",
        "movi a4, _chip_interrupt_stk + {stack_bytes}",
        "l32i a5, a4, 0",
        "2:",
        "mov a2, a5",
        "call0 commit_current_task_post_restore_stack_top",
        "mov a1, a2",
        "call0 restore_context",
        "l32i a0, a1, 0x04",
        "wsr.ps a0",
        "l32i a0, a1, 0x00",
        "wsr.epc1 a0",
        "l32i a0, a1, 0x08",
        "l32i a1, a1, 0x0c",
        "rsync",
        ".byte 0x00, 0x30, 0x00",
        stack_bytes = const WIFI_INTERRUPT_STACK_BYTES,
        options(noreturn)
    );
}

#[cfg(feature = "wifi")]
#[no_mangle]
#[ram]
unsafe extern "C" fn __level_1_interrupt_body(_level: u32, frame: &mut Context) {
    unsafe {
        let frame_ptr = frame as *mut Context as *mut u8;

        crate::wifi::compat::task::note_interrupt_frame(frame_ptr);
        crate::wifi::compat::port::isr_dispatch();
        crate::wifi::compat::port::apply_pending_context_switch();
    }
}

/// Unmask a CPU interrupt source, returning the previous mask.
///
/// # Safety
/// The source must have a handler and must not be owned by the SDK runtime.
/// Do not unmask interrupts inside an interrupt-free critical section.
/// The runtime permanently reserves its registered interrupt sources.
pub unsafe fn enable_interrupt(ty: InterruptType) -> u32 {
    // SAFETY: the caller establishes handler ownership and exclusion requirements.
    unsafe { xtensa_lx::interrupt::enable_mask(ty.mask()) }
}

/// Mask a CPU interrupt source, returning the previous mask.
///
/// Disabling a runtime-owned source prevents that runtime from making progress.
pub fn disable_interrupt(ty: InterruptType) -> u32 {
    xtensa_lx::interrupt::disable_mask(ty.mask())
}
