//! Receive ESP-NOW payloads on a selected channel.
//! To receive encrypted unicast, set PEER to Some(EspNowPeerConfig::encrypted(
//! sender_mac, lmk)) and use the same PMK and LMK on the sender.
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
const PEER: Option<EspNowPeerConfig> = None;
// Example encrypted configuration, using the sender's actual MAC:
// const PEER: Option<EspNowPeerConfig> = Some(
//     EspNowPeerConfig::encrypted([0x02, 0, 0, 0, 0, 2], *b"example-lmk-1234"),
// );

#[esp8266_hal::entry]
fn main() -> ! {
    let peripherals = esp8266_hal::target::Peripherals::take().unwrap();
    let mut serial = support::serial(peripherals.UART0, peripherals.GPIO);
    let (_, timer2) = peripherals.TIMER.timers();
    let runtime = support::init(timer2);
    let mut init = InitConfig::new(14, 4, 4).unwrap();
    init.queues.station_rx = 0;
    init.queues.esp_now_peers = usize::from(PEER.is_some());
    init.queues.esp_now_rx = 2;
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
    if let Some(peer) = PEER {
        session
            .add_peer(peer.channel(channel).for_station())
            .expect("add peer");
    }
    writeln!(serial, "Receiving ESP-NOW on channel {CHANNEL}\r").unwrap();

    loop {
        if let Some(frame) = session.receive() {
            writeln!(
                serial,
                "From {:02x?}: {:02x?}\r",
                frame.peer_addr(),
                frame.payload()
            )
            .unwrap();
        }
        esp8266_hal::wifi::delay_ticks(1);
    }
}
