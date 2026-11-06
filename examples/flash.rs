//! Persistent counter in an explicitly reserved 4 KiB flash sector.
//!
//! Set ESP8266_HAL_FLASH_SCRATCH_OFFSET to a hexadecimal byte offset only after
//! confirming the whole sector fits the installed chip and is separate from
//! firmware, mapped immutable data, other storage and calibration partitions.
//! With no offset this example prints a message and does not mutate flash.

#![no_std]
#![no_main]

#[cfg(feature = "wifi")]
mod support;

use core::fmt::Write;
use esp8266_hal::flash::SPI0Ext;
use esp8266_hal::prelude::*;
use esp8266_hal::target::Peripherals;
use panic_halt as _;

#[esp8266_hal::entry]
fn main() -> ! {
    let dp = Peripherals::take().unwrap();
    // Select the application's baud independently of the ROM boot output.
    dp.UART0
        .uart_clkdiv()
        .write(|w| unsafe { w.uart_clkdiv().bits(80_000_000 / 115_200) });
    let pins = dp.GPIO.split();
    let mut led = pins.gpio2.into_push_pull_output();
    let (mut timer1, _) = dp.TIMER.timers();
    let mut serial = dp
        .UART0
        .serial(pins.gpio1.into_uart(), pins.gpio3.into_uart());
    // Let pending ROM output finish before the application's first message.
    nb::block!(serial.flush()).unwrap();

    led.set_high().unwrap();

    let mut flash = dp.SPI0.flash();

    let address = match option_env!("ESP8266_HAL_FLASH_SCRATCH_OFFSET") {
        Some(value) => u32::from_str_radix(value.trim_start_matches("0x"), 16)
            .expect("scratch offset must be hexadecimal"),
        None => {
            writeln!(
                serial,
                "Set ESP8266_HAL_FLASH_SCRATCH_OFFSET to a reserved sector"
            )
            .unwrap();
            loop {
                core::hint::spin_loop();
            }
        }
    };

    timer1.delay_ms(1000);

    let mut buff = [0u8; 8];

    flash.read(address, &mut buff).unwrap();

    buff[0] = buff[0].wrapping_add(1);

    // SAFETY: supplying the scratch offset accepts the partition contract in
    // this example's module documentation. No live reference aliases the sector.
    unsafe {
        flash.erase_sectors(address, 1).unwrap();
        flash.write_bytes(address, &buff).unwrap();
    }

    write!(serial, "\r\ncounter {}:\r\n", buff[0]).unwrap();
    write!(serial, "press '0' to reset:\r\n").unwrap();

    loop {
        let byte = nb::block!(serial.read()).unwrap();

        if byte == b'0' {
            buff[0] = 0;

            // SAFETY: same exclusively reserved sector as above.
            unsafe {
                flash.erase_sectors(address, 1).unwrap();
                flash.write_bytes(address, &buff).unwrap();
            }
        }
    }
}
