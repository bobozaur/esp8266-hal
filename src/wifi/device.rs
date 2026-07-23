//! [`smoltcp`] device integration over [`wifi_core`](crate::wifi_core).
//!
//! This adapter is intentionally thin and caller-owned. It consumes an owned
//! typestate link (`StaLink` or `ApLink`) and exposes it as a
//! [`smoltcp::phy::Device`] without introducing an extra channel layer.

use core::{marker::PhantomData, task::Waker};

use portable_atomic::{AtomicBool, Ordering};
use smoltcp::phy::{self, DeviceCapabilities, Medium};
use smoltcp::time::Instant;

use crate::wifi_core::{
    ApLink, DriverError, EthernetNeighborHint, EthernetNeighborHints, RawRxRecord, StaLink,
    WifiDataLink, WifiRawDataLink, ETHERNET_ARP_IPV4_FRAME_LEN, MAX_ETHERNET_FRAME_SIZE,
    MIN_IPV4_ETHERNET_FRAME_SIZE,
};

/// WiFi TX backpressure retry budget.
const TX_RETRY_LIMIT: usize = 8;

/// A generic `smoltcp` device backed by an owned `wifi_core` link.
pub struct LinkDevice<L> {
    link: L,
    tx_backoff: AtomicBool,
    neighbor_hints: EthernetNeighborHints,
}

/// A station-backed `smoltcp` device.
pub type WifiDevice<
    'd,
    M,
    B,
    const EVENTS: usize,
    const RX: usize,
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
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >,
>;

impl<L: WifiDataLink> LinkDevice<L> {
    pub fn new(link: L) -> Result<Self, DriverError> {
        let frame_size = link.max_frame_size();
        if !(MIN_IPV4_ETHERNET_FRAME_SIZE..=MAX_ETHERNET_FRAME_SIZE).contains(&frame_size) {
            return Err(DriverError::InvalidFrameSize);
        }
        let local_mac = link.mac_address()?;
        Ok(Self {
            link,
            tx_backoff: AtomicBool::new(false),
            neighbor_hints: EthernetNeighborHints::new(Some(local_mac)),
        })
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

    pub fn receive_frame(&mut self, dst: &mut [u8]) -> Result<Option<usize>, DriverError> {
        self.link.receive(dst)
    }

    pub fn into_link(self) -> L {
        self.link
    }

    pub fn transmit_frame(&mut self, frame: &[u8]) -> Result<(), DriverError> {
        transmit_with_retry(&mut self.link, frame)
    }
}

#[allow(private_bounds)]
impl<L: WifiRawDataLink> phy::Device for LinkDevice<L> {
    type RxToken<'a>
        = WifiRxToken<'a>
    where
        Self: 'a;

    type TxToken<'a>
        = WifiTxToken<'a, L>
    where
        Self: 'a;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let frame = if let Some(hint) = self.neighbor_hints.take_pending() {
            WifiRxFrame::NeighborHint(hint)
        } else {
            let max_frame_size = self.link.max_frame_size();
            let record = loop {
                let record = self.link.receive_record()?;
                if record.len() <= max_frame_size {
                    break record;
                }
            };
            self.neighbor_hints.observe(record_frame(&record));
            WifiRxFrame::Record(record)
        };

        let link = &mut self.link;
        Some((
            WifiRxToken {
                frame,
                device_borrow: PhantomData,
            },
            WifiTxToken {
                link,
                tx_backoff: &self.tx_backoff,
            },
        ))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        if self.tx_backoff.swap(false, Ordering::Relaxed) {
            return None;
        }

        Some(WifiTxToken {
            link: &mut self.link,
            tx_backoff: &self.tx_backoff,
        })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = self.link.max_frame_size();
        caps.max_burst_size = Some(1);
        caps
    }
}

/// Token for receiving a single frame.
pub struct WifiRxToken<'a> {
    frame: WifiRxFrame,
    device_borrow: PhantomData<&'a mut ()>,
}

enum WifiRxFrame {
    Record(RawRxRecord),
    NeighborHint(EthernetNeighborHint),
}

impl phy::RxToken for WifiRxToken<'_> {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        match self.frame {
            WifiRxFrame::Record(record) => f(record_frame(&record)),
            WifiRxFrame::NeighborHint(hint) => {
                let mut frame = [0u8; ETHERNET_ARP_IPV4_FRAME_LEN];
                let len = hint.emit_arp_reply(&mut frame).unwrap_or(0);
                f(&frame[..len])
            }
        }
    }
}

/// Token for transmitting a single frame.
pub struct WifiTxToken<'a, L> {
    link: &'a mut L,
    tx_backoff: &'a AtomicBool,
}

impl<L: WifiDataLink> phy::TxToken for WifiTxToken<'_, L> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut buf = [0u8; MAX_ETHERNET_FRAME_SIZE];
        let frame = &mut buf[..len];
        let result = f(frame);

        match transmit_with_retry(self.link, frame) {
            Ok(()) => {}
            Err(DriverError::WouldBlock) => {
                self.tx_backoff.store(true, Ordering::Relaxed);
            }
            Err(_) => {}
        }

        result
    }
}

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

fn record_frame(record: &RawRxRecord) -> &[u8] {
    debug_assert!(record.len() <= MAX_ETHERNET_FRAME_SIZE);
    record.as_slice()
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
