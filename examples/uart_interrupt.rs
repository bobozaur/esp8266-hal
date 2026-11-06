#![no_std]
#![no_main]

#[cfg(feature = "wifi")]
mod support;

use core::cell::RefCell;
use critical_section::Mutex;
use esp8266_hal::interrupt::{enable_interrupt, InterruptType};
use esp8266_hal::prelude::*;
use esp8266_hal::target::Peripherals;
use esp8266_hal::uart::UART0Serial;
use panic_halt as _;

static SERIAL: Mutex<RefCell<Option<UART0Serial>>> = Mutex::new(RefCell::new(None));

#[esp8266_hal::entry]
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
    let mut led = pins.gpio2.into_push_pull_output();
    let (mut timer, _) = dp.TIMER.timers();

    critical_section::with(|cs| {
        serial.listen_rx();
        SERIAL.borrow(cs).replace(Some(serial));
    });
    // SAFETY: this application owns UART0, installed the static handler below,
    // and unmasks it outside the critical section. No SDK owns this source.
    unsafe {
        enable_interrupt(InterruptType::UART);
    }

    loop {
        timer.delay_ms(500);
        led.toggle().unwrap();
    }
}

#[interrupt(uart)]
fn uart() {
    critical_section::with(|cs| {
        if let Some(serial) = SERIAL.borrow(cs).borrow_mut().as_mut() {
            while let Ok(byte) = serial.read() {
                // Echo is best-effort when TX is full; do not block in an ISR.
                let _ = serial.write(byte);
            }
            serial.clear_rx_interrupt();
        }
    });
}
