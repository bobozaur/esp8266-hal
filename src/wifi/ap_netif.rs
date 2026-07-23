//! Minimal lwIP registration required by the SDK's soft-AP transmit path.
//!
//! Networking remains caller-owned. This netif exists only because the closed
//! ESP8266 Wi-Fi libraries expect an lwIP interface for AP-side frame routing.

use core::{cell::UnsafeCell, mem::MaybeUninit};

use portable_atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering};

use esp8266_wifi_sys::{self as sys, lwip};

struct NetifStorage(UnsafeCell<MaybeUninit<lwip::netif>>);
struct MacStorage(UnsafeCell<[u8; 6]>);

// The Wi-Fi backend is a process-wide singleton and mutates this storage only
// while the interface is detached from lwIP.
unsafe impl Sync for NetifStorage {}
// Access is serialized by OPERATION_STATE and the Wi-Fi singleton.
unsafe impl Sync for MacStorage {}

const TCPIP_UNINITIALIZED: u8 = 0;
const TCPIP_INITIALIZING: u8 = 1;
const TCPIP_READY: u8 = 2;
const OPERATION_IDLE: u8 = 0;
const OPERATION_RESERVED: u8 = 1;
const OPERATION_QUEUED: u8 = 2;
const OPERATION_DONE: u8 = 3;
const OPERATION_ABANDONED: u8 = 4;
const TCPIP_READY_WAIT_TICKS: usize = 200;
const OPERATION_WAIT_TICKS: usize = 200;

static TCPIP_STATE: AtomicU8 = AtomicU8::new(TCPIP_UNINITIALIZED);
static OPERATION_STATE: AtomicU8 = AtomicU8::new(OPERATION_IDLE);
static OPERATION_RESULT: AtomicI32 = AtomicI32::new(sys::ESP_FAIL);
static AP_NETIF_ADDED: AtomicBool = AtomicBool::new(false);
static AP_NETIF: NetifStorage = NetifStorage(UnsafeCell::new(MaybeUninit::uninit()));
static REQUESTED_MAC: MacStorage = MacStorage(UnsafeCell::new([0; 6]));

pub(crate) fn start(mac: [u8; 6]) -> Result<(), sys::esp_err_t> {
    init_tcpip()?;

    reserve_operation()?;
    unsafe { *REQUESTED_MAC.0.get() = mac };
    OPERATION_STATE.store(OPERATION_QUEUED, Ordering::Release);
    dispatch(start_on_tcpip_thread)
}

pub(crate) fn stop() -> Result<(), sys::esp_err_t> {
    if TCPIP_STATE.load(Ordering::Acquire) != TCPIP_READY {
        return Ok(());
    }

    reserve_operation()?;
    OPERATION_STATE.store(OPERATION_QUEUED, Ordering::Release);
    dispatch(stop_on_tcpip_thread)
}

fn reserve_operation() -> Result<(), sys::esp_err_t> {
    OPERATION_STATE
        .compare_exchange(
            OPERATION_IDLE,
            OPERATION_RESERVED,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .map(|_| ())
        .map_err(|_| sys::ESP_ERR_INVALID_STATE)
}

fn dispatch(callback: unsafe extern "C" fn(*mut core::ffi::c_void)) -> Result<(), sys::esp_err_t> {
    let queued = unsafe { lwip::tcpip_callback(Some(callback), core::ptr::null_mut()) };
    if queued != lwip::ERR_OK {
        OPERATION_STATE.store(OPERATION_IDLE, Ordering::Release);
        return Err(sys::ESP_FAIL);
    }

    if !wait_for_state(&OPERATION_STATE, OPERATION_DONE, OPERATION_WAIT_TICKS) {
        match OPERATION_STATE.compare_exchange(
            OPERATION_QUEUED,
            OPERATION_ABANDONED,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return Err(sys::ESP_ERR_TIMEOUT),
            Err(OPERATION_DONE) => {}
            Err(_) => return Err(sys::ESP_ERR_TIMEOUT),
        }
    }
    finish_dispatch()
}

fn finish_dispatch() -> Result<(), sys::esp_err_t> {
    let result = OPERATION_RESULT.load(Ordering::Acquire);
    OPERATION_STATE.store(OPERATION_IDLE, Ordering::Release);
    if result == sys::ESP_OK {
        Ok(())
    } else {
        Err(result)
    }
}

unsafe extern "C" fn start_on_tcpip_thread(_context: *mut core::ffi::c_void) {
    portable_atomic::fence(Ordering::Acquire);
    let mac = unsafe { *REQUESTED_MAC.0.get() };
    let result = start_inner(mac).map_or_else(|error| error, |()| sys::ESP_OK);
    complete_operation(result, true);
}

fn start_inner(mac: [u8; 6]) -> Result<(), sys::esp_err_t> {
    let netif = AP_NETIF.0.get().cast::<lwip::netif>();
    if !AP_NETIF_ADDED.load(Ordering::Acquire) {
        unsafe { netif.write(core::mem::zeroed()) };
    }
    unsafe { (*netif).hwaddr.copy_from_slice(&mac) };

    let ip = ipv4(192, 168, 4, 1);
    let gateway = ip;
    let netmask = ipv4(255, 255, 255, 0);

    if !AP_NETIF_ADDED.load(Ordering::Acquire) {
        let added = unsafe {
            lwip::netif_add(
                netif,
                &ip,
                &netmask,
                &gateway,
                core::ptr::null_mut(),
                Some(init_netif),
                Some(lwip::tcpip_input),
            )
        };
        if added.is_null() {
            return Err(sys::ESP_FAIL);
        }
        AP_NETIF_ADDED.store(true, Ordering::Release);
    }

    unsafe {
        lwip::netif_set_addr(netif, &ip, &netmask, &gateway);
        lwip::netif_set_link_up(netif);
        lwip::netif_set_up(netif);
    }
    Ok(())
}

unsafe extern "C" fn stop_on_tcpip_thread(_context: *mut core::ffi::c_void) {
    portable_atomic::fence(Ordering::Acquire);
    stop_inner();
    complete_operation(sys::ESP_OK, false);
}

fn complete_operation(result: sys::esp_err_t, undo_late_start: bool) {
    OPERATION_RESULT.store(result, Ordering::Release);
    if let Err(state) = OPERATION_STATE.compare_exchange(
        OPERATION_QUEUED,
        OPERATION_DONE,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        if state == OPERATION_ABANDONED {
            if undo_late_start && result == sys::ESP_OK {
                stop_inner();
            }
            OPERATION_STATE.store(OPERATION_IDLE, Ordering::Release);
        }
    }
}

fn stop_inner() {
    if !AP_NETIF_ADDED.swap(false, Ordering::AcqRel) {
        return;
    }

    let netif = AP_NETIF.0.get().cast::<lwip::netif>();
    unsafe {
        lwip::netif_set_link_down(netif);
        lwip::netif_set_down(netif);
        lwip::netif_remove(netif);
    }
}

fn init_tcpip() -> Result<(), sys::esp_err_t> {
    if TCPIP_STATE.load(Ordering::Acquire) == TCPIP_READY {
        return Ok(());
    }

    if TCPIP_STATE
        .compare_exchange(
            TCPIP_UNINITIALIZED,
            TCPIP_INITIALIZING,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_ok()
    {
        if !crate::wifi::compat::task::has_free_task_slot() {
            TCPIP_STATE.store(TCPIP_UNINITIALIZED, Ordering::Release);
            return Err(sys::ESP_ERR_NO_MEM);
        }
        unsafe { lwip::tcpip_init(Some(tcpip_ready), core::ptr::null_mut()) };
    }
    if wait_for_state(&TCPIP_STATE, TCPIP_READY, TCPIP_READY_WAIT_TICKS) {
        Ok(())
    } else {
        Err(sys::ESP_ERR_TIMEOUT)
    }
}

unsafe extern "C" fn tcpip_ready(_context: *mut core::ffi::c_void) {
    TCPIP_STATE.store(TCPIP_READY, Ordering::Release);
}

fn wait_for_state(state: &AtomicU8, expected: u8, max_ticks: usize) -> bool {
    for _ in 0..max_ticks {
        if state.load(Ordering::Acquire) == expected {
            return true;
        }
        unsafe { crate::wifi::compat::task::vTaskDelay(1) };
    }
    state.load(Ordering::Acquire) == expected
}

unsafe extern "C" fn init_netif(netif: *mut lwip::netif) -> lwip::err_t {
    let Some(netif) = (unsafe { netif.as_mut() }) else {
        return lwip::ERR_ARG;
    };

    netif.name = [b'a' as core::ffi::c_char, b'p' as core::ffi::c_char];
    netif.output = Some(lwip::etharp_output);
    netif.linkoutput = Some(link_output);
    netif.hwaddr_len = lwip::NETIF_MAX_HWADDR_LEN as u8;
    netif.mtu = 1_500;
    netif.flags =
        (lwip::NETIF_FLAG_BROADCAST | lwip::NETIF_FLAG_ETHARP | lwip::NETIF_FLAG_ETHERNET) as u8;
    lwip::ERR_OK
}

unsafe extern "C" fn link_output(
    _netif: *mut lwip::netif,
    _packet: *mut lwip::pbuf,
) -> lwip::err_t {
    lwip::ERR_OK
}

const fn ipv4(a: u8, b: u8, c: u8, d: u8) -> lwip::ip4_addr_t {
    lwip::ip4_addr_t {
        addr: u32::from_le_bytes([a, b, c, d]),
    }
}

// lwIP's timeout table references the DHCP server timer even though this
// driver deliberately does not embed the SDK DHCP server.
#[no_mangle]
extern "C" fn dhcps_coarse_tmr() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipv4_matches_lwip_little_endian_storage() {
        assert_eq!(ipv4(192, 168, 4, 1).addr, 0x0104_A8C0);
        assert_eq!(ipv4(255, 255, 255, 0).addr, 0x00FF_FFFF);
    }
}
