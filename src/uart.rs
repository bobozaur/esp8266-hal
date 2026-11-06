//! UART peripheral control.
//!
//! Implements the traits required for reading from and writing to a serial port.
//!
//! The ESP8266 has two UARTs available, `UART0` and `UART1`. `UART1` is write-only.

use crate::gpio::*;

use embedded_hal::serial::{Read, Write};
use esp8266::{UART0, UART1};
use void::Void;

/// Wait one maximum-length frame after FIFO empty. The peripheral bus is
/// 80 MHz and the supported CPU frequencies are at most 160 MHz, so 24 CPU
/// clocks per baud divider cover 12 wire bits at either supported CPU speed.
pub(crate) fn finish_frame(_divider: u32) {
    #[cfg(target_arch = "xtensa")]
    xtensa_lx::timer::delay(_divider * 24);
}

/// Extension trait for `UART0` for easily creating `UART0Serial` instances.
pub trait UART0Ext {
    fn serial(self, tdx: Gpio1<UART>, rxd: Gpio3<UART>) -> UART0Serial;
}

impl UART0Ext for UART0 {
    /// Create a new `UART0Serial` instance using the provided pins.
    fn serial(self, txd: Gpio1<UART>, rxd: Gpio3<UART>) -> UART0Serial {
        UART0Serial {
            uart: self,
            txd,
            rxd,
        }
    }
}

/// Extension trait for `UART1` for easily creating `UART1Serial` instances.
///
/// Note that `UART1` is write-only.
pub trait UART1Ext {
    fn serial(self, txd: Gpio2<UART>) -> UART1Serial;
}

impl UART1Ext for UART1 {
    /// Create a new write-only `UART1Serial` instance using the provided pin.
    fn serial(self, txd: Gpio2<UART>) -> UART1Serial {
        UART1Serial { uart: self, txd }
    }
}

/// Serial port using `UART0`.
pub struct UART0Serial {
    uart: UART0,
    txd: Gpio1<UART>,
    rxd: Gpio3<UART>,
}

impl UART0Serial {
    /// Enable an RX interrupt as soon as one byte reaches the FIFO.
    /// Install a static UART handler and unmask its CPU source separately.
    pub fn listen_rx(&mut self) {
        self.uart
            .uart_conf1()
            .modify(|_, w| unsafe { w.rxfifo_full_thrhd().bits(1) });
        self.clear_rx_interrupt();
        self.uart
            .uart_int_ena()
            .modify(|_, w| w.rxfifo_full_int_ena().set_bit());
    }

    pub fn unlisten_rx(&mut self) {
        self.uart
            .uart_int_ena()
            .modify(|_, w| w.rxfifo_full_int_ena().clear_bit());
        self.clear_rx_interrupt();
    }

    /// Acknowledge RX after draining the FIFO in the interrupt handler.
    pub fn clear_rx_interrupt(&mut self) {
        self.uart
            .uart_int_clr()
            .write(|w| w.rxfifo_full_int_clr().set_bit());
    }

    /// Free up the UART device and return the pins used.
    ///
    /// This operation blocks while there are still bytes in the transmit buffer.
    pub fn decompose(mut self) -> (UART0, Gpio1<UART>, Gpio3<UART>) {
        nb::block!(self.flush()).unwrap();
        (self.uart, self.txd, self.rxd)
    }
}

impl Read<u8> for UART0Serial {
    type Error = Void;

    /// Reads a single word from the serial interface.
    fn read(&mut self) -> nb::Result<u8, Self::Error> {
        if self.uart.uart_status().read().rxfifo_cnt().bits() > 0 {
            Ok(self.uart.uart_fifo().read().rxfifo_rd_byte().bits())
        } else {
            Err(nb::Error::WouldBlock)
        }
    }
}

impl Write<u8> for UART0Serial {
    type Error = Void;

    fn write(&mut self, word: u8) -> nb::Result<(), Self::Error> {
        if self.uart.uart_status().read().txfifo_cnt().bits() < 128 {
            self.uart
                .uart_fifo()
                .write(|w| unsafe { w.rxfifo_write_byte().bits(word) });
            Ok(())
        } else {
            Err(nb::Error::WouldBlock)
        }
    }

    fn flush(&mut self) -> nb::Result<(), Void> {
        if self.uart.uart_status().read().txfifo_cnt().bits() > 0 {
            Err(nb::Error::WouldBlock)
        } else {
            // FIFO empty can leave one final frame in the shift register.
            finish_frame(self.uart.uart_clkdiv().read().uart_clkdiv().bits());
            Ok(())
        }
    }
}

impl core::fmt::Write for UART0Serial {
    fn write_str(&mut self, s: &str) -> Result<(), core::fmt::Error> {
        s.as_bytes()
            .iter()
            .try_for_each(|c| nb::block!(self.write(*c)))
            .map_err(|_| core::fmt::Error)
    }
}

/// Write-only serial port using `UART1`.
pub struct UART1Serial {
    uart: UART1,
    txd: Gpio2<UART>,
}

impl UART1Serial {
    /// Free up the UART device and return the pins used.
    ///
    /// This operation blocks while there are still bytes in the transmit buffer.
    pub fn decompose(mut self) -> (UART1, Gpio2<UART>) {
        nb::block!(self.flush()).unwrap();
        (self.uart, self.txd)
    }
}

impl Write<u8> for UART1Serial {
    type Error = Void;

    fn write(&mut self, word: u8) -> nb::Result<(), Self::Error> {
        if self.uart.uart_status().read().txfifo_cnt().bits() < 128 {
            self.uart
                .uart_fifo()
                .write(|w| unsafe { w.rxfifo_write_byte().bits(word) });
            Ok(())
        } else {
            Err(nb::Error::WouldBlock)
        }
    }

    fn flush(&mut self) -> nb::Result<(), Void> {
        if self.uart.uart_status().read().txfifo_cnt().bits() > 0 {
            Err(nb::Error::WouldBlock)
        } else {
            // FIFO empty can leave one final frame in the shift register.
            finish_frame(self.uart.uart_clkdiv().read().uart_clkdiv().bits());
            Ok(())
        }
    }
}

impl core::fmt::Write for UART1Serial {
    fn write_str(&mut self, s: &str) -> Result<(), core::fmt::Error> {
        s.as_bytes()
            .iter()
            .try_for_each(|c| nb::block!(self.write(*c)))
            .map_err(|_| core::fmt::Error)
    }
}

impl embedded_hal::blocking::serial::write::Default<u8> for UART0Serial {}

impl embedded_hal::blocking::serial::write::Default<u8> for UART1Serial {}
