//! Send ESP-NOW broadcasts, or configure an encrypted unicast peer.
//! For encrypted unicast, set PEER to EspNowPeerConfig::encrypted(peer_mac, lmk)
//! and configure the receiver with this board's MAC and the same PMK and LMK.
//! The example keys are public placeholders; replace them for your application.

#![no_std]
#![no_main]

mod support;

use core::fmt::Write;
use esp8266_hal::timer::TimerExt;
use esp8266_hal::wifi::{Channel, EspNowPeerConfig, InitConfig, Wifi};
use panic_halt as _;

const CHANNEL: u8 = 6;
const PMK: [u8; 16] = *b"example-pmk-1234";
const PEER: EspNowPeerConfig = EspNowPeerConfig::broadcast();
// Example encrypted configuration, using the receiver's actual MAC:
// const PEER: EspNowPeerConfig =
//     EspNowPeerConfig::encrypted([0x02, 0, 0, 0, 0, 1], *b"example-lmk-1234");

#[esp8266_hal::entry]
fn main() -> ! {
    let peripherals = esp8266_hal::target::Peripherals::take().unwrap();
    let mut serial = support::serial(peripherals.UART0, peripherals.GPIO);
    let (_, timer2) = peripherals.TIMER.timers();
    let runtime = support::init(timer2);
    let mut init = InitConfig::new(14, 4, 4).unwrap();
    init.queues.station_rx = 0;
    init.queues.esp_now_peers = 1;
    let mut wifi = Wifi::new(runtime, init).expect("Wi-Fi initialization");
    let (mut controller, interfaces) = wifi.split();
    writeln!(
        serial,
        "Station MAC {:02x?}\r",
        interfaces.station.mac_address()
    )
    .unwrap();
    controller.enable_station().expect("station mode");
    controller.start_blocking(1_000).expect("start Wi-Fi");

    let channel = Channel::try_from(CHANNEL).expect("channel");
    let mut session = controller.esp_now().expect("ESP-NOW session");
    session.set_channel(channel).expect("set channel");
    session.start().expect("start ESP-NOW");
    session.set_pmk(&PMK).expect("set PMK");
    session
        .add_peer(PEER.channel(channel).for_station())
        .expect("add peer");

    loop {
        match session.send(PEER.peer_addr(), b"Hello from ESP8266") {
            Ok(()) => {
                // Keep one transmission outstanding until its callback arrives.
                loop {
                    if let Some(status) = session.send_status() {
                        writeln!(serial, "Send completed: {status:?}\r").unwrap();
                        break;
                    }
                    esp8266_hal::wifi::delay_ticks(1);
                }
            }
            Err(error) => writeln!(serial, "Send failed: {error:?}\r").unwrap(),
        }
        esp8266_hal::wifi::delay_ticks(100);
    }
}
