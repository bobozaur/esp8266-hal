#![no_std]
#![no_main]
use embassy_executor::Spawner;
use esp8266_hal::println;
use esp8266_hal::wifi::controller::WifiInitConfig;
use esp8266_hal::wifi::event::{self, WifiEvent};
use esp8266_hal::wifi::{WifiController, WifiMode};
use esp8266_wifi_sys as sys;
use portable_atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};

static SCAN_DONE_SEQ: AtomicU32 = AtomicU32::new(0);
static SCAN_DONE_COUNT: AtomicU8 = AtomicU8::new(0);
static SCAN_DONE_SUCCESS: AtomicBool = AtomicBool::new(false);

fn scan_event_handler(event: WifiEvent) {
    if let WifiEvent::ScanDone { count, success } = event {
        SCAN_DONE_COUNT.store(count, Ordering::Release);
        SCAN_DONE_SUCCESS.store(success, Ordering::Release);
        SCAN_DONE_SEQ.fetch_add(1, Ordering::AcqRel);
    }
}

#[inline(never)]
fn nmi_count() -> u32 {
    extern "C" {
        static g_nmi_call_count: u32;
    }

    unsafe { g_nmi_call_count }
}

#[inline(never)]
fn reset_scan_done_probe() {
    SCAN_DONE_COUNT.store(0, Ordering::Release);
    SCAN_DONE_SUCCESS.store(false, Ordering::Release);
    SCAN_DONE_SEQ.store(0, Ordering::Release);
}

#[inline(never)]
fn run_scan(label: &str, cfg: &sys::wifi_scan_config_t) {
    reset_scan_done_probe();
    esp8266_hal::wifi::compat::wifi_os_adapter::arm_pp_sig_trace(12);
    let nmi_before = nmi_count();
    let start_us = unsafe { sys::esp_timer_get_time() };
    unsafe { sys::esp_wifi_scan_start(cfg, true) };
    let elapsed_us = unsafe { sys::esp_timer_get_time() } - start_us;
    let nmi_after = nmi_count();
    let mut count: u16 = 0;
    unsafe { sys::esp_wifi_scan_get_ap_num(&mut count) };
    println!(
        "{}: event_seq={} event_ok={} event_count={} ap_num={} dt_us={} nmi={}->{}",
        label,
        SCAN_DONE_SEQ.load(Ordering::Acquire),
        SCAN_DONE_SUCCESS.load(Ordering::Acquire) as u8,
        SCAN_DONE_COUNT.load(Ordering::Acquire),
        count,
        elapsed_us,
        nmi_before,
        nmi_after
    );
}

#[esp8266_hal::main]
async fn main(_spawner: Spawner) {
    for _ in 0..20_000_000u32 {
        core::hint::spin_loop();
    }
    let mut wifi = WifiController::init(WifiInitConfig::default()).expect("wifi");
    event::set_event_handler(scan_event_handler).expect("event_handler");
    wifi.set_mode(WifiMode::Sta).expect("mode");
    wifi.start().expect("start");

    let passive_cfg = sys::wifi_scan_config_t {
        ssid: core::ptr::null_mut(),
        bssid: core::ptr::null_mut(),
        channel: 6,
        show_hidden: true,
        scan_type: sys::WIFI_SCAN_TYPE_PASSIVE,
        scan_time: sys::wifi_scan_time_t { passive: 500 },
    };
    run_scan("ch6 passive-any", &passive_cfg);
    println!("done");
    loop {
        embassy_time::Timer::after(embassy_time::Duration::from_secs(60)).await;
    }
}
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    esp8266_hal::logger::write_byte(b'?');
    loop {}
}
