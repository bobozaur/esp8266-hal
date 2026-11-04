#![no_std]
#![no_main]

#[cfg(feature = "wifi")]
mod support;

use core::fmt::Write;
use esp8266_hal::prelude::*;
use esp8266_hal::target::Peripherals;
use panic_halt as _;

#[esp8266_hal::main]
fn main() -> ! {
    let dp = Peripherals::take().unwrap();
    // Select the application's baud independently of the ROM boot output.
    dp.UART0
        .uart_clkdiv()
        .write(|w| unsafe { w.uart_clkdiv().bits(80_000_000 / 115_200) });
    let pins = dp.GPIO.split();
    let mut serial = dp
        .UART0
        .serial(pins.gpio1.into_uart(), pins.gpio3.into_uart());

    let (mut timer1, _) = dp.TIMER.timers();
    timer1.delay_ms(100);

    write!(serial, "\r\nstart:\r\n").unwrap();

    loop {
        let byte = nb::block!(serial.read()).unwrap();
        serial.write(byte).unwrap();
    }
}
