#![no_std]
#![no_main]

use esp8266_hal::entry;
use panic_halt as _;

#[entry]
fn main() -> ! {
    esp8266_hal::logger::write_byte(b'Z');
    loop {
        core::hint::spin_loop();
    }
}
