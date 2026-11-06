#[cfg(any(feature = "wifi-esp-now", feature = "wifi-promiscuous"))]
use super::Channel;
use super::{driver::Controller, DriverError};
#[cfg(any(
    feature = "wifi-esp-now",
    feature = "wifi-promiscuous",
    feature = "wifi-wps",
    feature = "wifi-smartconfig"
))]
use super::{resources::Resources, target::Sdk};

impl Controller<'_> {
    fn claim_session(&mut self) -> Result<(), DriverError> {
        if !self.sdk.started {
            return Err(DriverError::NotStarted);
        }
        if self.sdk.session_claimed
            || self
                .resources
                .state
                .with(|s| s.scan_pending || s.connecting || s.disconnecting)
        {
            return Err(DriverError::Busy);
        }
        self.sdk.session_claimed = true;
        Ok(())
    }
    #[cfg(feature = "wifi-esp-now")]
    pub fn esp_now(&mut self) -> Result<EspNowSession<'_>, DriverError> {
        if self.resources.esp_now_peer_capacity() == 0 && self.resources.esp_now_rx_capacity() == 0
        {
            return Err(DriverError::PacketOverflow);
        }
        self.claim_session()?;
        Ok(EspNowSession {
            sdk: self.sdk,
            resources: self.resources,
        })
    }
    #[cfg(feature = "wifi-promiscuous")]
    pub fn promiscuous(&mut self) -> Result<PromiscuousSession<'_>, DriverError> {
        if self.resources.promiscuous_capacity() == 0 {
            return Err(DriverError::PacketOverflow);
        }
        self.claim_session()?;
        Ok(PromiscuousSession {
            sdk: self.sdk,
            resources: self.resources,
            filter: super::PromiscuousFilter::all(),
            control_filter: super::PromiscuousControlFilter::empty(),
        })
    }
    #[cfg(feature = "wifi-wps")]
    pub fn wps(&mut self) -> Result<WpsSession<'_>, DriverError> {
        if !self.sdk.mode.is_some_and(super::RawMode::has_station) {
            return Err(DriverError::InvalidState);
        }
        self.resources.claim_events()?;
        if let Err(error) = self.claim_session() {
            self.resources.release_events();
            return Err(error);
        }
        Ok(WpsSession {
            sdk: self.sdk,
            resources: self.resources,
        })
    }
    #[cfg(feature = "wifi-smartconfig")]
    pub fn smart_config(&mut self) -> Result<SmartConfigSession<'_>, DriverError> {
        if !self.sdk.mode.is_some_and(super::RawMode::has_station) {
            return Err(DriverError::InvalidState);
        }
        self.resources.claim_events()?;
        if let Err(error) = self.claim_session() {
            self.resources.release_events();
            return Err(error);
        }
        Ok(SmartConfigSession {
            sdk: self.sdk,
            resources: self.resources,
        })
    }
}

#[cfg(feature = "wifi-esp-now")]
/// Exclusive ESP-NOW session. Call [`Self::start`] to activate it.
/// Drop attempts to stop the session; failed cleanup keeps control claimed
/// until the controller successfully stops the radio.
pub struct EspNowSession<'a> {
    sdk: &'a mut Sdk,
    resources: &'a Resources,
}
#[cfg(feature = "wifi-esp-now")]
impl EspNowSession<'_> {
    fn ensure_active(&self) -> Result<(), DriverError> {
        if self.sdk.esp_now_initialized {
            Ok(())
        } else {
            Err(DriverError::NotStarted)
        }
    }
    pub fn start(&mut self) -> Result<(), DriverError> {
        if self.sdk.esp_now_initialized {
            return Ok(());
        }
        self.resources.reset_esp_now_runtime();
        self.sdk.enable_esp_now()
    }
    pub fn stop(&mut self) -> Result<(), DriverError> {
        self.sdk.disable_esp_now()?;
        self.resources.reset_esp_now_runtime();
        Ok(())
    }
    pub fn is_active(&self) -> bool {
        self.sdk.esp_now_initialized
    }
    pub fn set_channel(&mut self, channel: Channel) -> Result<(), DriverError> {
        self.sdk.set_channel(channel)
    }
    pub fn set_pmk(&mut self, pmk: &[u8; 16]) -> Result<(), DriverError> {
        self.ensure_active()?;
        self.sdk.set_esp_now_pmk(pmk)
    }
    pub fn add_peer(&mut self, peer: super::EspNowPeerConfig) -> Result<bool, DriverError> {
        self.ensure_active()?;
        self.validate_interface(&peer)?;
        if self.sdk.contains_esp_now_peer(peer.peer_addr()) {
            return Ok(false);
        }
        if self.sdk.esp_now_peer_count()? >= self.resources.esp_now_peer_capacity() {
            return Err(DriverError::PeerOverflow);
        }
        self.sdk.add_esp_now_peer(&peer)?;
        Ok(true)
    }
    fn validate_interface(&self, peer: &super::EspNowPeerConfig) -> Result<(), DriverError> {
        use super::feature::EspNowInterface;
        let mode = self.sdk.mode.ok_or(DriverError::InvalidState)?;
        if match peer.interface_value() {
            EspNowInterface::Station => mode.has_station(),
            EspNowInterface::AccessPoint => mode.has_ap(),
        } {
            Ok(())
        } else {
            Err(DriverError::InvalidState)
        }
    }
    pub fn update_peer(&mut self, peer: super::EspNowPeerConfig) -> Result<(), DriverError> {
        self.ensure_active()?;
        self.validate_interface(&peer)?;
        if !self.sdk.contains_esp_now_peer(peer.peer_addr()) {
            return Err(DriverError::InvalidState);
        }
        self.sdk.modify_esp_now_peer(&peer)
    }
    pub fn remove_peer(&mut self, addr: &[u8; 6]) -> Result<bool, DriverError> {
        self.ensure_active()?;
        if !self.sdk.contains_esp_now_peer(addr) {
            return Ok(false);
        }
        self.sdk.remove_esp_now_peer(addr)?;
        Ok(true)
    }
    /// Submits one packet to a configured peer. Consume [`Self::send_status`]
    /// before submitting another; successful submission does not establish delivery.
    pub fn send(&mut self, addr: &[u8; 6], payload: &[u8]) -> Result<(), DriverError> {
        self.ensure_active()?;
        if payload.len() > super::ESP_NOW_MAX_PAYLOAD {
            return Err(DriverError::PayloadTooLong);
        }
        if !self.sdk.contains_esp_now_peer(addr) {
            return Err(DriverError::InvalidState);
        }
        self.resources.reserve_esp_now_tx()?;
        if let Err(error) = self.sdk.send_esp_now(addr, payload) {
            self.resources.cancel_esp_now_tx();
            return Err(error);
        }
        Ok(())
    }
    pub fn receive(&mut self) -> Option<super::EspNowRxFrame> {
        self.resources.dequeue_esp_now_rx()
    }
    pub fn send_status(&mut self) -> Option<super::EspNowSendStatus> {
        self.resources.take_esp_now_tx_status()
    }
    /// SDK peer count, including broadcast peers. Inactive sessions return zero;
    /// active native query failures are returned to the caller.
    pub fn peer_count(&self) -> Result<usize, DriverError> {
        self.sdk.esp_now_peer_count()
    }
}
#[cfg(feature = "wifi-esp-now")]
impl Drop for EspNowSession<'_> {
    fn drop(&mut self) {
        if self.stop().is_ok() {
            self.sdk.session_claimed = false
        }
    }
}

#[cfg(feature = "wifi-promiscuous")]
/// Exclusive capture session, activated explicitly with [`Self::start`].
/// Drop attempts to stop capture; failed cleanup requires a successful radio stop.
pub struct PromiscuousSession<'a> {
    sdk: &'a mut Sdk,
    resources: &'a Resources,
    filter: super::PromiscuousFilter,
    control_filter: super::PromiscuousControlFilter,
}
#[cfg(feature = "wifi-promiscuous")]
impl PromiscuousSession<'_> {
    pub fn start(&mut self, filter: super::PromiscuousFilter) -> Result<(), DriverError> {
        if self.sdk.promiscuous_enabled {
            return Err(DriverError::Busy);
        }
        self.resources.reset_promiscuous_runtime();
        self.sdk.spin_once();
        self.sdk.spin_once();
        self.sdk.enable_promiscuous(filter, self.control_filter)?;
        self.filter = filter;
        Ok(())
    }
    pub fn stop(&mut self) -> Result<(), DriverError> {
        if self.sdk.promiscuous_enabled {
            self.sdk.disable_promiscuous()?
        }
        self.resources.reset_promiscuous_runtime();
        self.filter = super::PromiscuousFilter::empty();
        Ok(())
    }
    pub fn is_active(&self) -> bool {
        self.sdk.promiscuous_enabled
    }
    pub fn filter(&self) -> super::PromiscuousFilter {
        self.filter
    }
    pub fn control_filter(&self) -> super::PromiscuousControlFilter {
        self.control_filter
    }
    pub fn set_channel(&mut self, channel: Channel) -> Result<(), DriverError> {
        self.sdk.set_channel(channel)
    }
    pub fn set_control_filter(
        &mut self,
        filter: super::PromiscuousControlFilter,
    ) -> Result<(), DriverError> {
        if self.sdk.promiscuous_enabled {
            self.sdk.set_promiscuous_control_filter(filter)?;
        }
        self.control_filter = filter;
        Ok(())
    }
    pub fn receive(&mut self) -> Option<super::PromiscuousRxFrame> {
        self.resources.dequeue_promiscuous_rx()
    }
}
#[cfg(feature = "wifi-promiscuous")]
impl Drop for PromiscuousSession<'_> {
    fn drop(&mut self) {
        if self.stop().is_ok() {
            self.sdk.session_claimed = false
        }
    }
}

#[cfg(feature = "wifi-wps")]
/// Exclusive WPS session, activated explicitly with [`Self::start`].
/// After failure or timeout, call [`Self::stop`] before retrying. Dropping the
/// session attempts cleanup; failed cleanup requires a successful radio stop.
pub struct WpsSession<'a> {
    sdk: &'a mut Sdk,
    resources: &'a Resources,
}
#[cfg(feature = "wifi-wps")]
impl WpsSession<'_> {
    pub fn start(&mut self, config: &super::WpsConfig<'_>) -> Result<(), DriverError> {
        if self.sdk.wps_enabled {
            return Err(DriverError::Busy);
        }
        self.sdk.synchronize_events()?;
        self.resources.clear_events();
        self.resources.state.with(|s| s.station_configured = false);
        self.sdk.enable_wps(config)?;
        if let Err(error) = self.sdk.start_wps() {
            let _ = self.sdk.disable_wps();
            return Err(error);
        }
        Ok(())
    }
    pub fn stop(&mut self) -> Result<(), DriverError> {
        if self.sdk.wps_enabled {
            self.sdk.disable_wps()?
        }
        Ok(())
    }
    pub fn is_active(&self) -> bool {
        self.sdk.wps_enabled
    }
    pub fn last_disconnect_reason(&self) -> Option<u8> {
        self.resources.last_disconnect_reason()
    }
    pub fn next_event(&mut self) -> Result<Option<super::WpsEvent>, DriverError> {
        use super::{WifiEvent, WpsEvent};
        while let Some(event) = self.resources.next_event()? {
            return Ok(Some(match event {
                WifiEvent::WpsSucceeded => WpsEvent::Succeeded,
                WifiEvent::WpsFailed { reason } => WpsEvent::Failed { reason },
                WifiEvent::WpsTimedOut => WpsEvent::TimedOut,
                WifiEvent::WpsPin { pin } => WpsEvent::Pin { pin },
                WifiEvent::StaDisconnected { reason } => WpsEvent::Disconnected { reason },
                _ => continue,
            }));
        }
        Ok(None)
    }
}
#[cfg(feature = "wifi-wps")]
impl Drop for WpsSession<'_> {
    fn drop(&mut self) {
        if self.stop().is_ok() {
            self.resources.release_events();
            self.sdk.session_claimed = false
        }
    }
}

#[cfg(feature = "wifi-smartconfig")]
/// Exclusive SmartConfig session, activated explicitly with [`Self::start`].
/// Keep the session alive through station connection, IP setup, and provisioning
/// acknowledgement. DHCP and acknowledgement policy belong to the application.
/// Drop attempts cleanup; failed cleanup requires a successful radio stop.
pub struct SmartConfigSession<'a> {
    sdk: &'a mut Sdk,
    resources: &'a Resources,
}
#[cfg(feature = "wifi-smartconfig")]
impl SmartConfigSession<'_> {
    pub fn set_type(&mut self, kind: super::SmartConfigType) -> Result<(), DriverError> {
        self.sdk.set_smartconfig_type(kind)
    }
    pub fn set_timeout(&mut self, timeout: super::SmartConfigTimeout) -> Result<(), DriverError> {
        self.sdk.set_smartconfig_timeout(timeout)
    }
    pub fn set_fast_mode(&mut self, enabled: bool) -> Result<(), DriverError> {
        self.sdk.set_smartconfig_fast_mode(enabled)
    }
    pub fn start(&mut self, config: &super::SmartConfigStartConfig) -> Result<(), DriverError> {
        if self.sdk.smartconfig_active {
            return Err(DriverError::Busy);
        }
        self.sdk.synchronize_events()?;
        self.resources.clear_events();
        self.resources.clear_smartconfig_credentials();
        self.sdk.start_smartconfig(config)
    }
    pub fn configure_station(
        &mut self,
        config: &super::ClientConfig<'_>,
    ) -> Result<(), DriverError> {
        self.resources.state.with(|s| s.station_configured = false);
        self.sdk.configure_sta(config)?;
        self.resources.state.with(|s| s.station_configured = true);
        Ok(())
    }
    pub fn connect_blocking(&mut self, timeout_ms: u32) -> Result<(), DriverError> {
        Controller {
            sdk: self.sdk,
            resources: self.resources,
        }
        .connect_blocking_impl(timeout_ms, true)
    }
    pub async fn connect_async(&mut self) -> Result<(), DriverError> {
        Controller {
            sdk: self.sdk,
            resources: self.resources,
        }
        .connect_async_impl(true)
        .await
    }
    pub fn stop(&mut self) -> Result<(), DriverError> {
        if self.sdk.smartconfig_active {
            self.sdk.stop_smartconfig()?
        }
        Ok(())
    }
    pub fn is_active(&self) -> bool {
        self.sdk.smartconfig_active
    }
    pub fn read_reserved_data(&mut self, buffer: &mut [u8]) -> Result<(), DriverError> {
        self.sdk.read_smartconfig_reserved_data(buffer)
    }
    pub fn next_event(&mut self) -> Result<Option<super::SmartConfigEvent>, DriverError> {
        self.resources.next_smartconfig_event()
    }
}
#[cfg(feature = "wifi-smartconfig")]
impl Drop for SmartConfigSession<'_> {
    fn drop(&mut self) {
        if self.stop().is_ok() {
            self.resources.release_events();
            self.sdk.session_claimed = false
        }
    }
}

#[cfg(all(
    test,
    any(
        feature = "wifi-esp-now",
        feature = "wifi-promiscuous",
        feature = "wifi-wps"
    )
))]
mod tests {
    use super::super::{allocator::test_allocator, QueueConfig};
    use super::*;
    #[cfg(feature = "wifi-esp-now")]
    #[test]
    fn esp_now_native_peers_include_broadcast_preserve_modes_and_propagate_errors() {
        use super::super::EspNowPeerConfig;
        let r = Resources::try_new(
            &QueueConfig {
                esp_now_peers: 2,
                ..QueueConfig::default()
            },
            test_allocator(),
        )
        .unwrap();
        let mut sdk = Sdk::new(&r);
        let mut c = Controller {
            sdk: &mut sdk,
            resources: &r,
        };
        c.enable_station().unwrap();
        c.start_blocking(1000).unwrap();
        let mut session = c.esp_now().unwrap();
        assert_eq!(session.peer_count(), Ok(0));
        session.start().unwrap();
        let first = EspNowPeerConfig::unencrypted([2; 6]);
        assert_eq!(session.add_peer(first), Ok(true));
        // A native addition outside the public helper must immediately affect
        // count/capacity/existence; there is no Resources mirror to update.
        session
            .sdk
            .add_esp_now_peer(&EspNowPeerConfig::broadcast())
            .unwrap();
        assert_eq!(session.peer_count(), Ok(2));
        assert_eq!(session.add_peer(EspNowPeerConfig::broadcast()), Ok(false));
        assert_eq!(
            session.add_peer(EspNowPeerConfig::unencrypted([4; 6])),
            Err(DriverError::PeerOverflow)
        );
        assert_eq!(
            session.update_peer(EspNowPeerConfig::encrypted([2; 6], [7; 16])),
            Ok(())
        );
        assert_eq!(
            session.update_peer(EspNowPeerConfig::unencrypted([4; 6])),
            Err(DriverError::InvalidState)
        );
        assert_eq!(session.remove_peer(&[4; 6]), Ok(false));
        assert_eq!(
            session.add_peer(first.for_access_point()),
            Err(DriverError::InvalidState)
        );
        assert_eq!(
            session.update_peer(first.for_access_point()),
            Err(DriverError::InvalidState)
        );
        session.sdk.fail_next = Some(DriverError::Sdk(9));
        assert_eq!(session.update_peer(first), Err(DriverError::Sdk(9)));
        session.sdk.fail_next = Some(DriverError::Sdk(8));
        assert_eq!(session.remove_peer(&[2; 6]), Err(DriverError::Sdk(8)));
        assert_eq!(session.peer_count(), Ok(2));
        assert_eq!(
            session.remove_peer(&EspNowPeerConfig::BROADCAST_ADDR),
            Ok(true)
        );
        session.sdk.peer_count_error = Some(DriverError::Sdk(7));
        assert_eq!(session.peer_count(), Err(DriverError::Sdk(7)));
        assert_eq!(
            session.add_peer(EspNowPeerConfig::unencrypted([4; 6])),
            Err(DriverError::Sdk(7))
        );
        session.sdk.peer_count_error = None;
        session.sdk.fail_next = Some(DriverError::Sdk(6));
        assert_eq!(
            session.add_peer(EspNowPeerConfig::unencrypted([4; 6])),
            Err(DriverError::Sdk(6))
        );
        assert_eq!(session.peer_count(), Ok(1));
        session.stop().unwrap();
        session.sdk.peer_count_error = Some(DriverError::Sdk(5));
        assert_eq!(session.peer_count(), Ok(0));
    }
    #[cfg(feature = "wifi-esp-now")]
    #[test]
    fn esp_now_capacity_serialization_restart_and_failed_drop_are_owned_by_sdk() {
        let r = Resources::try_new(
            &QueueConfig {
                esp_now_peers: 1,
                esp_now_rx: 1,
                ..QueueConfig::default()
            },
            test_allocator(),
        )
        .unwrap();
        let mut sdk = Sdk::new(&r);
        let mut c = Controller {
            sdk: &mut sdk,
            resources: &r,
        };
        c.enable_station().unwrap();
        c.start_blocking(1000).unwrap();
        {
            let mut session = c.esp_now().unwrap();
            session.start().unwrap();
            let first = super::super::EspNowPeerConfig::unencrypted([1; 6]);
            assert_eq!(session.add_peer(first), Ok(true));
            assert_eq!(session.add_peer(first), Ok(false));
            let commands = session.sdk.commands;
            assert_eq!(
                session.add_peer(super::super::EspNowPeerConfig::unencrypted([2; 6])),
                Err(DriverError::PeerOverflow)
            );
            assert_eq!(session.sdk.commands, commands);
            assert_eq!(
                session.send(&[1; 6], &[0; 251]),
                Err(DriverError::PayloadTooLong)
            );
            session.send(&[1; 6], b"hello").unwrap();
            assert_eq!(
                session.send(&[1; 6], b"again"),
                Err(DriverError::WouldBlock)
            );
            r.complete_esp_now_tx(super::super::EspNowSendStatus::Success)
                .unwrap();
            assert_eq!(
                session.send_status(),
                Some(super::super::EspNowSendStatus::Success)
            );
            session.stop().unwrap();
            session.start().unwrap();
            assert_eq!(session.peer_count(), Ok(0));
            session.sdk.fail_next = Some(DriverError::Sdk(9));
        }
        assert!(c.sdk.session_claimed);
        assert!(c.sdk.esp_now_initialized);
        assert_eq!(c.connect_blocking(0), Err(DriverError::Busy));
        let mut records = [super::super::AccessPointRecord::default()];
        assert_eq!(
            c.scan_blocking(&super::super::ScanOptions::all(), &mut records, 0),
            Err(DriverError::Busy)
        );
        assert!(matches!(c.esp_now(), Err(DriverError::Busy)));
        c.stop().unwrap();
        assert!(!c.sdk.session_claimed);
    }
    #[cfg(feature = "wifi-promiscuous")]
    #[test]
    fn promiscuous_control_filter_is_staged_until_start_and_survives_restart() {
        let r = Resources::try_new(
            &QueueConfig {
                promiscuous_rx: 1,
                ..QueueConfig::default()
            },
            test_allocator(),
        )
        .unwrap();
        let mut sdk = Sdk::new(&r);
        let mut c = Controller {
            sdk: &mut sdk,
            resources: &r,
        };
        c.enable_station().unwrap();
        c.start_blocking(1000).unwrap();
        let mut session = c.promiscuous().unwrap();
        let count = session.sdk.commands;
        let filter = super::super::PromiscuousControlFilter::ACK;
        session.set_control_filter(filter).unwrap();
        assert_eq!(session.sdk.commands, count);
        session
            .start(super::super::PromiscuousFilter::CONTROL)
            .unwrap();
        assert_eq!(session.sdk.promiscuous_control_filter, filter);
        session.stop().unwrap();
        assert_eq!(session.control_filter(), filter);
        session
            .start(super::super::PromiscuousFilter::DATA)
            .unwrap();
        assert_eq!(session.filter(), super::super::PromiscuousFilter::DATA);
        assert_eq!(session.sdk.promiscuous_control_filter, filter);

        session.stop().unwrap();
        session
            .set_control_filter(super::super::PromiscuousControlFilter::empty())
            .unwrap();
        assert_eq!(session.sdk.promiscuous_control_filter, filter);
        session
            .start(super::super::PromiscuousFilter::CONTROL)
            .unwrap();
        assert!(session.sdk.promiscuous_control_filter.is_empty());

        session.set_control_filter(filter).unwrap();
        session.sdk.fail_next = Some(DriverError::Sdk(123));
        assert_eq!(
            session.set_control_filter(super::super::PromiscuousControlFilter::empty()),
            Err(DriverError::Sdk(123))
        );
        assert_eq!(session.control_filter(), filter);
        assert_eq!(session.sdk.promiscuous_control_filter, filter);
        drop(session);
        let mut session = c.promiscuous().unwrap();
        assert!(session.control_filter().is_empty());
        session
            .start(super::super::PromiscuousFilter::CONTROL)
            .unwrap();
        assert!(session.sdk.promiscuous_control_filter.is_empty());
    }
    #[cfg(feature = "wifi-wps")]
    #[test]
    fn wps_validity_is_authoritative_and_restart_discards_old_terminal_events() {
        let r = Resources::try_new(
            &QueueConfig {
                events: 1,
                ..QueueConfig::default()
            },
            test_allocator(),
        )
        .unwrap();
        let mut sdk = Sdk::new(&r);
        let mut c = Controller {
            sdk: &mut sdk,
            resources: &r,
        };
        c.enable_station().unwrap();
        c.start_blocking(1000).unwrap();
        let mut session = c.wps().unwrap();
        session.start(&super::super::WpsConfig::pbc()).unwrap();
        r.publish_event(super::super::WifiEvent::StaStarted)
            .unwrap();
        assert_eq!(
            r.publish_event(super::super::WifiEvent::WpsSucceeded),
            Err(DriverError::EventOverflow)
        );
        assert!(r.state.with(|s| s.station_configured));
        assert_eq!(
            session.start(&super::super::WpsConfig::pbc()),
            Err(DriverError::Busy)
        );
        assert!(r.state.with(|s| s.station_configured));
        session.stop().unwrap();
        session.start(&super::super::WpsConfig::pbc()).unwrap();
        assert!(!r.state.with(|s| s.station_configured));
        assert_eq!(session.next_event(), Ok(None));
        r.publish_event(super::super::WifiEvent::WpsTimedOut)
            .unwrap();
        assert!(!r.state.with(|s| s.station_configured));
    }
    #[cfg(feature = "wifi-wps")]
    #[test]
    fn wps_completion_preserves_credentials_and_reason() {
        let r = Resources::try_new(&QueueConfig::default(), test_allocator()).unwrap();
        let mut sdk = Sdk::new(&r);
        let mut c = Controller {
            sdk: &mut sdk,
            resources: &r,
        };
        c.enable_station().unwrap();
        c.start_blocking(1000).unwrap();
        let mut session = c.wps().unwrap();
        r.publish_event(super::super::WifiEvent::WpsSucceeded)
            .unwrap();
        assert_eq!(
            session.next_event(),
            Ok(Some(super::super::WpsEvent::Succeeded))
        );
        assert!(r.state.with(|s| s.station_configured));
        r.publish_event(super::super::WifiEvent::StaDisconnected { reason: Some(8) })
            .unwrap();
        assert_eq!(
            session.next_event(),
            Ok(Some(super::super::WpsEvent::Disconnected {
                reason: Some(8)
            }))
        );
    }
}
