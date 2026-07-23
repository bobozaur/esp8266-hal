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

/// Write a UTF-8 string directly to UART0.
#[inline]
pub fn write_str(s: &str) {
    write_bytes(s.as_bytes());
}

/// Write a trailing CRLF.
#[inline]
pub fn write_newline() {
    write_bytes(b"\r\n");
}

/// Convert a nibble to ASCII hex.
#[inline]
pub const fn hex_nibble(n: u8) -> u8 {
    match n & 0x0f {
        0..=9 => b'0' + (n & 0x0f),
        _ => b'a' + ((n & 0x0f) - 10),
    }
}

/// Write a `u32` as exactly 8 lowercase hex digits.
#[inline]
pub fn write_hex_u32(value: u32) {
    write_byte(hex_nibble((value >> 28) as u8));
    write_byte(hex_nibble((value >> 24) as u8));
    write_byte(hex_nibble((value >> 20) as u8));
    write_byte(hex_nibble((value >> 16) as u8));
    write_byte(hex_nibble((value >> 12) as u8));
    write_byte(hex_nibble((value >> 8) as u8));
    write_byte(hex_nibble((value >> 4) as u8));
    write_byte(hex_nibble(value as u8));
}

/// Write a `u8` as exactly 2 lowercase hex digits.
#[inline]
pub fn write_hex_u8(value: u8) {
    write_byte(hex_nibble(value >> 4));
    write_byte(hex_nibble(value));
}

/// Write a `u32` in decimal without using formatting machinery.
#[inline]
pub fn write_dec_u32(mut value: u32) {
    let mut digits = [0u8; 10];
    let mut cursor = digits.len();

    loop {
        cursor -= 1;
        digits[cursor] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }

    write_bytes(&digits[cursor..]);
}

/// Write an IPv4 address in dotted-decimal notation.
#[inline]
pub fn write_ipv4_octets(octets: [u8; 4]) {
    write_dec_u32(octets[0] as u32);
    write_byte(b'.');
    write_dec_u32(octets[1] as u32);
    write_byte(b'.');
    write_dec_u32(octets[2] as u32);
    write_byte(b'.');
    write_dec_u32(octets[3] as u32);
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
