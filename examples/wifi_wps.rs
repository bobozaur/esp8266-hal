//! Provision with WPS push-button mode or a fixed PIN.
//! Set WPS_PIN below to this enrollee's eight-digit PIN for PIN mode, and enter
//! the same PIN at the access point.
//! WpsConfig::pin() can instead request a generated PIN, reported as an event.

#![no_std]
#![no_main]

mod support;

use core::fmt::Write;
use esp8266_hal::timer::TimerExt;
use esp8266_hal::wifi::{InitConfig, Wifi, WpsConfig, WpsEvent, WpsPin};
use panic_halt as _;

const WPS_PIN: Option<&str> = None;

#[esp8266_hal::entry]
fn main() -> ! {
    let peripherals = esp8266_hal::target::Peripherals::take().unwrap();
    let mut serial = support::serial(peripherals.UART0, peripherals.GPIO);
    let (_, timer2) = peripherals.TIMER.timers();
    let runtime = support::init(timer2);
    let mut init = InitConfig::new(14, 4, 4).unwrap();
    init.queues.station_rx = 0;
    let mut wifi = Wifi::new(runtime, init).expect("Wi-Fi initialization");
    let (mut controller, _) = wifi.split();
    controller.enable_station().expect("station mode");
    controller.start_blocking(1_000).expect("start Wi-Fi");

    let config = match WPS_PIN {
        Some(pin) => WpsConfig::fixed_pin(WpsPin::try_from(pin).expect("WPS PIN")),
        None => WpsConfig::pbc(),
    };
    let mut session = controller.wps().expect("WPS session");
    session.start(&config).expect("start WPS");
    writeln!(serial, "WPS started; activate the registrar\r").unwrap();

    loop {
        match session.next_event() {
            Ok(Some(WpsEvent::Pin { pin })) => {
                writeln!(
                    serial,
                    "Enter PIN {} at the access point\r",
                    core::str::from_utf8(pin.as_bytes()).unwrap()
                )
                .unwrap();
            }
            Ok(Some(event)) => {
                writeln!(serial, "WPS: {event:?}\r").unwrap();
                if matches!(
                    event,
                    WpsEvent::Succeeded | WpsEvent::Failed { .. } | WpsEvent::TimedOut
                ) {
                    break;
                }
            }
            Ok(None) => {}
            Err(error) => {
                writeln!(serial, "WPS failed: {error:?}\r").unwrap();
                break;
            }
        }
        esp8266_hal::wifi::delay_ticks(5);
    }
    session.stop().expect("stop WPS");
    loop {
        esp8266_hal::wifi::delay_ticks(100);
    }
}
