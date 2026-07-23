#[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps", test))]
use heapless::Vec;

#[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps", test))]
use super::DriverError;
#[cfg(feature = "wifi-wps")]
use super::WpsPin;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkState {
    Down,
    Up,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(target_arch = "xtensa"), allow(dead_code))]
pub(crate) enum WifiEvent {
    StaStarted,
    StaStopped,
    StaConnected,
    StaDisconnected {
        reason: Option<u8>,
    },
    ApStarted,
    ApStopped,
    #[cfg(feature = "wifi-wps")]
    WpsSucceeded,
    #[cfg(feature = "wifi-wps")]
    WpsFailed {
        reason: Option<u32>,
    },
    #[cfg(feature = "wifi-wps")]
    WpsTimedOut,
    #[cfg(feature = "wifi-wps")]
    WpsPin {
        pin: WpsPin,
    },
    #[cfg(feature = "wifi-smartconfig")]
    SmartConfigScanDone,
    #[cfg(feature = "wifi-smartconfig")]
    SmartConfigFoundChannel,
    #[cfg(feature = "wifi-smartconfig")]
    SmartConfigGotCredentials,
    #[cfg(feature = "wifi-smartconfig")]
    SmartConfigSendAckDone,
}

#[derive(Debug, PartialEq, Eq)]
#[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps", test))]
pub(crate) struct EventCursor {
    active: bool,
}

#[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps", test))]
pub(crate) struct EventBroker<const EVENTS: usize> {
    // Hardware note: an `Option<WifiEvent>` ring rewrite deadlocked
    // `esp_wifi_init_internal` on ESP8266 before any event was published.
    events: Vec<WifiEvent, EVENTS>,
    cursor_claimed: bool,
}

#[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps", test))]
impl<const EVENTS: usize> EventBroker<EVENTS> {
    pub const fn new() -> Self {
        Self {
            events: Vec::new(),
            cursor_claimed: false,
        }
    }

    pub fn claim_cursor(&mut self) -> Result<EventCursor, DriverError> {
        if EVENTS == 0 || self.cursor_claimed {
            return Err(DriverError::EventOverflow);
        }

        self.cursor_claimed = true;
        self.events.clear();
        Ok(EventCursor { active: true })
    }

    pub fn release_cursor(&mut self, cursor: &mut EventCursor) {
        if cursor.active && self.cursor_claimed {
            cursor.active = false;
            self.cursor_claimed = false;
            self.events.clear();
        }
    }

    #[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps"))]
    pub fn clear_cursor(&mut self, cursor: &EventCursor) {
        if cursor.active && self.cursor_claimed {
            self.events.clear();
        }
    }

    #[cfg(any(target_arch = "xtensa", test))]
    pub fn publish(&mut self, event: WifiEvent) -> Result<(), DriverError> {
        if !self.cursor_claimed {
            self.events.clear();
            return Ok(());
        }

        if self.events.push(event).is_err() {
            return Err(DriverError::EventOverflow);
        }
        Ok(())
    }

    pub fn try_next(&mut self, cursor: &mut EventCursor) -> Option<WifiEvent> {
        if !cursor.active || !self.cursor_claimed || self.events.is_empty() {
            return None;
        }

        Some(self.events.remove(0))
    }
}

#[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps", test))]
impl<const EVENTS: usize> Default for EventBroker<EVENTS> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_observes_events_in_publish_order() {
        let mut broker = EventBroker::<8>::new();
        let mut cursor = broker.claim_cursor().unwrap();

        broker.publish(WifiEvent::StaStarted).unwrap();
        broker.publish(WifiEvent::StaConnected).unwrap();

        assert_eq!(broker.try_next(&mut cursor), Some(WifiEvent::StaStarted));
        assert_eq!(broker.try_next(&mut cursor), Some(WifiEvent::StaConnected));
        assert_eq!(broker.try_next(&mut cursor), None);
    }

    #[test]
    fn publish_reports_overflow_when_capacity_is_exhausted() {
        let mut broker = EventBroker::<1>::new();
        let mut cursor = broker.claim_cursor().unwrap();

        broker.publish(WifiEvent::StaStarted).unwrap();
        assert_eq!(
            broker.publish(WifiEvent::StaConnected),
            Err(DriverError::EventOverflow)
        );
        assert_eq!(broker.try_next(&mut cursor), Some(WifiEvent::StaStarted));
    }

    #[test]
    fn queued_events_make_overflow_visible_without_losing_old_events() {
        let mut broker = EventBroker::<2>::new();
        let mut cursor = broker.claim_cursor().unwrap();

        broker.publish(WifiEvent::StaStarted).unwrap();
        broker.publish(WifiEvent::StaConnected).unwrap();

        assert_eq!(
            broker.publish(WifiEvent::ApStarted),
            Err(DriverError::EventOverflow)
        );

        assert_eq!(broker.try_next(&mut cursor), Some(WifiEvent::StaStarted));
        assert_eq!(broker.try_next(&mut cursor), Some(WifiEvent::StaConnected));
        assert_eq!(broker.try_next(&mut cursor), None);
    }

    #[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps"))]
    #[test]
    fn clearing_a_cursor_discards_the_previous_session_generation() {
        let mut broker = EventBroker::<2>::new();
        let mut cursor = broker.claim_cursor().unwrap();

        broker.publish(WifiEvent::StaConnected).unwrap();
        broker.clear_cursor(&cursor);

        assert_eq!(broker.try_next(&mut cursor), None);
    }

    #[test]
    fn zero_capacity_event_queue_rejects_subscriptions() {
        let mut broker = EventBroker::<0>::new();
        assert_eq!(broker.claim_cursor(), Err(DriverError::EventOverflow));
    }

    #[test]
    fn no_subscriber_publish_does_not_retain_backlog() {
        let mut broker = EventBroker::<2>::new();

        broker.publish(WifiEvent::StaConnected).unwrap();
        broker
            .publish(WifiEvent::StaDisconnected { reason: None })
            .unwrap();
        broker.publish(WifiEvent::ApStarted).unwrap();
        let mut cursor = broker.claim_cursor().unwrap();
        assert_eq!(broker.try_next(&mut cursor), None);
    }

    #[test]
    fn released_cursor_is_invalidated() {
        let mut broker = EventBroker::<2>::new();
        let mut cursor = broker.claim_cursor().unwrap();

        broker.release_cursor(&mut cursor);
        broker.publish(WifiEvent::StaStarted).unwrap();

        assert_eq!(broker.try_next(&mut cursor), None);
        assert!(broker.claim_cursor().is_ok());
    }
}
