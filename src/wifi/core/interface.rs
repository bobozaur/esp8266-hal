use super::{
    resources::{InterfaceId, Resources, RxPacket},
    DriverError, LinkState,
};
use core::task::Waker;

/// Independent station or access-point Ethernet endpoint borrowed from Wifi.
/// Connection policy and radio configuration remain on the controller.
///
/// RX queue slots hold native buffer leases. Disconnection discards queued
/// frames for that interface; checked-out adapter tokens remain valid until
/// released and continue consuming native RX capacity. IP addressing, DHCP,
/// sockets, and reset of upper-stack state belong to the application.
pub struct Interface<'d> {
    pub(crate) resources: &'d Resources,
    pub(crate) id: InterfaceId,
    pub(crate) mac: [u8; 6],
}

pub struct Interfaces<'d> {
    pub station: Interface<'d>,
    pub access_point: Interface<'d>,
}

impl<'d> Interface<'d> {
    /// MAC address captured during Wi-Fi initialization.
    pub fn mac_address(&self) -> [u8; 6] {
        self.mac
    }
    pub fn max_frame_size(&self) -> usize {
        self.resources.frame_bytes
    }
    pub fn link_state(&self) -> LinkState {
        self.resources.interface_link_state(self.id)
    }
    /// Replaces the link notification waiter. Register before rechecking link state.
    pub fn register_link_waker(&self, w: &Waker) {
        self.resources.register_link_waker(self.id, w)
    }
    /// Replaces the RX notification waiter. Register before rechecking RX availability.
    pub fn register_rx_waker(&self, w: &Waker) {
        self.resources.register_rx_waker(self.id, w)
    }
    pub fn has_rx_data(&self) -> bool {
        self.resources.has_rx(self.id)
    }
    pub(crate) fn receive_record(&mut self) -> Option<RxPacket<'d>> {
        self.resources.take_rx(self.id)
    }
    /// Consumes a whole frame, releasing it even if the destination is too small.
    pub fn receive(&mut self, dst: &mut [u8]) -> Result<Option<usize>, DriverError> {
        let Some(packet) = self.receive_record() else {
            return Ok(None);
        };
        let frame = packet.as_slice();
        if dst.len() < frame.len() {
            return Err(DriverError::PacketOverflow);
        }
        dst[..frame.len()].copy_from_slice(frame);
        Ok(Some(frame.len()))
    }
    pub(crate) fn tx_generation(&self) -> u32 {
        self.resources.generation(self.id)
    }
    /// Copies a complete Ethernet frame into native storage when accepted.
    ///
    /// Frames must contain at least the 14-byte Ethernet header and fit
    /// [`Self::max_frame_size`]. Valid short frames retain their logical length.
    /// [`DriverError::WouldBlock`] leaves the frame with the caller for retry;
    /// stopped or disconnected interfaces return [`DriverError::NotStarted`].
    pub fn transmit(&mut self, frame: &[u8]) -> Result<(), DriverError> {
        self.transmit_in_generation(frame, self.tx_generation())
    }
    pub(crate) fn transmit_in_generation(
        &mut self,
        frame: &[u8],
        generation: u32,
    ) -> Result<(), DriverError> {
        super::validate_frame_len(frame.len())?;
        if frame.len() > self.max_frame_size() {
            return Err(DriverError::PayloadTooLong);
        }
        let _submission = self.resources.begin_submission(self.id, generation)?;
        super::target::Sdk::transmit(self.id, frame)
    }
}

#[cfg(all(test, not(target_arch = "xtensa")))]
mod tests {
    use super::super::{allocator::test_allocator, target::TxScript, QueueConfig, WifiEvent};
    use super::*;

    #[test]
    fn transmit_rejects_incomplete_headers_and_preserves_frame_capacity() {
        let resources = Resources::try_new(
            &QueueConfig {
                frame_bytes: 82,
                ..QueueConfig::default()
            },
            test_allocator(),
        )
        .unwrap();
        resources.publish_event(WifiEvent::StaConnected).unwrap();
        let mut interface = Interface {
            resources: &resources,
            id: InterfaceId::Station,
            mac: [2; 6],
        };
        let tx = TxScript::new([Ok(()), Ok(()), Ok(()), Ok(())]);
        for len in 0..14 {
            assert_eq!(
                interface.transmit(&[0; 14][..len]),
                Err(DriverError::InvalidFrameSize)
            );
        }
        for len in [14, 18, 42, 82] {
            assert_eq!(interface.transmit(&[0; 82][..len]), Ok(()));
        }
        assert_eq!(
            interface.transmit(&[0; 83]),
            Err(DriverError::PayloadTooLong)
        );
        assert_eq!(
            interface.transmit(&[0; 1515]),
            Err(DriverError::PayloadTooLong)
        );
        let frames = tx.frames();
        assert_eq!(frames.len(), 4);
        for ((id, frame), len) in frames.iter().zip([14, 18, 42, 82]) {
            assert_eq!(*id, InterfaceId::Station);
            assert_eq!(frame.len(), len);
        }
    }
}
