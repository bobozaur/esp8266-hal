#![no_std]
#![no_main]

use panic_halt as _;

#[esp8266_hal::main]
fn main() -> ! {
    esp8266_hal::logger::write_byte(b'Z');
    loop {
        core::hint::spin_loop();
    }
}
