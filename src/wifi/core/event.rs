use heapless::Vec;

use super::{Deadline, DriverError, SmartConfigCredentials, WifiRuntimeBackend, WpsPin};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkState {
    Down,
    Up,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WifiEventKind {
    DriverReady,
    ScanDone,
    StaStarted,
    StaStopped,
    StaConnected,
    StaDisconnected,
    ApStarted,
    ApStopped,
    WpsSucceeded,
    WpsFailed,
    WpsTimedOut,
    WpsPin,
    SmartConfigScanDone,
    SmartConfigFoundChannel,
    SmartConfigGotCredentials,
    SmartConfigSendAckDone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WifiEvent {
    DriverReady,
    ScanDone { count: u8, success: bool },
    StaStarted,
    StaStopped,
    StaConnected,
    StaDisconnected { reason: Option<u8> },
    ApStarted,
    ApStopped,
    WpsSucceeded,
    WpsFailed { reason: Option<u32> },
    WpsTimedOut,
    WpsPin { pin: WpsPin },
    SmartConfigScanDone,
    SmartConfigFoundChannel,
    SmartConfigGotCredentials { credentials: SmartConfigCredentials },
    SmartConfigSendAckDone,
}

impl WifiEvent {
    pub const fn kind(self) -> WifiEventKind {
        match self {
            Self::DriverReady => WifiEventKind::DriverReady,
            Self::ScanDone { .. } => WifiEventKind::ScanDone,
            Self::StaStarted => WifiEventKind::StaStarted,
            Self::StaStopped => WifiEventKind::StaStopped,
            Self::StaConnected => WifiEventKind::StaConnected,
            Self::StaDisconnected { .. } => WifiEventKind::StaDisconnected,
            Self::ApStarted => WifiEventKind::ApStarted,
            Self::ApStopped => WifiEventKind::ApStopped,
            Self::WpsSucceeded => WifiEventKind::WpsSucceeded,
            Self::WpsFailed { .. } => WifiEventKind::WpsFailed,
            Self::WpsTimedOut => WifiEventKind::WpsTimedOut,
            Self::WpsPin { .. } => WifiEventKind::WpsPin,
            Self::SmartConfigScanDone => WifiEventKind::SmartConfigScanDone,
            Self::SmartConfigFoundChannel => WifiEventKind::SmartConfigFoundChannel,
            Self::SmartConfigGotCredentials { .. } => WifiEventKind::SmartConfigGotCredentials,
            Self::SmartConfigSendAckDone => WifiEventKind::SmartConfigSendAckDone,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct EventCursor {
    slot: usize,
    next_sequence: u64,
}

pub struct EventBroker<const EVENTS: usize, const SUBS: usize> {
    events: Vec<WifiEvent, EVENTS>,
    claimed: [bool; SUBS],
    positions: [u64; SUBS],
    first_sequence: u64,
    next_sequence: u64,
    link_state: LinkState,
    ap_link_state: LinkState,
    last_disconnect_reason: Option<u8>,
    overflow_count: u32,
    discard_count: u32,
}

impl<const EVENTS: usize, const SUBS: usize> EventBroker<EVENTS, SUBS> {
    pub const fn new() -> Self {
        Self {
            events: Vec::new(),
            claimed: [false; SUBS],
            positions: [0; SUBS],
            first_sequence: 0,
            next_sequence: 0,
            link_state: LinkState::Down,
            ap_link_state: LinkState::Down,
            last_disconnect_reason: None,
            overflow_count: 0,
            discard_count: 0,
        }
    }

    pub fn claim_cursor(&mut self) -> Result<EventCursor, DriverError> {
        for slot in 0..SUBS {
            if !self.claimed[slot] {
                self.claimed[slot] = true;
                self.positions[slot] = self.next_sequence;
                return Ok(EventCursor {
                    slot,
                    next_sequence: self.next_sequence,
                });
            }
        }

        Err(DriverError::EventOverflow)
    }

    pub fn release_cursor(&mut self, cursor: &mut EventCursor) {
        if cursor.slot < SUBS && self.claimed[cursor.slot] {
            self.claimed[cursor.slot] = false;
            self.positions[cursor.slot] = 0;
            cursor.slot = usize::MAX;
            self.compact();
        }
    }

    pub fn publish(&mut self, event: WifiEvent) -> Result<(), DriverError> {
        match event {
            WifiEvent::StaConnected => {
                self.link_state = LinkState::Up;
            }
            WifiEvent::StaDisconnected { reason } => {
                self.link_state = LinkState::Down;
                self.last_disconnect_reason = reason;
            }
            WifiEvent::StaStopped => {
                self.link_state = LinkState::Down;
            }
            WifiEvent::ApStarted => {
                self.ap_link_state = LinkState::Up;
            }
            WifiEvent::ApStopped => {
                self.ap_link_state = LinkState::Down;
            }
            _ => {}
        }

        if !self.has_claimed_cursors() {
            self.clear();
            self.discard_count = self.discard_count.wrapping_add(1);
            return Ok(());
        }

        self.compact();
        if self.events.push(event).is_err() {
            self.overflow_count = self.overflow_count.wrapping_add(1);
            return Err(DriverError::EventOverflow);
        }
        self.next_sequence += 1;
        Ok(())
    }

    pub fn try_next(&mut self, cursor: &mut EventCursor) -> Option<WifiEvent> {
        if cursor.slot >= SUBS || !self.claimed[cursor.slot] {
            return None;
        }

        let offset = cursor.next_sequence.checked_sub(self.first_sequence)?;
        let event = self.events.get(offset as usize).copied()?;

        cursor.next_sequence += 1;
        self.positions[cursor.slot] = cursor.next_sequence;
        self.compact();

        Some(event)
    }

    pub fn wait_blocking<B: WifiRuntimeBackend>(
        &mut self,
        cursor: &mut EventCursor,
        backend: &mut B,
        wanted: WifiEvent,
        timeout_ms: u32,
    ) -> Result<WifiEvent, DriverError> {
        let deadline = Deadline::new(backend.now_ms(), timeout_ms);

        loop {
            while let Some(event) = self.try_next(cursor) {
                if event == wanted {
                    return Ok(event);
                }
            }

            if deadline.is_expired(backend.now_ms()) {
                return Err(DriverError::Timeout);
            }

            backend.spin_once();
        }
    }

    pub const fn link_state(&self) -> LinkState {
        self.link_state
    }

    pub const fn ap_link_state(&self) -> LinkState {
        self.ap_link_state
    }

    pub const fn last_disconnect_reason(&self) -> Option<u8> {
        self.last_disconnect_reason
    }

    pub const fn overflow_count(&self) -> u32 {
        self.overflow_count
    }

    pub const fn discard_count(&self) -> u32 {
        self.discard_count
    }

    fn has_claimed_cursors(&self) -> bool {
        self.claimed.iter().copied().any(|claimed| claimed)
    }

    fn clear(&mut self) {
        self.events.clear();
        self.first_sequence = self.next_sequence;
    }

    fn compact(&mut self) {
        let mut min = self.next_sequence;
        let mut any_claimed = false;

        for slot in 0..SUBS {
            if self.claimed[slot] {
                any_claimed = true;
                min = min.min(self.positions[slot]);
            }
        }

        if !any_claimed {
            self.clear();
            return;
        }

        let remove = min.saturating_sub(self.first_sequence) as usize;
        if remove == 0 {
            return;
        }

        for _ in 0..remove {
            let _ = self.events.remove(0);
        }

        self.first_sequence = min;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_observes_events_in_publish_order() {
        let mut broker = EventBroker::<8, 2>::new();
        let mut cursor = broker.claim_cursor().unwrap();

        broker.publish(WifiEvent::StaStarted).unwrap();
        broker.publish(WifiEvent::StaConnected).unwrap();

        assert_eq!(broker.try_next(&mut cursor), Some(WifiEvent::StaStarted));
        assert_eq!(broker.try_next(&mut cursor), Some(WifiEvent::StaConnected));
        assert_eq!(broker.try_next(&mut cursor), None);
    }

    #[test]
    fn publish_reports_overflow_when_capacity_is_exhausted() {
        let mut broker = EventBroker::<1, 1>::new();
        let mut cursor = broker.claim_cursor().unwrap();

        broker.publish(WifiEvent::StaStarted).unwrap();
        assert_eq!(broker.try_next(&mut cursor), Some(WifiEvent::StaStarted));
        broker.publish(WifiEvent::StaConnected).unwrap();
        assert_eq!(
            broker.publish(WifiEvent::ApStarted),
            Err(DriverError::EventOverflow)
        );
    }

    #[test]
    fn independent_cursors_survive_compaction() {
        let mut broker = EventBroker::<8, 2>::new();
        let mut fast = broker.claim_cursor().unwrap();
        let mut slow = broker.claim_cursor().unwrap();

        broker.publish(WifiEvent::StaStarted).unwrap();
        broker.publish(WifiEvent::StaConnected).unwrap();

        assert_eq!(broker.try_next(&mut fast), Some(WifiEvent::StaStarted));
        assert_eq!(broker.try_next(&mut fast), Some(WifiEvent::StaConnected));

        assert_eq!(broker.try_next(&mut slow), Some(WifiEvent::StaStarted));
        assert_eq!(broker.try_next(&mut slow), Some(WifiEvent::StaConnected));
        assert_eq!(broker.try_next(&mut fast), None);
        assert_eq!(broker.try_next(&mut slow), None);
    }

    #[test]
    fn new_cursor_starts_after_existing_backlog() {
        let mut broker = EventBroker::<8, 2>::new();
        let mut first = broker.claim_cursor().unwrap();
        broker.publish(WifiEvent::StaStarted).unwrap();
        assert_eq!(broker.try_next(&mut first), Some(WifiEvent::StaStarted));

        broker.publish(WifiEvent::StaConnected).unwrap();
        let mut second = broker.claim_cursor().unwrap();

        assert_eq!(broker.try_next(&mut second), None);
        assert_eq!(broker.try_next(&mut first), Some(WifiEvent::StaConnected));
    }

    #[test]
    fn scan_done_event_preserves_count_and_status() {
        let mut broker = EventBroker::<8, 1>::new();
        let mut cursor = broker.claim_cursor().unwrap();

        broker
            .publish(WifiEvent::ScanDone {
                count: 3,
                success: true,
            })
            .unwrap();

        assert_eq!(
            broker.try_next(&mut cursor),
            Some(WifiEvent::ScanDone {
                count: 3,
                success: true,
            })
        );
    }

    #[test]
    fn ap_events_do_not_change_station_link_state() {
        let mut broker = EventBroker::<8, 1>::new();

        broker.publish(WifiEvent::ApStarted).unwrap();
        assert_eq!(broker.link_state(), LinkState::Down);

        broker.publish(WifiEvent::StaConnected).unwrap();
        assert_eq!(broker.link_state(), LinkState::Up);

        broker.publish(WifiEvent::ApStopped).unwrap();
        assert_eq!(broker.link_state(), LinkState::Up);

        broker
            .publish(WifiEvent::StaDisconnected { reason: Some(7) })
            .unwrap();
        assert_eq!(broker.link_state(), LinkState::Down);
    }

    #[test]
    fn slow_cursor_makes_overflow_visible_without_losing_old_events() {
        let mut broker = EventBroker::<2, 2>::new();
        let mut fast = broker.claim_cursor().unwrap();
        let mut slow = broker.claim_cursor().unwrap();

        broker.publish(WifiEvent::StaStarted).unwrap();
        broker.publish(WifiEvent::StaConnected).unwrap();

        assert_eq!(broker.try_next(&mut fast), Some(WifiEvent::StaStarted));
        assert_eq!(broker.try_next(&mut fast), Some(WifiEvent::StaConnected));
        assert_eq!(
            broker.publish(WifiEvent::ApStarted),
            Err(DriverError::EventOverflow)
        );
        assert_eq!(broker.overflow_count(), 1);

        assert_eq!(broker.try_next(&mut slow), Some(WifiEvent::StaStarted));
        assert_eq!(broker.try_next(&mut slow), Some(WifiEvent::StaConnected));
        assert_eq!(broker.try_next(&mut slow), None);
    }

    #[test]
    fn zero_capacity_event_queue_updates_state_and_counts_overflow() {
        let mut broker = EventBroker::<0, 1>::new();
        let _cursor = broker.claim_cursor().unwrap();

        assert_eq!(
            broker.publish(WifiEvent::StaConnected),
            Err(DriverError::EventOverflow)
        );

        assert_eq!(broker.link_state(), LinkState::Up);
        assert_eq!(broker.overflow_count(), 1);
    }

    #[test]
    fn no_subscriber_publish_updates_state_and_counts_discard() {
        let mut broker = EventBroker::<2, 1>::new();

        broker.publish(WifiEvent::StaConnected).unwrap();

        assert_eq!(broker.link_state(), LinkState::Up);
        assert_eq!(broker.discard_count(), 1);
        assert_eq!(broker.overflow_count(), 0);
    }

    #[test]
    fn released_cursor_is_invalidated() {
        let mut broker = EventBroker::<2, 1>::new();
        let mut cursor = broker.claim_cursor().unwrap();

        broker.release_cursor(&mut cursor);
        broker.publish(WifiEvent::StaStarted).unwrap();

        assert_eq!(broker.try_next(&mut cursor), None);
        assert!(broker.claim_cursor().is_ok());
    }
}
