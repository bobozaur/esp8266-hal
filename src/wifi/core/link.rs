use core::task::Waker;

#[cfg(all(feature = "wifi", target_arch = "xtensa"))]
use super::packet::RawRxRecord;
use super::{DriverError, LinkState};

/// Largest Ethernet frame accepted by the ESP8266 data path.
pub const MAX_ETHERNET_FRAME_SIZE: usize = 1514;
/// Smallest Ethernet frame capacity that can carry a minimum IPv4 datagram.
pub const MIN_IPV4_ETHERNET_FRAME_SIZE: usize = 82;

pub(crate) fn validate_frame_len(len: usize) -> Result<(), DriverError> {
    if len > MAX_ETHERNET_FRAME_SIZE {
        Err(DriverError::PayloadTooLong)
    } else {
        Ok(())
    }
}

pub trait WifiDataLink {
    fn register_link_waker(&self, waker: &Waker);
    fn register_rx_waker(&self, waker: &Waker);
    fn link_state(&self) -> LinkState;
    fn mac_address(&self) -> Result<[u8; 6], DriverError>;
    fn max_frame_size(&self) -> usize;
    fn refresh_rx(&mut self) -> Result<(), DriverError>;
    fn has_rx_data(&self) -> bool;
    fn receive(&mut self, dst: &mut [u8]) -> Result<Option<usize>, DriverError>;
    fn transmit(&mut self, frame: &[u8]) -> Result<(), DriverError>;
}

#[cfg(all(feature = "wifi", target_arch = "xtensa"))]
pub(crate) trait WifiRawDataLink: WifiDataLink {
    fn receive_record(&mut self) -> Option<RawRxRecord>;
}
