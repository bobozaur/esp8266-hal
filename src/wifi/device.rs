//! [`smoltcp`] device integration over [`wifi_core`](crate::wifi_core).
//!
//! This adapter is intentionally thin and caller-owned. It consumes an owned
//! typestate link (`StaLink` or `ApLink`) and exposes it as a
//! [`smoltcp::phy::Device`] without introducing an extra channel layer.

use core::marker::PhantomData;
use core::task::Waker;

use portable_atomic::{AtomicBool, Ordering};
use smoltcp::phy::{self, DeviceCapabilities, Medium};
use smoltcp::time::Instant;

use crate::wifi_core::{ApLink, DriverError, StaLink, WifiDataLink};

/// Maximum Ethernet frame size (MTU 1500 + 14-byte header).
const MAX_FRAME_SIZE: usize = 1514;
/// WiFi TX backpressure retry budget.
const TX_RETRY_LIMIT: usize = 8;

/// A generic `smoltcp` device backed by an owned `wifi_core` link.
pub struct LinkDevice<L> {
    link: L,
    tx_backoff: AtomicBool,
}

/// A station-backed `smoltcp` device.
pub type WifiDevice<
    'd,
    M,
    B,
    const EVENTS: usize,
    const RX: usize,
    const SUBS: usize,
    const ESP_NOW_PEERS: usize = 0,
    const ESP_NOW_RX: usize = 0,
    const PROMISCUOUS_RX: usize = 0,
    const AP_RX_SLOTS: usize = 0,
    const AP_RX_BYTES: usize = 0,
> = LinkDevice<
    StaLink<
        'd,
        M,
        B,
        EVENTS,
        RX,
        SUBS,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >,
>;

/// An access-point-backed `smoltcp` device.
pub type ApWifiDevice<
    'd,
    M,
    B,
    const EVENTS: usize,
    const RX: usize,
    const SUBS: usize,
    const ESP_NOW_PEERS: usize = 0,
    const ESP_NOW_RX: usize = 0,
    const PROMISCUOUS_RX: usize = 0,
    const AP_RX_SLOTS: usize = 0,
    const AP_RX_BYTES: usize = 0,
> = LinkDevice<
    ApLink<
        'd,
        M,
        B,
        EVENTS,
        RX,
        SUBS,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >,
>;

impl<L: WifiDataLink> LinkDevice<L> {
    pub fn new(link: L) -> Self {
        Self {
            link,
            tx_backoff: AtomicBool::new(false),
        }
    }

    pub fn mac_address(&self) -> Result<[u8; 6], DriverError> {
        self.link.mac_address()
    }

    pub fn register_waker(&self, waker: &Waker) {
        self.link.register_rx_waker(waker);
    }

    pub fn has_rx_data(&self) -> bool {
        self.link.has_rx_data()
    }

    pub fn rx_drops(&self) -> u32 {
        self.link.rx_drops()
    }

    pub fn receive_frame(&mut self, dst: &mut [u8]) -> Option<usize> {
        self.link.receive(dst)
    }

    pub fn into_link(self) -> L {
        self.link
    }

    pub fn transmit_frame(&mut self, frame: &[u8]) -> Result<(), DriverError> {
        transmit_with_retry(&mut self.link, frame)
    }
}

impl<L: WifiDataLink> phy::Device for LinkDevice<L> {
    type RxToken<'a>
        = WifiRxToken<'a, L>
    where
        Self: 'a;

    type TxToken<'a>
        = WifiTxToken<'a, L>
    where
        Self: 'a;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        if !self.link.has_rx_data() {
            return None;
        }

        let link = &mut self.link as *mut L;
        Some((
            WifiRxToken {
                link,
                _lifetime: PhantomData,
            },
            WifiTxToken {
                link,
                tx_backoff: &self.tx_backoff,
                _lifetime: PhantomData,
            },
        ))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        if self.tx_backoff.swap(false, Ordering::Relaxed) {
            return None;
        }

        Some(WifiTxToken {
            link: &mut self.link as *mut L,
            tx_backoff: &self.tx_backoff,
            _lifetime: PhantomData,
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
pub struct WifiRxToken<'a, L> {
    link: *mut L,
    _lifetime: PhantomData<&'a mut L>,
}

impl<L: WifiDataLink> phy::RxToken for WifiRxToken<'_, L> {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        let mut buf = [0u8; MAX_FRAME_SIZE];
        let len = unsafe { (&mut *self.link).receive(&mut buf) }
            .expect("RxToken created but RX queue empty");

        f(&buf[..len])
    }
}

/// Token for transmitting a single frame.
pub struct WifiTxToken<'a, L> {
    link: *mut L,
    tx_backoff: &'a AtomicBool,
    _lifetime: PhantomData<&'a mut L>,
}

impl<L: WifiDataLink> phy::TxToken for WifiTxToken<'_, L> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut buf = [0u8; MAX_FRAME_SIZE];
        let frame = &mut buf[..len.min(MAX_FRAME_SIZE)];
        let result = f(frame);

        match transmit_with_retry(unsafe { &mut *self.link }, frame) {
            Ok(()) => {}
            Err(DriverError::WouldBlock) => {
                self.tx_backoff.store(true, Ordering::Relaxed);
            }
            Err(_) => {}
        }

        result
    }
}

pub(crate) unsafe fn debug_reset_sta_rx_cb_trace() {}

fn transmit_with_retry<L: WifiDataLink>(link: &mut L, frame: &[u8]) -> Result<(), DriverError> {
    let mut last = DriverError::WouldBlock;

    for _ in 0..TX_RETRY_LIMIT {
        match link.transmit(frame) {
            Ok(()) => return Ok(()),
            Err(DriverError::WouldBlock) => {
                last = DriverError::WouldBlock;
                wait_for_tx_progress();
            }
            Err(error) => return Err(error),
        }
    }

    Err(last)
}

#[inline(always)]
fn wait_for_tx_progress() {
    #[cfg(target_arch = "xtensa")]
    unsafe {
        crate::wifi::compat::task::vTaskDelay(1);
    }

    #[cfg(not(target_arch = "xtensa"))]
    core::hint::spin_loop();
}
