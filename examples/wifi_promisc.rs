//! Capture frame metadata on a selected Wi-Fi channel.
//! Change FILTER and CONTROL_FILTER to select the desired frame classes.

#![no_std]
#![no_main]

mod support;

use core::fmt::Write;
use esp8266_hal::timer::TimerExt;
use esp8266_hal::wifi::{Channel, InitConfig, PromiscuousControlFilter, PromiscuousFilter, Wifi};
use panic_halt as _;

const CHANNEL: u8 = 6;
const FILTER: PromiscuousFilter = PromiscuousFilter::MANAGEMENT;
const CONTROL_FILTER: PromiscuousControlFilter = PromiscuousControlFilter::all();

#[esp8266_hal::entry]
fn main() -> ! {
    let peripherals = esp8266_hal::target::Peripherals::take().unwrap();
    let mut serial = support::serial(peripherals.UART0, peripherals.GPIO);
    let (_, timer2) = peripherals.TIMER.timers();
    let runtime = support::init(timer2);
    let mut init = InitConfig::new(14, 4, 4).unwrap();
    init.queues.station_rx = 0;
    init.queues.promiscuous_rx = 2;
    let mut wifi = Wifi::new(runtime, init).expect("Wi-Fi initialization");
    let (mut controller, _) = wifi.split();
    controller.enable_station().expect("station mode");
    controller.start_blocking(1_000).expect("start Wi-Fi");

    let mut session = controller.promiscuous().expect("capture session");
    session
        .set_channel(Channel::try_from(CHANNEL).expect("channel"))
        .expect("set channel");
    session.start(FILTER).expect("start capture");
    session
        .set_control_filter(CONTROL_FILTER)
        .expect("control filter");
    writeln!(serial, "Capturing on channel {CHANNEL}\r").unwrap();

    loop {
        if let Some(frame) = session.receive() {
            let info = frame.info();
            writeln!(
                serial,
                "{:?}: channel={} captured={} bytes\r",
                info.packet_type,
                info.channel,
                frame.payload().len()
            )
            .unwrap();
        }
        esp8266_hal::wifi::delay_ticks(1);
    }
}
