//! Minimal UART0 logging via direct FIFO register writes.
//!
//! The ROM bootloader already configures UART0 at 74880 baud, so this works
//! without any pin setup. Open your serial monitor at **74880 baud**.
//!
//! # Usage
//!
//! ```rust,no_run
//! use esp8266_hal::println;
//!
//! println!("Hello from ESP8266!");
//! println!("x = {}", 42);
//! ```

const UART0_FIFO: *mut u32 = 0x6000_0000 as *mut u32;
const UART0_STATUS: *const u32 = 0x6000_001C as *const u32;

/// Write a single byte to UART0, blocking until the TX FIFO has space.
#[inline(never)]
pub fn write_byte(b: u8) {
    unsafe {
        // Spin until TX FIFO has room (count < 128).
        while (UART0_STATUS.read_volatile() >> 16) & 0xFF >= 128 {}
        UART0_FIFO.write_volatile(b as u32);
    }
}

/// Write a byte slice to UART0.
pub fn write_bytes(s: &[u8]) {
    for &b in s {
        write_byte(b);
    }
}

/// Best-effort single-byte trace write — tries briefly, then drops.
///
/// Reduced spin count (was 16384) to avoid stalling the tick ISR when the
/// UART FIFO fills up from trace output.
pub fn trace_byte(b: u8) {
    unsafe {
        for _ in 0..8 {
            if (UART0_STATUS.read_volatile() >> 16) & 0xFF < 128 {
                UART0_FIFO.write_volatile(b as u32);
                break;
            }
            core::hint::spin_loop();
        }
    }
}

/// Adapter for `core::fmt::Write` so `write!()` / `writeln!()` work.
pub struct UartWriter;

impl core::fmt::Write for UartWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        write_bytes(s.as_bytes());
        Ok(())
    }
}

/// Print to UART0 (no newline).
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let _ = write!($crate::logger::UartWriter, $($arg)*);
    }};
}

/// Print to UART0 with a trailing `\r\n`.
#[macro_export]
macro_rules! println {
    () => { $crate::print!("\r\n") };
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let _ = write!($crate::logger::UartWriter, $($arg)*);
        $crate::logger::write_bytes(b"\r\n");
    }};
}
