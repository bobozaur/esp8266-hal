//! ESP8266 promiscuous-mode capture validation using the typestate WiFi core.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_futures::yield_now;
use embassy_time::{Duration, Instant, Timer};

use esp8266_hal::wifi::{
    Channel, Esp8266TargetBackend, InitConfig, PromiscuousControlFilter, PromiscuousFilter,
    PromiscuousPacketType, PromiscuousRxFrame, WifiDriver, WifiResources,
};
use static_cell::StaticCell;
use xtensa_lx_rt::exception::{Context, ExceptionCause};

type PromiscResources = WifiResources<8, 4, 2, 0, 0, 2>;

static WIFI_RESOURCES: StaticCell<PromiscResources> = StaticCell::new();

// Keep capture and API-validation paths linked in the same hardware shape; the
// ESP8266 promiscuous enable path is sensitive enough that a tiny API-only
// binary is not representative of the production capture path.
#[used]
static API_ONLY: u8 = if option_env!("ESP8266_HAL_PROMISC_API_ONLY").is_some() {
    1
} else {
    0
};

#[inline(always)]
fn log_line(message: &[u8]) {
    esp8266_hal::logger::write_bytes(message);
    esp8266_hal::logger::write_newline();
}

fn selected_filter() -> (&'static [u8], PromiscuousFilter) {
    match option_env!("ESP8266_HAL_PROMISC_FILTER") {
        Some("management") => (b"management", PromiscuousFilter::MANAGEMENT),
        Some("control") => (b"control", PromiscuousFilter::CONTROL),
        Some("data") => (b"data", PromiscuousFilter::DATA),
        Some("misc") => (b"misc", PromiscuousFilter::MISC),
        _ => (b"all", PromiscuousFilter::all()),
    }
}

fn selected_control_filter() -> (&'static [u8], PromiscuousControlFilter) {
    match option_env!("ESP8266_HAL_PROMISC_CTRL_FILTER") {
        Some("wrapper") => (b"wrapper", PromiscuousControlFilter::WRAPPER),
        Some("block-ack-request") => (
            b"block-ack-request",
            PromiscuousControlFilter::BLOCK_ACK_REQUEST,
        ),
        Some("block-ack") => (b"block-ack", PromiscuousControlFilter::BLOCK_ACK),
        Some("ps-poll") => (b"ps-poll", PromiscuousControlFilter::PS_POLL),
        Some("rts") => (b"rts", PromiscuousControlFilter::RTS),
        Some("cts") => (b"cts", PromiscuousControlFilter::CTS),
        Some("ack") => (b"ack", PromiscuousControlFilter::ACK),
        Some("cf-end") => (b"cf-end", PromiscuousControlFilter::CF_END),
        Some("cf-end-ack") => (b"cf-end-ack", PromiscuousControlFilter::CF_END_ACK),
        Some("all") => (b"all", PromiscuousControlFilter::all()),
        _ => (b"none", PromiscuousControlFilter::empty()),
    }
}

fn api_only() -> bool {
    unsafe { core::ptr::read_volatile(core::ptr::addr_of!(API_ONLY)) == 1 }
}

fn selected_channel() -> Channel {
    let channel = match option_env!("ESP8266_HAL_PROMISC_CHANNEL") {
        Some("1") => 1,
        Some("2") => 2,
        Some("3") => 3,
        Some("4") => 4,
        Some("5") => 5,
        Some("6") => 6,
        Some("7") => 7,
        Some("8") => 8,
        Some("9") => 9,
        Some("10") => 10,
        Some("11") => 11,
        Some("12") => 12,
        Some("13") => 13,
        Some("14") => 14,
        _ => 6,
    };
    Channel::try_from(channel).expect("promisc channel")
}

fn log_filter(name: &[u8]) {
    esp8266_hal::logger::write_bytes(b"promisc filter ");
    esp8266_hal::logger::write_bytes(name);
    esp8266_hal::logger::write_newline();
}

fn log_control_filter(name: &[u8]) {
    esp8266_hal::logger::write_bytes(b"promisc ctrl filter ");
    esp8266_hal::logger::write_bytes(name);
    esp8266_hal::logger::write_newline();
}

fn log_channel(channel: Channel) {
    esp8266_hal::logger::write_bytes(b"promisc channel ");
    esp8266_hal::logger::write_dec_u32(channel.get() as u32);
    esp8266_hal::logger::write_newline();
}

fn log_frame(frame: &PromiscuousRxFrame) {
    let info = frame.info();
    esp8266_hal::logger::write_bytes(b"promisc frame type=");
    let packet_type: &[u8] = match info.packet_type {
        PromiscuousPacketType::Management => b"management",
        PromiscuousPacketType::Control => b"control",
        PromiscuousPacketType::Data => b"data",
        PromiscuousPacketType::Misc => b"misc",
    };
    esp8266_hal::logger::write_bytes(packet_type);
    esp8266_hal::logger::write_bytes(b" ch=");
    esp8266_hal::logger::write_dec_u32(info.channel as u32);
    esp8266_hal::logger::write_bytes(b" len=");
    esp8266_hal::logger::write_dec_u32(frame.payload().len() as u32);
    esp8266_hal::logger::write_newline();
}

#[esp8266_hal::main]
async fn main(_spawner: Spawner) {
    log_line(b"wifi_promisc start");

    let resources = WIFI_RESOURCES.init_with(WifiResources::new);
    let backend = Esp8266TargetBackend::new(resources);
    let driver = WifiDriver::with_backend(resources, backend)
        .initialize(InitConfig::default())
        .expect("wifi init");
    log_line(b"wifi init ok");

    let mut driver = match driver.enable_sta().and_then(|driver| driver.start()) {
        Ok(driver) => driver,
        Err(_error) => {
            log_line(b"promisc start err");
            loop {
                Timer::after(Duration::from_secs(60)).await;
            }
        }
    };
    log_line(b"promisc sta started");

    let channel = selected_channel();
    let (filter_name, filter) = selected_filter();
    let (control_filter_name, control_filter) = selected_control_filter();
    log_filter(filter_name);
    log_control_filter(control_filter_name);
    log_channel(channel);

    let mut session = match driver.promiscuous() {
        Ok(session) => session,
        Err(_error) => {
            log_line(b"promisc session err");
            loop {
                Timer::after(Duration::from_secs(60)).await;
            }
        }
    };

    if session.set_channel(channel).is_err() {
        log_line(b"promisc set channel err");
        loop {
            Timer::after(Duration::from_secs(60)).await;
        }
    }
    log_line(b"promisc set channel ok");

    if session.start(filter).is_err() {
        log_line(b"promisc enable err");
        loop {
            Timer::after(Duration::from_secs(60)).await;
        }
    }
    log_line(b"promisc enable ok");

    if api_only() {
        if session
            .set_control_filter(PromiscuousControlFilter::ACK)
            .is_err()
            || session.control_filter() != PromiscuousControlFilter::ACK
            || !session.is_active()
            || session.filter() != filter
        {
            log_line(b"promisc ctrl filter err");
            loop {
                Timer::after(Duration::from_secs(60)).await;
            }
        }
        log_line(b"promisc ctrl filter ack ok");

        if session
            .set_control_filter(PromiscuousControlFilter::empty())
            .is_err()
            || session.control_filter() != PromiscuousControlFilter::empty()
        {
            log_line(b"promisc ctrl filter err");
            loop {
                Timer::after(Duration::from_secs(60)).await;
            }
        }
        log_line(b"promisc ctrl filter none ok");

        if session.stop().is_err()
            || session.is_active()
            || session.filter() != PromiscuousFilter::empty()
        {
            log_line(b"promisc stop err");
            loop {
                Timer::after(Duration::from_secs(60)).await;
            }
        }
        log_line(b"promisc stop ok");
        log_line(b"promisc-control-api-ok");
        loop {
            Timer::after(Duration::from_secs(60)).await;
        }
    }

    if !control_filter.is_empty() && session.set_control_filter(control_filter).is_err() {
        log_line(b"promisc ctrl filter err");
        log_line(b"promisc start err");
        loop {
            Timer::after(Duration::from_secs(60)).await;
        }
    }
    if !control_filter.is_empty() {
        log_line(b"promisc ctrl filter ok");
    }
    log_line(b"promisc started");

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut polls = 0_u32;
    while Instant::now() < deadline {
        if let Some(frame) = session.receive() {
            log_frame(&frame);
            if filter_name == b"management" {
                log_line(b"promisc-management-ok");
            } else if filter_name == b"control" {
                log_line(b"promisc-control-ok");
            } else {
                log_line(b"promisc-ok");
            }
            let _ = session.stop();
            loop {
                Timer::after(Duration::from_secs(60)).await;
            }
        }

        polls = polls.wrapping_add(1);
        if polls & 0xff == 0 {
            yield_now().await;
        }
    }

    log_line(b"promisc timeout");
    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    esp8266_hal::logger::write_byte(b'?');
    loop {}
}

#[link_section = ".rwtext"]
#[xtensa_lx_rt::exception]
fn exception(cause: ExceptionCause, frame: Context) -> ! {
    let _ = cause;
    let _ = frame;
    esp8266_hal::logger::write_byte(b'E');
    loop {}
}
