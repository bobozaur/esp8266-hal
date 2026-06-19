//! SmartConfig provisioning example using the typestate WiFi core.

#![no_std]
#![no_main]

use esp8266_hal::wifi::{
    Esp8266TargetBackend, InitConfig, SmartConfigCredentials, SmartConfigStartConfig,
    SmartConfigTimeout, SmartConfigType, StaMode, Started, WifiDriver, WifiEvent, WifiResources,
};
use smoltcp::iface::{Config as SmolConfig, Interface, SocketHandle, SocketSet, SocketStorage};
use smoltcp::socket::{dhcpv4, udp};
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpAddress, IpCidr, IpEndpoint, Ipv4Address};
use static_cell::StaticCell;
use xtensa_lx_rt::exception::{Context, ExceptionCause};

const UDP_BUF_BYTES: usize = 32;
const ESPTOUCH_ACK_PORT: u16 = 18_266;
const DHCP_TIMEOUT_MS: u64 = 30_000;
const STATIC_ACK_IP: [u8; 4] = [192, 168, 4, 2];
const STATIC_ACK_ENABLED: bool = option_env!("ESP8266_HAL_SMARTCONFIG_STATIC_ACK").is_some();

type SmartConfigResources = WifiResources<8, 4, 2>;
type SmartConfigDriver =
    WifiDriver<'static, Started<StaMode>, Esp8266TargetBackend<'static, 8, 4, 2>, 8, 4, 2>;
type SmartConfigLink = esp8266_hal::wifi_core::StaLink<
    'static,
    StaMode,
    Esp8266TargetBackend<'static, 8, 4, 2>,
    8,
    4,
    2,
>;
type SmartConfigDevice = esp8266_hal::wifi::smoltcp::WifiDevice<
    'static,
    StaMode,
    Esp8266TargetBackend<'static, 8, 4, 2>,
    8,
    4,
    2,
>;

static WIFI_RESOURCES: StaticCell<SmartConfigResources> = StaticCell::new();

extern "C" {
    fn smartconfig_get_status() -> i32;
    static SnifGlob: [u8; 20];
    static g_SC_Select: u32;
    static pTouchGlob: *const core::ffi::c_void;
    static promiscuous_cb: *const core::ffi::c_void;
    static sniffer_enable: u32;
}

#[cfg(esp8266_hal_smartconfig_sniffer_trace)]
extern "C" {
    static g_smartconfig_sniffer_calls: u32;
    static g_smartconfig_sniffer_last_type: u32;
    static g_smartconfig_sniffer_last_len: u32;
    static g_smartconfig_sniffer_last_channel: u32;
    static g_smartconfig_sniffer_last_rssi: u32;
}

#[cfg(esp8266_hal_smartconfig_wifi_trace)]
extern "C" {
    fn esp8266_hal_sc_wifi_trace_snapshot(out: *mut u32, len: usize);
}

#[cfg(esp8266_hal_smartconfig_rx_trace)]
extern "C" {
    fn esp8266_hal_sc_rx_trace_snapshot(out: *mut u32, len: usize);
}

#[cfg(esp8266_hal_smartconfig_event_trace)]
extern "C" {
    fn esp8266_hal_sc_event_trace_snapshot(out: *mut u32, len: usize);
}

#[inline(always)]
fn log_line(message: &[u8]) {
    esp8266_hal::logger::write_bytes(message);
    esp8266_hal::logger::write_newline();
}

fn idle_forever() -> ! {
    loop {
        esp8266_hal::wifi::delay_ticks(100);
    }
}

fn smartconfig_start_config() -> SmartConfigStartConfig {
    SmartConfigStartConfig::default()
        .enable_log(option_env!("ESP8266_HAL_SMARTCONFIG_ENABLE_LOG").is_some())
}

fn smartconfig_diag_enabled() -> bool {
    option_env!("ESP8266_HAL_SMARTCONFIG_DIAG").is_some()
}

fn smartconfig_wait_diag_enabled() -> bool {
    option_env!("ESP8266_HAL_SMARTCONFIG_DIAG_WAIT").is_some()
}

#[cfg(esp8266_hal_smartconfig_rx_trace)]
fn smartconfig_rx_diag_enabled() -> bool {
    option_env!("ESP8266_HAL_SMARTCONFIG_RX_DIAG").is_some()
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct SmartConfigDiagSnapshot {
    status: u32,
    select: u32,
    touch: u32,
    ets_timer: u32,
    tick: u32,
    heap: u32,
    promiscuous_cb: u32,
    sniffer_enable: u32,
    scheduler_suspended: u32,
    pended_ticks: u32,
    tick_isr: u32,
    intenable: u32,
    tick_stage: u32,
    ctx_stage: u32,
    switch_stage: u32,
    wait_stage: u32,
    current_task: u32,
}

fn read_smartconfig_debug() -> SmartConfigDiagSnapshot {
    SmartConfigDiagSnapshot {
        status: unsafe { smartconfig_get_status() }.max(0) as u32,
        select: unsafe { core::ptr::read_volatile(core::ptr::addr_of!(g_SC_Select)) },
        touch: unsafe { core::ptr::read_volatile(core::ptr::addr_of!(pTouchGlob)) as u32 },
        ets_timer: unsafe {
            core::ptr::read_volatile(core::ptr::addr_of!(
                esp8266_hal::wifi::compat::timer::g_ets_timer_task_count
            ))
        },
        tick: unsafe { esp8266_hal::wifi::compat::task::xTaskGetTickCount() },
        heap: esp8266_hal::wifi::free_heap_bytes(),
        promiscuous_cb: unsafe {
            core::ptr::read_volatile(core::ptr::addr_of!(promiscuous_cb)) as u32
        },
        sniffer_enable: unsafe { core::ptr::read_volatile(core::ptr::addr_of!(sniffer_enable)) },
        scheduler_suspended: unsafe {
            esp8266_hal::wifi::compat::task::debug_scheduler_suspended_count()
        },
        pended_ticks: unsafe { esp8266_hal::wifi::compat::task::debug_pended_ticks_count() },
        tick_isr: unsafe {
            core::ptr::read_volatile(core::ptr::addr_of!(
                esp8266_hal::wifi::compat::port::g_tick_isr_count
            ))
        },
        intenable: unsafe {
            core::ptr::read_volatile(core::ptr::addr_of!(
                esp8266_hal::wifi::compat::port::g_last_intenable
            ))
        },
        tick_stage: unsafe {
            core::ptr::read_volatile(core::ptr::addr_of!(
                esp8266_hal::wifi::compat::task::g_tick_stage
            ))
        },
        ctx_stage: unsafe {
            core::ptr::read_volatile(core::ptr::addr_of!(
                esp8266_hal::wifi::compat::port::g_ctx_stage
            ))
        },
        switch_stage: unsafe {
            core::ptr::read_volatile(core::ptr::addr_of!(
                esp8266_hal::wifi::compat::task::g_switch_stage
            ))
        },
        wait_stage: unsafe {
            core::ptr::read_volatile(core::ptr::addr_of!(
                esp8266_hal::wifi::compat::task::g_wait_stage
            ))
        },
        current_task: esp8266_hal::wifi::compat::task::debug_current_task_marker() as u32,
    }
}

fn write_smartconfig_debug(label: &[u8], diag: SmartConfigDiagSnapshot) {
    esp8266_hal::logger::write_bytes(b"smartconfig diag ");
    esp8266_hal::logger::write_bytes(label);
    esp8266_hal::logger::write_bytes(b" status=");
    esp8266_hal::logger::write_dec_u32(diag.status);
    esp8266_hal::logger::write_bytes(b" sc=");
    esp8266_hal::logger::write_hex_u32(diag.select);
    esp8266_hal::logger::write_bytes(b" pt=");
    esp8266_hal::logger::write_hex_u32(diag.touch);
    esp8266_hal::logger::write_bytes(b" et=");
    esp8266_hal::logger::write_dec_u32(diag.ets_timer);
    esp8266_hal::logger::write_bytes(b" tick=");
    esp8266_hal::logger::write_dec_u32(diag.tick);
    esp8266_hal::logger::write_bytes(b" heap=");
    esp8266_hal::logger::write_dec_u32(diag.heap);
    esp8266_hal::logger::write_bytes(b" cb=");
    esp8266_hal::logger::write_hex_u32(diag.promiscuous_cb);
    esp8266_hal::logger::write_bytes(b" se=");
    esp8266_hal::logger::write_hex_u32(diag.sniffer_enable);
    esp8266_hal::logger::write_bytes(b" susp=");
    esp8266_hal::logger::write_dec_u32(diag.scheduler_suspended);
    esp8266_hal::logger::write_bytes(b" pend=");
    esp8266_hal::logger::write_dec_u32(diag.pended_ticks);
    esp8266_hal::logger::write_bytes(b" isr=");
    esp8266_hal::logger::write_dec_u32(diag.tick_isr);
    esp8266_hal::logger::write_bytes(b" ie=");
    esp8266_hal::logger::write_hex_u32(diag.intenable);
    esp8266_hal::logger::write_bytes(b" stage=");
    esp8266_hal::logger::write_hex_u32(diag.tick_stage);
    esp8266_hal::logger::write_byte(b'/');
    esp8266_hal::logger::write_hex_u32(diag.ctx_stage);
    esp8266_hal::logger::write_byte(b'/');
    esp8266_hal::logger::write_hex_u32(diag.switch_stage);
    esp8266_hal::logger::write_byte(b'/');
    esp8266_hal::logger::write_hex_u32(diag.wait_stage);
    esp8266_hal::logger::write_byte(b'/');
    esp8266_hal::logger::write_hex_u32(diag.current_task);
    #[cfg(esp8266_hal_smartconfig_sniffer_trace)]
    unsafe {
        esp8266_hal::logger::write_bytes(b" snc=");
        esp8266_hal::logger::write_dec_u32(core::ptr::read_volatile(core::ptr::addr_of!(
            g_smartconfig_sniffer_calls
        )));
        esp8266_hal::logger::write_bytes(b" snt=");
        esp8266_hal::logger::write_dec_u32(core::ptr::read_volatile(core::ptr::addr_of!(
            g_smartconfig_sniffer_last_type
        )));
        esp8266_hal::logger::write_bytes(b" snl=");
        esp8266_hal::logger::write_dec_u32(core::ptr::read_volatile(core::ptr::addr_of!(
            g_smartconfig_sniffer_last_len
        )));
        esp8266_hal::logger::write_bytes(b" snch=");
        esp8266_hal::logger::write_dec_u32(core::ptr::read_volatile(core::ptr::addr_of!(
            g_smartconfig_sniffer_last_channel
        )));
        esp8266_hal::logger::write_bytes(b" snr=");
        esp8266_hal::logger::write_hex_u32(core::ptr::read_volatile(core::ptr::addr_of!(
            g_smartconfig_sniffer_last_rssi
        )));
    }
    #[cfg(esp8266_hal_smartconfig_wifi_trace)]
    {
        let mut trace = [0_u32; 12];
        unsafe {
            esp8266_hal_sc_wifi_trace_snapshot(trace.as_mut_ptr(), trace.len());
        }
        esp8266_hal::logger::write_bytes(b" wch=");
        esp8266_hal::logger::write_dec_u32(trace[0]);
        esp8266_hal::logger::write_byte(b'/');
        esp8266_hal::logger::write_dec_u32(trace[1]);
        esp8266_hal::logger::write_byte(b'/');
        esp8266_hal::logger::write_hex_u32(trace[2]);
        esp8266_hal::logger::write_bytes(b" wf=");
        esp8266_hal::logger::write_dec_u32(trace[3]);
        esp8266_hal::logger::write_byte(b'/');
        esp8266_hal::logger::write_hex_u32(trace[4]);
        esp8266_hal::logger::write_byte(b'/');
        esp8266_hal::logger::write_hex_u32(trace[5]);
        esp8266_hal::logger::write_bytes(b" wcb=");
        esp8266_hal::logger::write_dec_u32(trace[6]);
        esp8266_hal::logger::write_byte(b'/');
        esp8266_hal::logger::write_hex_u32(trace[7]);
        esp8266_hal::logger::write_byte(b'/');
        esp8266_hal::logger::write_hex_u32(trace[8]);
        esp8266_hal::logger::write_bytes(b" wp=");
        esp8266_hal::logger::write_dec_u32(trace[9]);
        esp8266_hal::logger::write_byte(b'/');
        esp8266_hal::logger::write_dec_u32(trace[10]);
        esp8266_hal::logger::write_byte(b'/');
        esp8266_hal::logger::write_hex_u32(trace[11]);
    }
    #[cfg(esp8266_hal_smartconfig_rx_trace)]
    {
        let mut trace = [0_u32; 8];
        unsafe {
            esp8266_hal_sc_rx_trace_snapshot(trace.as_mut_ptr(), trace.len());
        }
        esp8266_hal::logger::write_bytes(b" rxset=");
        esp8266_hal::logger::write_dec_u32(trace[0]);
        esp8266_hal::logger::write_byte(b'/');
        esp8266_hal::logger::write_hex_u32(trace[1]);
        esp8266_hal::logger::write_byte(b'/');
        esp8266_hal::logger::write_hex_u32(trace[2]);
        esp8266_hal::logger::write_bytes(b" rx=");
        esp8266_hal::logger::write_dec_u32(trace[3]);
        esp8266_hal::logger::write_byte(b'/');
        esp8266_hal::logger::write_dec_u32(trace[4]);
        esp8266_hal::logger::write_byte(b'/');
        esp8266_hal::logger::write_dec_u32(trace[5]);
        esp8266_hal::logger::write_byte(b'/');
        esp8266_hal::logger::write_dec_u32(trace[6]);
        esp8266_hal::logger::write_byte(b'/');
        esp8266_hal::logger::write_hex_u32(trace[7]);
    }
    #[cfg(esp8266_hal_smartconfig_event_trace)]
    {
        let mut trace = [0_u32; 8];
        unsafe {
            esp8266_hal_sc_event_trace_snapshot(trace.as_mut_ptr(), trace.len());
        }
        esp8266_hal::logger::write_bytes(b" ev=");
        esp8266_hal::logger::write_dec_u32(trace[0]);
        esp8266_hal::logger::write_byte(b'/');
        esp8266_hal::logger::write_dec_u32(trace[1]);
        esp8266_hal::logger::write_byte(b'/');
        esp8266_hal::logger::write_dec_u32(trace[2]);
        esp8266_hal::logger::write_byte(b'/');
        esp8266_hal::logger::write_hex_u32(trace[3]);
        esp8266_hal::logger::write_byte(b'/');
        esp8266_hal::logger::write_dec_u32(trace[4]);
        esp8266_hal::logger::write_bytes(b" got=");
        esp8266_hal::logger::write_dec_u32(trace[5]);
        esp8266_hal::logger::write_byte(b'/');
        esp8266_hal::logger::write_dec_u32(trace[6]);
        esp8266_hal::logger::write_byte(b'/');
        esp8266_hal::logger::write_dec_u32(trace[7]);
    }
    esp8266_hal::logger::write_bytes(b" snif=");
    let snif = unsafe { core::ptr::addr_of!(SnifGlob) as *const u8 };
    for idx in 0..20 {
        esp8266_hal::logger::write_hex_u8(unsafe { core::ptr::read_volatile(snif.add(idx)) });
    }
    esp8266_hal::logger::write_newline();
}

#[cfg(esp8266_hal_smartconfig_rx_trace)]
fn read_smartconfig_rx_trace() -> [u32; 8] {
    let mut trace = [0_u32; 8];
    unsafe {
        esp8266_hal_sc_rx_trace_snapshot(trace.as_mut_ptr(), trace.len());
    }
    trace
}

#[cfg(esp8266_hal_smartconfig_rx_trace)]
fn write_smartconfig_rx_trace(label: &[u8], trace: [u32; 8]) {
    esp8266_hal::logger::write_bytes(b"smartconfig rxdiag ");
    esp8266_hal::logger::write_bytes(label);
    esp8266_hal::logger::write_bytes(b" set=");
    esp8266_hal::logger::write_dec_u32(trace[0]);
    esp8266_hal::logger::write_byte(b'/');
    esp8266_hal::logger::write_hex_u32(trace[1]);
    esp8266_hal::logger::write_byte(b'/');
    esp8266_hal::logger::write_hex_u32(trace[2]);
    esp8266_hal::logger::write_bytes(b" cb=");
    esp8266_hal::logger::write_dec_u32(trace[3]);
    esp8266_hal::logger::write_byte(b'/');
    esp8266_hal::logger::write_dec_u32(trace[4]);
    esp8266_hal::logger::write_byte(b'/');
    esp8266_hal::logger::write_dec_u32(trace[5]);
    esp8266_hal::logger::write_byte(b'/');
    esp8266_hal::logger::write_dec_u32(trace[6]);
    esp8266_hal::logger::write_byte(b'/');
    esp8266_hal::logger::write_hex_u32(trace[7]);
    #[cfg(esp8266_hal_smartconfig_sniffer_trace)]
    unsafe {
        esp8266_hal::logger::write_bytes(b" sn=");
        esp8266_hal::logger::write_dec_u32(core::ptr::read_volatile(core::ptr::addr_of!(
            g_smartconfig_sniffer_calls
        )));
        esp8266_hal::logger::write_byte(b'/');
        esp8266_hal::logger::write_dec_u32(core::ptr::read_volatile(core::ptr::addr_of!(
            g_smartconfig_sniffer_last_type
        )));
        esp8266_hal::logger::write_byte(b'/');
        esp8266_hal::logger::write_dec_u32(core::ptr::read_volatile(core::ptr::addr_of!(
            g_smartconfig_sniffer_last_len
        )));
        esp8266_hal::logger::write_byte(b'/');
        esp8266_hal::logger::write_dec_u32(core::ptr::read_volatile(core::ptr::addr_of!(
            g_smartconfig_sniffer_last_channel
        )));
        esp8266_hal::logger::write_byte(b'/');
        esp8266_hal::logger::write_hex_u32(core::ptr::read_volatile(core::ptr::addr_of!(
            g_smartconfig_sniffer_last_rssi
        )));
    }
    esp8266_hal::logger::write_newline();
}

#[cfg(esp8266_hal_smartconfig_rx_trace)]
fn log_smartconfig_rx_trace(label: &[u8]) -> Option<[u32; 8]> {
    if !smartconfig_rx_diag_enabled() {
        return None;
    }
    let trace = read_smartconfig_rx_trace();
    write_smartconfig_rx_trace(label, trace);
    Some(trace)
}

#[cfg(not(esp8266_hal_smartconfig_rx_trace))]
fn log_smartconfig_rx_trace(_label: &[u8]) -> Option<[u32; 8]> {
    None
}

fn log_smartconfig_debug(label: &[u8]) -> Option<SmartConfigDiagSnapshot> {
    if !smartconfig_diag_enabled() {
        return None;
    }
    let diag = read_smartconfig_debug();
    write_smartconfig_debug(label, diag);
    Some(diag)
}

fn maybe_log_smartconfig_wait(
    last: &mut Option<SmartConfigDiagSnapshot>,
    unchanged_loops: &mut u32,
) {
    if !smartconfig_wait_diag_enabled() {
        return;
    }

    let diag = read_smartconfig_debug();
    if last.map(|last| last != diag).unwrap_or(true) || *unchanged_loops >= 1024 {
        write_smartconfig_debug(b"wait", diag);
        *last = Some(diag);
        *unchanged_loops = 0;
    } else {
        *unchanged_loops = (*unchanged_loops).wrapping_add(1);
    }
}

#[cfg(esp8266_hal_smartconfig_rx_trace)]
fn maybe_log_smartconfig_rx_wait(last: &mut Option<[u32; 8]>, unchanged_loops: &mut u32) {
    if !smartconfig_rx_diag_enabled() {
        return;
    }

    let trace = read_smartconfig_rx_trace();
    if last.map(|last| last != trace).unwrap_or(true) || *unchanged_loops >= 1024 {
        write_smartconfig_rx_trace(b"wait", trace);
        *last = Some(trace);
        *unchanged_loops = 0;
    } else {
        *unchanged_loops = (*unchanged_loops).wrapping_add(1);
    }
}

#[cfg(not(esp8266_hal_smartconfig_rx_trace))]
fn maybe_log_smartconfig_rx_wait(_last: &mut Option<[u32; 8]>, _unchanged_loops: &mut u32) {}

fn heap_leak<T>(value: T, label: &[u8]) -> Option<&'static mut T> {
    let ptr =
        unsafe { esp8266_hal::wifi::compat::memory::malloc(core::mem::size_of::<T>()) as *mut T };
    if ptr.is_null() {
        esp8266_hal::logger::write_bytes(b"smartconfig alloc err ");
        esp8266_hal::logger::write_bytes(label);
        esp8266_hal::logger::write_newline();
        return None;
    }
    unsafe {
        ptr.write(value);
        Some(&mut *ptr)
    }
}

fn provision_with_smartconfig(driver: &mut SmartConfigDriver) -> Option<SmartConfigCredentials> {
    let mut session = match driver.smart_config() {
        Ok(session) => session,
        Err(_error) => {
            log_line(b"smartconfig session err");
            return None;
        }
    };

    let _ = session.set_type(SmartConfigType::EspTouch);
    let _ = session.set_timeout(SmartConfigTimeout::try_from(15).expect("smartconfig timeout"));

    if session.start(&smartconfig_start_config()).is_err() {
        log_line(b"smartconfig start err");
        return None;
    }
    log_line(b"smartconfig ready");
    let mut wait_diag = log_smartconfig_debug(b"start");
    let mut rx_diag = log_smartconfig_rx_trace(b"start");

    let mut unchanged_wait_loops = 0_u32;
    let mut unchanged_rx_loops = 0_u32;
    loop {
        match session.next_event() {
            Some(WifiEvent::SmartConfigScanDone) => {
                log_line(b"smartconfig scan done");
                wait_diag = log_smartconfig_debug(b"scan-done");
                rx_diag = log_smartconfig_rx_trace(b"scan-done");
                unchanged_wait_loops = 0;
                unchanged_rx_loops = 0;
            }
            Some(WifiEvent::SmartConfigFoundChannel) => {
                log_line(b"smartconfig found channel");
                wait_diag = log_smartconfig_debug(b"found-channel");
                rx_diag = log_smartconfig_rx_trace(b"found-channel");
                unchanged_wait_loops = 0;
                unchanged_rx_loops = 0;
            }
            Some(WifiEvent::SmartConfigGotCredentials { credentials }) => {
                wait_diag = log_smartconfig_debug(b"credentials");
                rx_diag = log_smartconfig_rx_trace(b"credentials");
                unchanged_wait_loops = 0;
                unchanged_rx_loops = 0;
                esp8266_hal::logger::write_bytes(b"smartconfig credentials ssid-len=");
                esp8266_hal::logger::write_dec_u32(credentials.ssid().len() as u32);
                esp8266_hal::logger::write_newline();
                let config = match credentials.client_config() {
                    Ok(config) => config,
                    Err(_error) => {
                        log_line(b"smartconfig credentials err");
                        continue;
                    }
                };
                if session.configure_station(&config).is_err() {
                    log_line(b"smartconfig configure err");
                    continue;
                }
                if session.connect_blocking(30_000).is_err() {
                    log_line(b"smartconfig connect err");
                    continue;
                }
                log_line(b"smartconfig connected");
                log_line(b"smartconfig-credentials-ok");
                if session.stop().is_err() {
                    log_line(b"smartconfig stop err");
                }
                return Some(credentials);
            }
            Some(WifiEvent::SmartConfigSendAckDone) => {
                let _ = log_smartconfig_debug(b"ack-done");
                let _ = log_smartconfig_rx_trace(b"ack-done");
                log_line(b"smartconfig sdk ack done");
            }
            _ => {}
        }
        maybe_log_smartconfig_wait(&mut wait_diag, &mut unchanged_wait_loops);
        maybe_log_smartconfig_rx_wait(&mut rx_diag, &mut unchanged_rx_loops);
        esp8266_hal::wifi::delay_ticks(5);
    }
}

fn now() -> Instant {
    let us = unsafe { esp8266_wifi_sys::esp_timer_get_time() };
    Instant::from_millis(us / 1000)
}

fn elapsed_ms_since(start: Instant) -> u64 {
    (now() - start).total_millis()
}

fn log_dhcp_up(address: [u8; 4], prefix_len: u8) {
    esp8266_hal::logger::write_bytes(b"smartconfig dhcp up ");
    esp8266_hal::logger::write_ipv4_octets(address);
    esp8266_hal::logger::write_bytes(b"/");
    esp8266_hal::logger::write_dec_u32(prefix_len as u32);
    esp8266_hal::logger::write_newline();
}

fn run_network_ack(
    driver: SmartConfigDriver,
    credentials: &SmartConfigCredentials,
) -> Result<(), ()> {
    log_line(b"smartconfig net start");
    let mut link = match driver.sta_link() {
        Ok(link) => link,
        Err(_error) => {
            log_line(b"smartconfig net err");
            return Err(());
        }
    };
    let sta_mac = match link.mac_address() {
        Ok(mac) => mac,
        Err(_error) => {
            log_line(b"smartconfig mac err");
            return Err(());
        }
    };

    if STATIC_ACK_ENABLED {
        return send_esptouch_static_ack(&mut link, credentials, sta_mac);
    }

    let mut device = SmartConfigDevice::new(link);
    let config = SmolConfig::new(HardwareAddress::Ethernet(EthernetAddress(sta_mac)));
    let mut iface = Interface::new(config, &mut device, now());
    log_line(b"smartconfig net iface");
    iface.update_ip_addrs(|addrs| {
        let _ = addrs.push(IpCidr::new(Ipv4Address::UNSPECIFIED.into(), 0));
    });

    let storage = heap_leak([SocketStorage::EMPTY; 3], b"sockets").ok_or(())?;
    let mut sockets = SocketSet::new(&mut storage[..]);
    let dhcp_handle = sockets.add(dhcpv4::Socket::new());
    log_line(b"smartconfig dhcp socket");

    let local_ip = match wait_for_dhcp(&mut iface, &mut device, &mut sockets, dhcp_handle) {
        Some(local_ip) => local_ip,
        None => {
            log_line(b"smartconfig dhcp err");
            return Err(());
        }
    };

    send_esptouch_ack(
        &mut iface,
        &mut device,
        &mut sockets,
        credentials,
        sta_mac,
        local_ip,
    )
}

fn send_esptouch_static_ack(
    link: &mut SmartConfigLink,
    credentials: &SmartConfigCredentials,
    sta_mac: [u8; 6],
) -> Result<(), ()> {
    let ap_mac = match credentials.bssid().or_else(connected_bssid) {
        Some(bssid) => bssid,
        None => {
            log_line(b"smartconfig ack bssid err");
            return Err(());
        }
    };
    let phone = credentials.cellphone_ip().octets();

    esp8266_hal::logger::write_bytes(b"smartconfig static ack bssid ");
    write_mac(ap_mac);
    esp8266_hal::logger::write_newline();
    esp8266_hal::logger::write_bytes(b"smartconfig static ack target ");
    esp8266_hal::logger::write_ipv4_octets(phone);
    esp8266_hal::logger::write_byte(b':');
    esp8266_hal::logger::write_dec_u32(ESPTOUCH_ACK_PORT as u32);
    esp8266_hal::logger::write_newline();

    let mut frame = [0u8; 60];
    frame[0..6].copy_from_slice(&ap_mac);
    frame[6..12].copy_from_slice(&sta_mac);
    frame[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());

    let ip = &mut frame[14..34];
    ip[0] = 0x45;
    ip[1] = 0;
    ip[2..4].copy_from_slice(&(20_u16 + 8 + 11).to_be_bytes());
    ip[4..6].copy_from_slice(&0x1234_u16.to_be_bytes());
    ip[6..8].copy_from_slice(&0_u16.to_be_bytes());
    ip[8] = 64;
    ip[9] = 17;
    ip[10..12].copy_from_slice(&0_u16.to_be_bytes());
    ip[12..16].copy_from_slice(&STATIC_ACK_IP);
    ip[16..20].copy_from_slice(&phone);
    let checksum = ipv4_checksum(ip);
    ip[10..12].copy_from_slice(&checksum.to_be_bytes());

    let udp = &mut frame[34..42];
    udp[0..2].copy_from_slice(&ESPTOUCH_ACK_PORT.to_be_bytes());
    udp[2..4].copy_from_slice(&ESPTOUCH_ACK_PORT.to_be_bytes());
    udp[4..6].copy_from_slice(&(8_u16 + 11).to_be_bytes());
    udp[6..8].copy_from_slice(&0_u16.to_be_bytes());

    let payload = &mut frame[42..53];
    payload[0] = credentials.token();
    payload[1..7].copy_from_slice(&sta_mac);
    payload[7..11].copy_from_slice(&STATIC_ACK_IP);

    esp8266_hal::wifi::compat::port::set_wdev_tick_poll_enabled(true);
    esp8266_hal::wifi::delay_ticks(2);
    let mut sent = 0;
    for _ in 0..40 {
        match link.transmit(&frame) {
            Ok(()) => {
                sent += 1;
                if sent >= 5 {
                    esp8266_hal::wifi::compat::port::set_wdev_tick_poll_enabled(false);
                    log_line(b"smartconfig ack sent");
                    return Ok(());
                }
                esp8266_hal::wifi::delay_ticks(2);
            }
            Err(esp8266_hal::wifi::DriverError::WouldBlock) => {
                esp8266_hal::wifi::delay_ticks(1);
            }
            Err(_) => {
                esp8266_hal::wifi::compat::port::set_wdev_tick_poll_enabled(false);
                log_line(b"smartconfig ack send err");
                return Err(());
            }
        }
    }

    esp8266_hal::wifi::compat::port::set_wdev_tick_poll_enabled(false);
    log_line(b"smartconfig ack send err");
    Err(())
}

fn write_mac(mac: [u8; 6]) {
    for (index, byte) in mac.iter().copied().enumerate() {
        if index != 0 {
            esp8266_hal::logger::write_byte(b':');
        }
        esp8266_hal::logger::write_hex_u8(byte);
    }
}

fn connected_bssid() -> Option<[u8; 6]> {
    let mut record: esp8266_wifi_sys::wifi_ap_record_t = unsafe { core::mem::zeroed() };
    if unsafe { esp8266_wifi_sys::esp_wifi_sta_get_ap_info(&mut record) } == 0 {
        Some(record.bssid)
    } else {
        None
    }
}

fn ipv4_checksum(header: &[u8]) -> u16 {
    let mut sum = 0u32;
    let mut chunks = header.chunks_exact(2);
    for chunk in chunks.by_ref() {
        sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
    }
    if let Some(byte) = chunks.remainder().first() {
        sum += (*byte as u32) << 8;
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

fn wait_for_dhcp(
    iface: &mut Interface,
    device: &mut SmartConfigDevice,
    sockets: &mut SocketSet<'static>,
    dhcp_handle: SocketHandle,
) -> Option<[u8; 4]> {
    log_line(b"smartconfig waiting dhcp");
    let start = now();
    let mut first_poll = true;

    loop {
        let log_poll = first_poll;
        if first_poll {
            first_poll = false;
            log_line(b"smartconfig dhcp poll");
        }
        if log_poll {
            log_line(b"smartconfig dhcp poll enter");
        }
        let _ = iface.poll(now(), device, sockets);
        if log_poll {
            log_line(b"smartconfig dhcp poll exit");
        }
        {
            let dhcp = sockets.get_mut::<dhcpv4::Socket>(dhcp_handle);
            if let Some(dhcpv4::Event::Configured(config)) = dhcp.poll() {
                iface.update_ip_addrs(|addrs| {
                    if let Some(addr) = addrs.iter_mut().next() {
                        *addr = IpCidr::Ipv4(config.address);
                    }
                });
                if let Some(router) = config.router {
                    iface.routes_mut().add_default_ipv4_route(router).ok();
                }
            }
        }

        if let Some(ip) = iface.ipv4_addr() {
            if !ip.is_unspecified() {
                let prefix_len = iface
                    .ip_addrs()
                    .iter()
                    .find_map(|cidr| match cidr {
                        IpCidr::Ipv4(v4) => Some(v4.prefix_len()),
                        #[allow(unreachable_patterns)]
                        _ => None,
                    })
                    .unwrap_or(24);
                let octets = ip.octets();
                log_dhcp_up(octets, prefix_len);
                return Some(octets);
            }
        }

        if elapsed_ms_since(start) >= DHCP_TIMEOUT_MS {
            log_line(b"smartconfig dhcp timeout");
            return None;
        }

        esp8266_hal::wifi::delay_ticks(1);
    }
}

fn send_esptouch_ack(
    iface: &mut Interface,
    device: &mut SmartConfigDevice,
    sockets: &mut SocketSet<'static>,
    credentials: &SmartConfigCredentials,
    sta_mac: [u8; 6],
    local_ip: [u8; 4],
) -> Result<(), ()> {
    let rx_meta = heap_leak([udp::PacketMetadata::EMPTY; 1], b"udp-rx-meta").ok_or(())?;
    let tx_meta = heap_leak([udp::PacketMetadata::EMPTY; 1], b"udp-tx-meta").ok_or(())?;
    let rx_buf = heap_leak([0; 1], b"udp-rx").ok_or(())?;
    let tx_buf = heap_leak([0; UDP_BUF_BYTES], b"udp-tx").ok_or(())?;
    let rx_buffer = udp::PacketBuffer::new(&mut rx_meta[..], &mut rx_buf[..]);
    let tx_buffer = udp::PacketBuffer::new(&mut tx_meta[..], &mut tx_buf[..]);
    let udp_handle = sockets.add(udp::Socket::new(rx_buffer, tx_buffer));

    let mut payload = [0u8; 11];
    payload[0] = credentials.token();
    payload[1..7].copy_from_slice(&sta_mac);
    payload[7..11].copy_from_slice(&local_ip);

    let phone = credentials.cellphone_ip().octets();
    esp8266_hal::logger::write_bytes(b"smartconfig ack target ");
    esp8266_hal::logger::write_ipv4_octets(phone);
    esp8266_hal::logger::write_byte(b':');
    esp8266_hal::logger::write_dec_u32(ESPTOUCH_ACK_PORT as u32);
    esp8266_hal::logger::write_newline();

    let remote = IpEndpoint::new(
        IpAddress::Ipv4(Ipv4Address::new(phone[0], phone[1], phone[2], phone[3])),
        ESPTOUCH_ACK_PORT,
    );

    {
        let socket = sockets.get_mut::<udp::Socket>(udp_handle);
        if socket.bind(ESPTOUCH_ACK_PORT).is_err() {
            log_line(b"smartconfig ack bind err");
            return Err(());
        }
    }

    for _ in 0..5 {
        let send_start = now();
        loop {
            let send_result = {
                let socket = sockets.get_mut::<udp::Socket>(udp_handle);
                socket.send_slice(&payload, remote)
            };
            match send_result {
                Ok(()) => break,
                Err(udp::SendError::BufferFull) => {}
                Err(udp::SendError::Unaddressable) => {
                    log_line(b"smartconfig ack send err");
                    return Err(());
                }
            }

            let _ = iface.poll(now(), device, sockets);
            if elapsed_ms_since(send_start) >= 1_000 {
                log_line(b"smartconfig ack send err");
                return Err(());
            }
            esp8266_hal::wifi::delay_ticks(1);
        }

        let flush_start = now();
        loop {
            let _ = iface.poll(now(), device, sockets);
            let queued = sockets.get::<udp::Socket>(udp_handle).send_queue();
            if queued == 0 {
                break;
            }
            if elapsed_ms_since(flush_start) >= 1_000 {
                log_line(b"smartconfig ack send err");
                return Err(());
            }
            esp8266_hal::wifi::delay_ticks(1);
        }

        esp8266_hal::wifi::delay_ticks(10);
    }

    log_line(b"smartconfig ack sent");
    Ok(())
}

#[esp8266_hal::main]
fn main() -> ! {
    log_line(b"wifi_smartconfig start");

    let resources = WIFI_RESOURCES.init_with(WifiResources::new);
    let backend = Esp8266TargetBackend::new(resources);
    let driver = WifiDriver::with_backend(resources, backend)
        .initialize(InitConfig::new(14, 4, 4).expect("init config"))
        .expect("wifi init");
    log_line(b"wifi init ok");

    let mut driver = match driver.enable_sta().and_then(|driver| driver.start()) {
        Ok(driver) => driver,
        Err(_error) => {
            log_line(b"smartconfig sta start err");
            idle_forever();
        }
    };
    log_line(b"smartconfig sta started");

    let credentials = match provision_with_smartconfig(&mut driver) {
        Some(credentials) => credentials,
        None => idle_forever(),
    };
    if run_network_ack(driver, &credentials).is_err() {
        log_line(b"smartconfig ack err");
        idle_forever();
    }
    log_line(b"smartconfig-ok");
    idle_forever();
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    esp8266_hal::println!("panic {}", info);
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
