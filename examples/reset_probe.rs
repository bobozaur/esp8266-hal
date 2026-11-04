#![no_std]
#![no_main]

#[cfg(feature = "wifi")]
mod support;

use esp8266_hal::target::Peripherals;
use panic_halt as _;

#[esp8266_hal::main]
fn main() -> ! {
    let dp = Peripherals::take().unwrap();
    // Finish ROM output at its original baud before switching the divider.
    while dp.UART0.uart_status().read().txfifo_cnt().bits() != 0 {}
    let previous_divider = dp.UART0.uart_clkdiv().read().uart_clkdiv().bits();
    // Cover a final 12-bit frame with a CPU running at up to 160 MHz.
    xtensa_lx::timer::delay(previous_divider * 24);
    // Select the application's baud independently of the ROM boot output.
    dp.UART0
        .uart_clkdiv()
        .write(|w| unsafe { w.uart_clkdiv().bits(80_000_000 / 115_200) });
    esp8266_hal::logger::write_byte(b'Z');
    loop {
        core::hint::spin_loop();
    }
}
