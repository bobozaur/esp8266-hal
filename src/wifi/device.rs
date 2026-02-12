//! [`smoltcp`] network device integration.
//!
//! Provides [`WifiDevice`], an implementation of [`smoltcp::phy::Device`] that
//! bridges the SDK's internal TX/RX path with the smoltcp network stack.
//!
//! Packets flow directly between the WiFi driver and smoltcp via
//! `esp_wifi_internal_tx` / `esp_wifi_internal_reg_rxcb`, bypassing the SDK's
//! lwIP stack entirely.
//!
//! # RX buffering
//!
//! Received packets are tracked in a small ring of ownership metadata. The
//! internal WiFi RX callback records `(buffer, len, eb)` and returns success
//! without freeing the packet; task-level code later copies the bytes into the
//! smoltcp buffer and releases ownership via the SDK contract. If the ring is
//! full when a new packet arrives, the packet is dropped immediately.
//!
//! # Synchronization
//!
//! All access to the ring buffer is protected by critical sections (interrupt
//! disable on single-core ESP8266). The RX waker uses a separate
//! critical-section-protected slot for callback-safe wakeup signaling.

use core::cell::UnsafeCell;
use core::task::Waker;
use esp8266_wifi_sys as sys;
use portable_atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};
use smoltcp::phy::{self, DeviceCapabilities, Medium};
use smoltcp::time::Instant;

use crate::wifi::controller::WifiInterface;

/// Maximum Ethernet frame size (MTU 1500 + 14-byte header).
const MAX_FRAME_SIZE: usize = 1514;

/// Number of entries in the RX metadata ring per interface.
const RX_RING_SIZE: usize = 2;
const ETHTYPE_IPV4: u16 = 0x0800;
const ETHTYPE_ARP: u16 = 0x0806;
const ETHTYPE_IPV6: u16 = 0x86DD;

#[derive(Clone, Copy, Debug, Default)]
pub struct StaNetDiagSnapshot {
    pub rx_total: u32,
    pub rx_arp: u32,
    pub rx_ipv4: u32,
    pub rx_ipv6: u32,
    pub rx_other: u32,
    pub rx_drops: u32,
    pub tx_total: u32,
    pub tx_arp: u32,
    pub tx_ipv4: u32,
    pub tx_ipv6: u32,
    pub tx_other: u32,
    pub tx_fail: u32,
    pub last_rx_ethertype: u16,
    pub last_tx_ethertype: u16,
    pub last_tx_ret: u32,
}

// ---------------------------------------------------------------------------
// Interrupt-safe waker
// ---------------------------------------------------------------------------

/// A waker slot protected by critical sections.
///
/// Safe to register from task context and wake from ISR context on
/// single-core ESP8266 (critical section = interrupt disable).
struct CsWaker {
    inner: UnsafeCell<Option<Waker>>,
}

// SAFETY: Single-core ESP8266, all access protected by critical sections.
unsafe impl Send for CsWaker {}
unsafe impl Sync for CsWaker {}

impl CsWaker {
    const fn new() -> Self {
        Self {
            inner: UnsafeCell::new(None),
        }
    }

    /// Register a waker (task context).
    fn register(&self, waker: &Waker) {
        critical_section::with(|_| {
            let slot = unsafe { &mut *self.inner.get() };
            match slot {
                Some(existing) if existing.will_wake(waker) => {}
                _ => *slot = Some(waker.clone()),
            }
        });
    }

    /// Take and wake the stored waker. Safe to call from ISR context.
    /// The actual `wake()` call happens outside the critical section.
    fn wake(&self) {
        let waker = critical_section::with(|_| unsafe { (*self.inner.get()).take() });
        if let Some(w) = waker {
            w.wake();
        }
    }
}

// ---------------------------------------------------------------------------
// RX ring buffer
// ---------------------------------------------------------------------------

/// A single entry in the RX ring buffer.
#[derive(Clone, Copy)]
struct RxEntry {
    /// SDK-delivered frame buffer.
    buffer: *mut core::ffi::c_void,
    /// SDK-delivered ownership token. Non-null for driver-owned buffers.
    eb: *mut core::ffi::c_void,
    /// Length of the frame.
    len: u16,
}

/// Fixed-size ring buffer for received packets.
///
/// Written to from the C RX callback (ISR context) and read from task context
/// by smoltcp. All access MUST be inside a `critical_section::with` block.
pub(crate) struct RxRingBuffer {
    entries: [RxEntry; RX_RING_SIZE],
    /// Write index (next slot to write to).
    head: usize,
    /// Read index (next slot to read from).
    tail: usize,
    /// Number of entries currently occupied.
    count: usize,
    /// Number of frames dropped due to ring full (diagnostic counter).
    drops: u32,
}

impl RxRingBuffer {
    const fn new() -> Self {
        const EMPTY_ENTRY: RxEntry = RxEntry {
            buffer: core::ptr::null_mut(),
            eb: core::ptr::null_mut(),
            len: 0,
        };
        Self {
            entries: [EMPTY_ENTRY; RX_RING_SIZE],
            head: 0,
            tail: 0,
            count: 0,
            drops: 0,
        }
    }

    fn is_empty(&self) -> bool {
        self.count == 0
    }

    fn is_full(&self) -> bool {
        self.count >= RX_RING_SIZE
    }

    /// Push a received frame into the ring buffer.
    ///
    /// Returns `true` if the frame was stored, `false` if the ring is full.
    ///
    /// # Safety
    ///
    /// - Caller must be inside a critical section.
    /// - `buffer` / `eb` must remain valid until `release_rx_ownership()`.
    unsafe fn push(
        &mut self,
        buffer: *mut core::ffi::c_void,
        len: u16,
        eb: *mut core::ffi::c_void,
    ) -> bool {
        if self.is_full() {
            return false;
        }

        let entry = &mut self.entries[self.head];
        entry.buffer = buffer;
        entry.eb = eb;
        entry.len = len;

        self.head = (self.head + 1) % RX_RING_SIZE;
        self.count += 1;

        true
    }

    /// Pop the oldest frame metadata.
    ///
    /// Returns the copied length on success.
    ///
    /// # Safety
    ///
    /// Caller must be inside a critical section.
    fn pop(&mut self) -> Option<RxEntry> {
        if self.is_empty() {
            return None;
        }

        let entry = self.entries[self.tail];
        self.entries[self.tail] = RxEntry {
            buffer: core::ptr::null_mut(),
            eb: core::ptr::null_mut(),
            len: 0,
        };

        self.tail = (self.tail + 1) % RX_RING_SIZE;
        self.count -= 1;

        Some(entry)
    }
}

// ---------------------------------------------------------------------------
// Per-interface RX ring storage
// ---------------------------------------------------------------------------

/// Static RX ring buffer and waker for the STA interface.
static mut STA_RX_RING_STORAGE: RxRingBuffer = RxRingBuffer::new();
static STA_RX_RING: AtomicPtr<RxRingBuffer> = AtomicPtr::new(core::ptr::null_mut());
static STA_RX_WAKER: CsWaker = CsWaker::new();
static STA_RX_TOTAL: AtomicU32 = AtomicU32::new(0);
static STA_RX_ARP: AtomicU32 = AtomicU32::new(0);
static STA_RX_IPV4: AtomicU32 = AtomicU32::new(0);
static STA_RX_IPV6: AtomicU32 = AtomicU32::new(0);
static STA_RX_OTHER: AtomicU32 = AtomicU32::new(0);
static STA_RX_LAST_ETHERTYPE: AtomicU32 = AtomicU32::new(0);
static STA_TX_TOTAL: AtomicU32 = AtomicU32::new(0);
static STA_TX_ARP: AtomicU32 = AtomicU32::new(0);
static STA_TX_IPV4: AtomicU32 = AtomicU32::new(0);
static STA_TX_IPV6: AtomicU32 = AtomicU32::new(0);
static STA_TX_OTHER: AtomicU32 = AtomicU32::new(0);
static STA_TX_FAIL: AtomicU32 = AtomicU32::new(0);
static STA_TX_LAST_ETHERTYPE: AtomicU32 = AtomicU32::new(0);
static STA_TX_LAST_RET: AtomicU32 = AtomicU32::new(0);

#[no_mangle]
pub static mut g_sta_rx_cb_stage: u32 = 0;
#[no_mangle]
pub static mut g_sta_rx_cb_len: u32 = 0;
#[no_mangle]
pub static mut g_sta_rx_cb_buffer: u32 = 0;
#[no_mangle]
pub static mut g_sta_rx_cb_eb: u32 = 0;

/// Static RX ring buffer and waker for the AP interface.
#[cfg(feature = "wifi-ap")]
static mut AP_RX_RING_STORAGE: RxRingBuffer = RxRingBuffer::new();
#[cfg(feature = "wifi-ap")]
static AP_RX_RING: AtomicPtr<RxRingBuffer> = AtomicPtr::new(core::ptr::null_mut());
#[cfg(feature = "wifi-ap")]
static AP_RX_WAKER: CsWaker = CsWaker::new();

#[inline(always)]
fn ap_rx_ring() -> *mut RxRingBuffer {
    #[cfg(feature = "wifi-ap")]
    {
        AP_RX_RING.load(Ordering::Acquire)
    }

    #[cfg(not(feature = "wifi-ap"))]
    {
        panic!("AP RX ring requires wifi-ap feature")
    }
}

#[inline(always)]
fn classify_ethertype(buffer: *const u8, len: usize) -> u16 {
    if buffer.is_null() || len < 14 {
        return 0;
    }

    unsafe { u16::from_be_bytes([*buffer.add(12), *buffer.add(13)]) }
}

#[inline(always)]
fn count_ethertype(
    ethertype: u16,
    arp: &AtomicU32,
    ipv4: &AtomicU32,
    ipv6: &AtomicU32,
    other: &AtomicU32,
) {
    match ethertype {
        ETHTYPE_ARP => {
            arp.fetch_add(1, Ordering::Relaxed);
        }
        ETHTYPE_IPV4 => {
            ipv4.fetch_add(1, Ordering::Relaxed);
        }
        ETHTYPE_IPV6 => {
            ipv6.fetch_add(1, Ordering::Relaxed);
        }
        _ => {
            other.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[inline(always)]
fn record_net_rx(interface: WifiInterface, buffer: *mut core::ffi::c_void, len: u16) {
    if !matches!(interface, WifiInterface::Sta) {
        return;
    }

    STA_RX_TOTAL.fetch_add(1, Ordering::Relaxed);
    let ethertype = classify_ethertype(buffer as *const u8, len as usize);
    STA_RX_LAST_ETHERTYPE.store(ethertype as u32, Ordering::Relaxed);
    count_ethertype(
        ethertype,
        &STA_RX_ARP,
        &STA_RX_IPV4,
        &STA_RX_IPV6,
        &STA_RX_OTHER,
    );
}

pub(crate) fn record_net_tx(interface: WifiInterface, buffer: *const u8, len: usize, ret: i32) {
    if !matches!(interface, WifiInterface::Sta) {
        return;
    }

    STA_TX_TOTAL.fetch_add(1, Ordering::Relaxed);
    let ethertype = classify_ethertype(buffer, len);
    STA_TX_LAST_ETHERTYPE.store(ethertype as u32, Ordering::Relaxed);
    STA_TX_LAST_RET.store(ret as u32, Ordering::Relaxed);
    count_ethertype(
        ethertype,
        &STA_TX_ARP,
        &STA_TX_IPV4,
        &STA_TX_IPV6,
        &STA_TX_OTHER,
    );
    if ret != 0 {
        STA_TX_FAIL.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn clear_sta_net_diag() {
    STA_RX_TOTAL.store(0, Ordering::Relaxed);
    STA_RX_ARP.store(0, Ordering::Relaxed);
    STA_RX_IPV4.store(0, Ordering::Relaxed);
    STA_RX_IPV6.store(0, Ordering::Relaxed);
    STA_RX_OTHER.store(0, Ordering::Relaxed);
    STA_RX_LAST_ETHERTYPE.store(0, Ordering::Relaxed);
    STA_TX_TOTAL.store(0, Ordering::Relaxed);
    STA_TX_ARP.store(0, Ordering::Relaxed);
    STA_TX_IPV4.store(0, Ordering::Relaxed);
    STA_TX_IPV6.store(0, Ordering::Relaxed);
    STA_TX_OTHER.store(0, Ordering::Relaxed);
    STA_TX_FAIL.store(0, Ordering::Relaxed);
    STA_TX_LAST_ETHERTYPE.store(0, Ordering::Relaxed);
    STA_TX_LAST_RET.store(0, Ordering::Relaxed);
    critical_section::with(|_| {
        let ring = STA_RX_RING.load(Ordering::Relaxed);
        if !ring.is_null() {
            unsafe { &mut *ring }.drops = 0;
        }
    });
}

pub fn sta_net_diag_snapshot() -> StaNetDiagSnapshot {
    let rx_drops = critical_section::with(|_| {
        let ring = STA_RX_RING.load(Ordering::Relaxed);
        if ring.is_null() {
            0
        } else {
            unsafe { &*ring }.drops
        }
    });

    StaNetDiagSnapshot {
        rx_total: STA_RX_TOTAL.load(Ordering::Relaxed),
        rx_arp: STA_RX_ARP.load(Ordering::Relaxed),
        rx_ipv4: STA_RX_IPV4.load(Ordering::Relaxed),
        rx_ipv6: STA_RX_IPV6.load(Ordering::Relaxed),
        rx_other: STA_RX_OTHER.load(Ordering::Relaxed),
        rx_drops,
        tx_total: STA_TX_TOTAL.load(Ordering::Relaxed),
        tx_arp: STA_TX_ARP.load(Ordering::Relaxed),
        tx_ipv4: STA_TX_IPV4.load(Ordering::Relaxed),
        tx_ipv6: STA_TX_IPV6.load(Ordering::Relaxed),
        tx_other: STA_TX_OTHER.load(Ordering::Relaxed),
        tx_fail: STA_TX_FAIL.load(Ordering::Relaxed),
        last_rx_ethertype: STA_RX_LAST_ETHERTYPE.load(Ordering::Relaxed) as u16,
        last_tx_ethertype: STA_TX_LAST_ETHERTYPE.load(Ordering::Relaxed) as u16,
        last_tx_ret: STA_TX_LAST_RET.load(Ordering::Relaxed),
    }
}

fn ensure_sta_rx_ring() -> Result<*mut RxRingBuffer, crate::wifi::WifiError> {
    let existing = STA_RX_RING.load(Ordering::Acquire);
    if !existing.is_null() {
        return Ok(existing);
    }

    critical_section::with(|_| {
        let existing = STA_RX_RING.load(Ordering::Relaxed);
        if !existing.is_null() {
            return Ok(existing);
        }

        let ring = unsafe { &raw mut STA_RX_RING_STORAGE };
        unsafe { ring.write(RxRingBuffer::new()) };
        STA_RX_RING.store(ring, Ordering::Release);
        Ok(ring)
    })
}

#[cfg(feature = "wifi-ap")]
fn ensure_ap_rx_ring() -> Result<*mut RxRingBuffer, crate::wifi::WifiError> {
    let existing = AP_RX_RING.load(Ordering::Acquire);
    if !existing.is_null() {
        return Ok(existing);
    }

    critical_section::with(|_| {
        let existing = AP_RX_RING.load(Ordering::Relaxed);
        if !existing.is_null() {
            return Ok(existing);
        }

        let ring = unsafe { &raw mut AP_RX_RING_STORAGE };
        unsafe { ring.write(RxRingBuffer::new()) };
        AP_RX_RING.store(ring, Ordering::Release);
        Ok(ring)
    })
}

#[inline(always)]
fn ap_rx_waker() -> &'static CsWaker {
    #[cfg(feature = "wifi-ap")]
    {
        &AP_RX_WAKER
    }

    #[cfg(not(feature = "wifi-ap"))]
    {
        panic!("AP RX waker requires wifi-ap feature")
    }
}

pub(crate) unsafe fn debug_reset_sta_rx_cb_trace() {
    unsafe {
        g_sta_rx_cb_stage = 0;
        g_sta_rx_cb_len = 0;
        g_sta_rx_cb_buffer = 0;
        g_sta_rx_cb_eb = 0;
    }
}

// ---------------------------------------------------------------------------
// WifiDevice
// ---------------------------------------------------------------------------

/// Tracks whether the last TX attempt failed, providing simple backpressure.
/// When set, `transmit()` returns `None` until the next successful poll
/// clears it.
static TX_BACKOFF: AtomicBool = AtomicBool::new(false);

/// A smoltcp network device backed by the ESP8266 WiFi driver.
///
/// # Synchronization
///
/// Wakers live in static storage so C callback pointers remain valid even if
/// this struct is moved. The large RX rings are allocated lazily on first RX
/// registration and then leaked for the lifetime of the program.
/// All access is protected by critical sections.
pub struct WifiDevice {
    interface: WifiInterface,
}

// SAFETY: ESP8266 is single-core. The device is just an interface selector;
// the actual shared state lives in statics protected by critical sections.
unsafe impl Send for WifiDevice {}

impl WifiDevice {
    /// Create a new WiFi network device for the given interface.
    ///
    /// The WiFi driver must already be started. Call
    /// [`register_rx_callback`](Self::register_rx_callback) to begin receiving
    /// packets.
    pub fn new(interface: WifiInterface) -> Self {
        Self { interface }
    }

    /// Get the static ring buffer for this device's interface.
    fn rx_ring(&self) -> *mut RxRingBuffer {
        match self.interface {
            WifiInterface::Sta => STA_RX_RING.load(Ordering::Acquire),
            WifiInterface::Ap => ap_rx_ring(),
        }
    }

    /// Get the static waker for this device's interface.
    fn rx_waker(&self) -> &'static CsWaker {
        match self.interface {
            WifiInterface::Sta => &STA_RX_WAKER,
            WifiInterface::Ap => ap_rx_waker(),
        }
    }

    /// Register the internal RX callback with the SDK.
    ///
    /// This must be called after `esp_wifi_start()` so that received packets
    /// are forwarded to this device.
    pub fn register_rx_callback(&self) -> Result<(), crate::wifi::WifiError> {
        match self.interface {
            WifiInterface::Sta => {
                let _ = ensure_sta_rx_ring()?;
            }
            WifiInterface::Ap => {
                #[cfg(feature = "wifi-ap")]
                {
                    let _ = ensure_ap_rx_ring()?;
                }

                #[cfg(not(feature = "wifi-ap"))]
                {
                    return Err(crate::wifi::WifiError::WrongMode);
                }
            }
        }

        let cb = match self.interface {
            WifiInterface::Sta => rx_callback_sta as unsafe extern "C" fn(_, _, _) -> _,
            WifiInterface::Ap => {
                #[cfg(feature = "wifi-ap")]
                {
                    rx_callback_ap as unsafe extern "C" fn(_, _, _) -> _
                }

                #[cfg(not(feature = "wifi-ap"))]
                {
                    return Err(crate::wifi::WifiError::WrongMode);
                }
            }
        };

        crate::wifi::EspError::check(unsafe {
            sys::esp_wifi_internal_reg_rxcb(self.interface.to_raw(), Some(cb))
        })?;
        Ok(())
    }

    /// Get the WiFi interface this device is bound to.
    pub fn interface(&self) -> WifiInterface {
        self.interface
    }

    /// Register a waker to be notified when a packet is received.
    pub fn register_waker(&self, waker: &Waker) {
        self.rx_waker().register(waker);
    }

    /// Return the number of RX frames dropped due to a full ring buffer.
    ///
    /// This is a diagnostic counter that wraps at `u32::MAX`. Useful for
    /// detecting sustained back-pressure from slow packet processing.
    pub fn rx_drops(&self) -> u32 {
        critical_section::with(|_| {
            let ring = self.rx_ring();
            if ring.is_null() {
                0
            } else {
                unsafe { &*ring }.drops
            }
        })
    }

    /// Returns true if there are packets available to receive.
    pub fn has_rx_data(&self) -> bool {
        critical_section::with(|_| {
            let ring = self.rx_ring();
            !ring.is_null() && !unsafe { &*ring }.is_empty()
        })
    }

    /// Pop one frame from the ring buffer.
    pub(crate) fn pop_frame(&self, buf: &mut [u8]) -> Option<usize> {
        let entry = critical_section::with(|_| {
            let ring = self.rx_ring();
            if ring.is_null() {
                None
            } else {
                unsafe { &mut *ring }.pop()
            }
        })?;

        let len = entry.len as usize;
        let copy_len = len.min(buf.len());
        debug_assert_eq!(copy_len, len);
        if copy_len > 0 && !entry.buffer.is_null() {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    entry.buffer as *const u8,
                    buf.as_mut_ptr(),
                    copy_len,
                );
            }
        }
        unsafe { release_rx_ownership(entry.buffer, entry.eb) };
        Some(copy_len)
    }
}

#[inline(always)]
unsafe fn release_rx_ownership(buffer: *mut core::ffi::c_void, eb: *mut core::ffi::c_void) {
    if !eb.is_null() {
        unsafe { sys::esp_wifi_internal_free_rx_buffer(eb) };
    } else if !buffer.is_null() {
        unsafe { crate::wifi::compat::memory::free(buffer) };
    }
}

// ---------------------------------------------------------------------------
// C callbacks — deliver into static ring buffers
// ---------------------------------------------------------------------------

/// Deliver a received frame into the given static ring buffer.
///
/// # Safety
///
/// `ring` must point to a valid `RxRingBuffer` (our statics always do).
/// `buffer` is the SDK-delivered frame data. Ownership is retained for task
/// context if the ring accepts the packet, or released immediately on drop.
unsafe fn deliver_rx(
    interface: WifiInterface,
    ring: *mut RxRingBuffer,
    waker: &CsWaker,
    buffer: *mut core::ffi::c_void,
    len: u16,
    eb: *mut core::ffi::c_void,
) -> sys::esp_err_t {
    if !buffer.is_null() && len > 0 {
        unsafe {
            g_sta_rx_cb_stage = b'E' as u32;
            g_sta_rx_cb_len = len as u32;
            g_sta_rx_cb_buffer = buffer as usize as u32;
            g_sta_rx_cb_eb = eb as usize as u32;
        }
        record_net_rx(interface, buffer, len);
        let stored = critical_section::with(|_| {
            let ring = unsafe { &mut *ring };
            unsafe { ring.push(buffer, len, eb) }
        });
        unsafe {
            g_sta_rx_cb_stage = if stored { b'C' as u32 } else { b'D' as u32 };
        }
        if stored {
            waker.wake();
            unsafe {
                g_sta_rx_cb_stage = b'W' as u32;
            }
        } else {
            unsafe { release_rx_ownership(buffer, eb) };
            unsafe {
                g_sta_rx_cb_stage = b'R' as u32;
            }
            // Ring full after the ownership decision — release immediately
            // and count the drop.
            critical_section::with(|_| {
                let ring = unsafe { &mut *ring };
                ring.drops = ring.drops.wrapping_add(1);
            });
        }
    } else {
        // Invalid frame -- release immediately.
        unsafe {
            g_sta_rx_cb_stage = b'I' as u32;
            g_sta_rx_cb_len = len as u32;
            g_sta_rx_cb_buffer = buffer as usize as u32;
            g_sta_rx_cb_eb = eb as usize as u32;
        }
        unsafe { release_rx_ownership(buffer, eb) };
    }

    sys::ESP_OK
}

/// C callback for STA interface packets.
unsafe extern "C" fn rx_callback_sta(
    buffer: *mut core::ffi::c_void,
    len: u16,
    eb: *mut core::ffi::c_void,
) -> sys::esp_err_t {
    let ring = STA_RX_RING.load(Ordering::Acquire);
    if ring.is_null() {
        unsafe { release_rx_ownership(buffer, eb) };
        return sys::ESP_OK;
    }
    unsafe { deliver_rx(WifiInterface::Sta, ring, &STA_RX_WAKER, buffer, len, eb) }
}

/// C callback for AP interface packets.
#[cfg(feature = "wifi-ap")]
unsafe extern "C" fn rx_callback_ap(
    buffer: *mut core::ffi::c_void,
    len: u16,
    eb: *mut core::ffi::c_void,
) -> sys::esp_err_t {
    let ring = ap_rx_ring();
    if ring.is_null() {
        unsafe { release_rx_ownership(buffer, eb) };
        return sys::ESP_OK;
    }
    unsafe { deliver_rx(WifiInterface::Ap, ring, ap_rx_waker(), buffer, len, eb) }
}

// ---------------------------------------------------------------------------
// smoltcp phy::Device implementation
// ---------------------------------------------------------------------------

impl phy::Device for WifiDevice {
    type RxToken<'a>
        = WifiRxToken<'a>
    where
        Self: 'a;
    type TxToken<'a>
        = WifiTxToken<'a>
    where
        Self: 'a;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        if !self.has_rx_data() {
            return None;
        }

        Some((
            WifiRxToken { device: self },
            WifiTxToken {
                interface: self.interface,
                _lifetime: core::marker::PhantomData,
            },
        ))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        // If the last TX failed (SDK queue full), skip this round so the
        // SDK can drain. The backoff is cleared on the next receive() call
        // or explicit clear_tx_backoff().
        if TX_BACKOFF.load(Ordering::Relaxed) {
            TX_BACKOFF.store(false, Ordering::Relaxed);
            return None;
        }
        Some(WifiTxToken {
            interface: self.interface,
            _lifetime: core::marker::PhantomData,
        })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = 1500;
        caps.max_burst_size = Some(1);
        caps
    }
}

/// Token for receiving a single frame.
pub struct WifiRxToken<'a> {
    device: &'a WifiDevice,
}

impl<'a> phy::RxToken for WifiRxToken<'a> {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        // Pop frame: data is copied out of the ring inside a critical section.
        let mut buf = [0u8; MAX_FRAME_SIZE];
        let len = self
            .device
            .pop_frame(&mut buf)
            .expect("RxToken created but ring empty");

        f(&buf[..len])
    }
}

/// Token for transmitting a single frame.
pub struct WifiTxToken<'a> {
    interface: WifiInterface,
    _lifetime: core::marker::PhantomData<&'a ()>,
}

impl<'a> phy::TxToken for WifiTxToken<'a> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut buf = [0u8; MAX_FRAME_SIZE];
        let frame = &mut buf[..len.min(MAX_FRAME_SIZE)];
        let result = f(frame);

        let ret = unsafe {
            sys::esp_wifi_internal_tx(
                self.interface.to_raw(),
                frame.as_mut_ptr() as *mut core::ffi::c_void,
                frame.len() as u16,
            )
        };
        record_net_tx(self.interface, frame.as_ptr(), frame.len(), ret);

        if ret != 0 {
            // Signal backpressure so the next transmit() call backs off.
            TX_BACKOFF.store(true, Ordering::Relaxed);
            log::warn!("WiFi TX failed: {}", ret);
        }

        result
    }
}
