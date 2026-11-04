#[cfg(any(target_arch = "xtensa", test))]
use super::DriverError;
/// Largest Ethernet frame accepted by the ESP8266 data path.
pub const MAX_ETHERNET_FRAME_SIZE: usize = 1514;
/// Smallest Ethernet capacity that can carry a minimum IPv4 datagram.
pub const MIN_IPV4_ETHERNET_FRAME_SIZE: usize = 82;
#[cfg(any(target_arch = "xtensa", test))]
pub(crate) fn validate_frame_len(len: usize) -> Result<(), DriverError> {
    if len < 14 {
        Err(DriverError::InvalidFrameSize)
    } else if len > MAX_ETHERNET_FRAME_SIZE {
        Err(DriverError::PayloadTooLong)
    } else {
        Ok(())
    }
}
