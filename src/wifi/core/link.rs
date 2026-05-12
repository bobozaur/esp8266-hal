use core::task::Waker;

use super::{DriverError, LinkState, RawRxRecord};

pub trait WifiDataLink {
    fn register_link_waker(&self, waker: &Waker);
    fn register_rx_waker(&self, waker: &Waker);
    fn link_state(&self) -> LinkState;
    fn mac_address(&self) -> Result<[u8; 6], DriverError>;
    fn refresh_rx(&mut self) -> Result<(), DriverError>;
    fn has_rx_data(&self) -> bool;
    fn queued_rx(&self) -> usize;
    fn rx_drops(&self) -> u32;
    fn receive_raw(&mut self) -> Option<RawRxRecord>;
    fn receive(&mut self, dst: &mut [u8]) -> Option<usize> {
        self.receive_raw().map(|record| record.copy_into(dst))
    }
    fn transmit(&mut self, frame: &[u8]) -> Result<(), DriverError>;
}
