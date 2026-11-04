//! Caller-polled smoltcp integration for either Wi-Fi interface.
//!
//! One caller-supplied TX slot preserves a frame when SDK admission returns
//! WouldBlock. The adapter performs no allocation.
//! Subsequent device polls retry it before issuing another transmit token; the
//! application must keep polling smoltcp while traffic is pending.

use super::neighbor::{EthernetNeighborHint, EthernetNeighborHints, ETHERNET_ARP_IPV4_FRAME_LEN};
use crate::wifi_core::{DriverError, Interface, LinkState, RxPacket};
use core::task::Waker;
use smoltcp::{
    phy::{self, DeviceCapabilities, Medium},
    time::Instant,
};

/// A concrete station or access-point smoltcp device.
pub struct WifiDevice<'d, 'tx> {
    interface: Interface<'d>,
    pending_tx: PendingTx<'tx>,
    generation: u32,
    neighbor_hints: EthernetNeighborHints,
}

struct PendingTx<'d> {
    bytes: &'d mut [u8],
    len: usize,
}

impl<'d, 'tx> WifiDevice<'d, 'tx> {
    /// Use caller-owned storage for the single pending transmit frame.
    ///
    /// The buffer must fit [`Interface::max_frame_size`], otherwise this returns
    /// [`DriverError::InvalidFrameSize`]. Extra capacity is unused. Keeping the
    /// buffer borrowed prevents reuse while a frame is waiting for SDK admission.
    pub fn new(interface: Interface<'d>, tx_buffer: &'tx mut [u8]) -> Result<Self, DriverError> {
        let bytes = tx_buffer
            .get_mut(..interface.max_frame_size())
            .ok_or(DriverError::InvalidFrameSize)?;
        let mac = interface.mac_address();
        let generation = interface.tx_generation();
        Ok(Self {
            interface,
            generation,
            pending_tx: PendingTx { bytes, len: 0 },
            neighbor_hints: EthernetNeighborHints::new(Some(mac)),
        })
    }

    pub fn mac_address(&self) -> [u8; 6] {
        self.interface.mac_address()
    }

    pub fn register_waker(&self, waker: &Waker) {
        self.interface.register_rx_waker(waker);
        self.interface.register_link_waker(waker);
    }

    pub fn has_rx_data(&self) -> bool {
        self.neighbor_hints.has_pending() || self.interface.has_rx_data()
    }

    pub fn receive_frame(&mut self, dst: &mut [u8]) -> Result<Option<usize>, DriverError> {
        self.interface.receive(dst)
    }

    pub fn transmit_frame(&mut self, frame: &[u8]) -> Result<(), DriverError> {
        self.poll_transmit()?;
        if self.pending_tx.len != 0 {
            return Err(DriverError::WouldBlock);
        }
        self.interface.transmit(frame)
    }

    /// Retry the buffered frame once. Returns true while it still needs polling.
    pub fn poll_transmit(&mut self) -> Result<bool, DriverError> {
        self.synchronize_generation();
        if self.pending_tx.len == 0 {
            return Ok(false);
        }
        match self.interface.transmit_in_generation(
            &self.pending_tx.bytes[..self.pending_tx.len],
            self.generation,
        ) {
            Ok(()) => {
                self.pending_tx.len = 0;
                Ok(false)
            }
            Err(DriverError::WouldBlock) => Ok(true),
            Err(error) => {
                self.pending_tx.len = 0;
                Err(error)
            }
        }
    }

    pub fn into_interface(self) -> Interface<'d> {
        self.interface
    }

    fn synchronize_generation(&mut self) {
        let generation = self.interface.tx_generation();
        if self.generation != generation {
            self.pending_tx.len = 0;
            self.neighbor_hints.clear();
            self.generation = generation;
        }
    }
}

impl<'d, 'tx> phy::Device for WifiDevice<'d, 'tx> {
    type RxToken<'a>
        = WifiRxToken<'d>
    where
        Self: 'a;
    type TxToken<'a>
        = WifiTxToken<'a, 'd, 'tx>
    where
        Self: 'a;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        // A receive token includes a TX token, so reserve the sole TX slot first.
        let _ = self.poll_transmit();
        if self.pending_tx.len != 0 || self.interface.link_state() != LinkState::Up {
            return None;
        }
        let frame = if let Some(hint) = self.neighbor_hints.take_pending() {
            WifiRxFrame::NeighborHint(hint)
        } else {
            let record = self.interface.receive_record()?;
            self.neighbor_hints.observe(record.as_slice());
            WifiRxFrame::Record(record)
        };
        Some((
            WifiRxToken { frame },
            WifiTxToken {
                generation: self.generation,
                interface: &mut self.interface,
                pending: &mut self.pending_tx,
            },
        ))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        let _ = self.poll_transmit();
        if self.pending_tx.len != 0 || self.interface.link_state() != LinkState::Up {
            return None;
        }
        Some(WifiTxToken {
            generation: self.generation,
            interface: &mut self.interface,
            pending: &mut self.pending_tx,
        })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = self.interface.max_frame_size();
        caps.max_burst_size = Some(1);
        caps
    }
}

pub struct WifiRxToken<'d> {
    frame: WifiRxFrame<'d>,
}
enum WifiRxFrame<'d> {
    Record(RxPacket<'d>),
    NeighborHint(EthernetNeighborHint),
}

impl phy::RxToken for WifiRxToken<'_> {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        match self.frame {
            WifiRxFrame::Record(record) => f(record.as_slice()),
            WifiRxFrame::NeighborHint(hint) => {
                let mut frame = [0; ETHERNET_ARP_IPV4_FRAME_LEN];
                let len = hint.emit_arp_reply(&mut frame).unwrap_or(0);
                f(&frame[..len])
            }
        }
    }
}

pub struct WifiTxToken<'a, 'd, 'tx> {
    interface: &'a mut Interface<'d>,
    pending: &'a mut PendingTx<'tx>,
    generation: u32,
}

impl phy::TxToken for WifiTxToken<'_, '_, '_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let frame = &mut self.pending.bytes[..len];
        let result = f(frame);
        // The controller can stop/reconfigure inside f; generation validation
        // happens at submission, after that independently borrowed control work.
        match self
            .interface
            .transmit_in_generation(frame, self.generation)
        {
            Err(DriverError::WouldBlock) => self.pending.len = len,
            Ok(()) | Err(_) => self.pending.len = 0,
        }
        result
    }
}
