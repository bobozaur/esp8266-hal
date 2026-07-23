use core::task::Waker;

use portable_atomic::{AtomicU32, Ordering};

use super::event::LinkState;
#[cfg(any(target_arch = "xtensa", test, feature = "wifi-smartconfig"))]
use super::event::WifiEvent;
#[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps"))]
use super::event::{EventBroker, EventCursor};
#[cfg(any(feature = "wifi-esp-now", feature = "wifi-promiscuous"))]
use super::feature::BoundedQueue;
#[cfg(feature = "wifi-promiscuous")]
use super::feature::PromiscuousRxFrame;
#[cfg(feature = "wifi-esp-now")]
use super::feature::{EspNowPeer, EspNowPeerTable, EspNowRxFrame, EspNowSendStatus};
use super::locked::Locked;
#[cfg(any(all(feature = "wifi", target_arch = "xtensa"), test))]
use super::packet::{release_owned_rx_slot, RxPushError};
use super::packet::{OwnedRxPool, RawRxRecord, RxRing};
use super::waker::CallbackWaker;
#[cfg(any(
    target_arch = "xtensa",
    test,
    feature = "wifi-esp-now",
    feature = "wifi-smartconfig",
    feature = "wifi-wps"
))]
use super::DriverError;
#[cfg(feature = "wifi-smartconfig")]
use super::{SmartConfigCredentials, SmartConfigEvent};

const STA_LINK_UP: u32 = 1 << 0;
const AP_LINK_UP: u32 = 1 << 1;
const DISCONNECT_REASON_SHIFT: u32 = 16;
const DISCONNECT_REASON_MASK: u32 = 0x1ff << DISCONNECT_REASON_SHIFT;
const STA_RX_LEN_MASK: u32 = 0xffff;
const AP_RX_LEN_SHIFT: u32 = 16;
const AP_RX_LEN_MASK: u32 = 0xffff << AP_RX_LEN_SHIFT;

#[cfg(feature = "wifi-esp-now")]
#[derive(Clone, Copy)]
#[cfg_attr(not(target_arch = "xtensa"), allow(dead_code))]
enum EspNowTxSlot {
    Idle,
    Pending,
    Complete(EspNowSendStatus),
}

/// Caller-owned storage for Wi-Fi events, packets, and optional feature data.
///
/// The first two capacities size the provisioning-event ring and STA zero-copy
/// RX ring. The remaining capacities default to zero and allocate storage only
/// when ESP-NOW, promiscuous capture, or copied AP RX is used.
/// Keeping this object in static storage makes all driver memory explicit and
/// avoids hidden heap allocation on packet and event paths.
pub struct WifiResources<
    const EVENTS: usize,
    const RX: usize,
    const ESP_NOW_PEERS: usize = 0,
    const ESP_NOW_RX: usize = 0,
    const PROMISCUOUS_RX: usize = 0,
    const AP_RX_SLOTS: usize = 0,
    const AP_RX_BYTES: usize = 0,
> {
    #[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps"))]
    pub(crate) events: Locked<EventBroker<EVENTS>>,
    pub(crate) sta_rx: Locked<RxRing<RX>>,
    pub(crate) ap_rx: Locked<RxRing<AP_RX_SLOTS>>,
    #[cfg_attr(not(target_arch = "xtensa"), allow(dead_code))]
    pub(crate) ap_rx_owned: Locked<OwnedRxPool<AP_RX_SLOTS, AP_RX_BYTES>>,
    #[cfg(feature = "wifi-esp-now")]
    esp_now_peers: Locked<EspNowPeerTable<ESP_NOW_PEERS>>,
    #[cfg(feature = "wifi-esp-now")]
    esp_now_rx: Locked<BoundedQueue<EspNowRxFrame, ESP_NOW_RX>>,
    #[cfg(feature = "wifi-esp-now")]
    esp_now_tx: Locked<EspNowTxSlot>,
    #[cfg(feature = "wifi-promiscuous")]
    promiscuous_rx: Locked<BoundedQueue<PromiscuousRxFrame, PROMISCUOUS_RX>>,
    #[cfg(feature = "wifi-smartconfig")]
    smartconfig_credentials: Locked<Option<SmartConfigCredentials>>,
    // Atomic mirrors let interrupt and SDK task callbacks publish independently
    // without taking the public state locks.
    event_mirror: AtomicU32,
    rx_len_mirror: AtomicU32,
    sta_link_waker: CallbackWaker,
    ap_link_waker: CallbackWaker,
    sta_rx_waker: CallbackWaker,
    ap_rx_waker: CallbackWaker,
}

impl<
        const EVENTS: usize,
        const RX: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    > Default
    for WifiResources<
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
{
    fn default() -> Self {
        Self::new()
    }
}

impl<
        const EVENTS: usize,
        const RX: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    >
    WifiResources<EVENTS, RX, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX, AP_RX_SLOTS, AP_RX_BYTES>
{
    pub const fn new() -> Self {
        Self {
            #[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps"))]
            events: Locked::new(EventBroker::new()),
            sta_rx: Locked::new(RxRing::new()),
            ap_rx: Locked::new(RxRing::new()),
            ap_rx_owned: Locked::new(OwnedRxPool::new()),
            #[cfg(feature = "wifi-esp-now")]
            esp_now_peers: Locked::new(EspNowPeerTable::new()),
            #[cfg(feature = "wifi-esp-now")]
            esp_now_rx: Locked::new(BoundedQueue::new()),
            #[cfg(feature = "wifi-esp-now")]
            esp_now_tx: Locked::new(EspNowTxSlot::Idle),
            #[cfg(feature = "wifi-promiscuous")]
            promiscuous_rx: Locked::new(BoundedQueue::new()),
            #[cfg(feature = "wifi-smartconfig")]
            smartconfig_credentials: Locked::new(None),
            event_mirror: AtomicU32::new(0),
            rx_len_mirror: AtomicU32::new(0),
            sta_link_waker: CallbackWaker::new(),
            ap_link_waker: CallbackWaker::new(),
            sta_rx_waker: CallbackWaker::new(),
            ap_rx_waker: CallbackWaker::new(),
        }
    }

    pub const fn event_capacity(&self) -> usize {
        EVENTS
    }

    pub const fn rx_capacity(&self) -> usize {
        RX
    }

    #[cfg(feature = "wifi-esp-now")]
    pub const fn esp_now_peer_capacity(&self) -> usize {
        ESP_NOW_PEERS
    }

    #[cfg(feature = "wifi-esp-now")]
    pub const fn esp_now_rx_capacity(&self) -> usize {
        ESP_NOW_RX
    }

    #[cfg(feature = "wifi-promiscuous")]
    pub const fn promiscuous_rx_capacity(&self) -> usize {
        PROMISCUOUS_RX
    }

    pub const fn ap_rx_copy_capacity(&self) -> usize {
        AP_RX_SLOTS
    }

    pub const fn ap_rx_copy_frame_bytes(&self) -> usize {
        AP_RX_BYTES
    }

    pub(crate) fn queued_rx(&self) -> usize {
        decode_sta_rx_len(self.rx_len_mirror.load(Ordering::Relaxed))
    }

    pub(crate) fn queued_ap_rx(&self) -> usize {
        decode_ap_rx_len(self.rx_len_mirror.load(Ordering::Relaxed))
    }

    #[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps"))]
    pub(crate) fn claim_event_cursor(&self) -> Result<EventCursor, DriverError> {
        self.events.with(EventBroker::claim_cursor)
    }

    #[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps"))]
    pub(crate) fn release_event_cursor(&self, cursor: &mut EventCursor) {
        self.events.with(|events| events.release_cursor(cursor));
    }

    #[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps"))]
    pub(crate) fn clear_event_cursor(&self, cursor: &EventCursor) {
        self.events.with(|events| events.clear_cursor(cursor));
    }

    #[cfg(any(target_arch = "xtensa", test))]
    pub(crate) fn publish_event(&self, event: WifiEvent) -> Result<(), DriverError> {
        #[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps"))]
        let result = self.events.with(|events| {
            let result = events.publish(event);
            self.update_event_mirrors(event);
            result
        });
        #[cfg(not(any(feature = "wifi-smartconfig", feature = "wifi-wps")))]
        let result = {
            self.update_event_mirrors(event);
            Ok(())
        };
        match event {
            WifiEvent::StaStarted
            | WifiEvent::StaStopped
            | WifiEvent::StaConnected
            | WifiEvent::StaDisconnected { .. } => self.sta_link_waker.wake(),
            WifiEvent::ApStarted | WifiEvent::ApStopped => self.ap_link_waker.wake(),
            #[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps"))]
            _ => {}
        }
        result
    }

    pub(crate) fn register_sta_link_waker(&self, waker: &Waker) {
        self.sta_link_waker.register(waker);
    }

    pub(crate) fn register_ap_link_waker(&self, waker: &Waker) {
        self.ap_link_waker.register(waker);
    }

    pub fn link_state(&self) -> LinkState {
        decode_sta_link_state(self.event_mirror.load(Ordering::Relaxed))
    }

    pub fn ap_link_state(&self) -> LinkState {
        decode_ap_link_state(self.event_mirror.load(Ordering::Relaxed))
    }

    pub fn last_disconnect_reason(&self) -> Option<u8> {
        decode_disconnect_reason(self.event_mirror.load(Ordering::Relaxed))
    }

    #[cfg(any(all(feature = "wifi", target_arch = "xtensa"), test))]
    pub(crate) fn enqueue_rx(&self, record: RawRxRecord) -> Result<(), DriverError> {
        let result = self.sta_rx.with(|rx| {
            let result = rx.try_push(record);
            self.update_sta_rx_mirror(rx.len());
            result
        });

        match result {
            Ok(()) => {
                self.sta_rx_waker.wake();
                Ok(())
            }
            Err(rejected) => {
                let error = release_rejected_rx(rejected);
                Err(error)
            }
        }
    }

    #[cfg(any(all(feature = "wifi", target_arch = "xtensa"), test))]
    pub(crate) fn enqueue_ap_rx_copy(&self, frame: &[u8]) -> Result<(), DriverError> {
        let lease = match self.ap_rx_owned.with(|owned| owned.try_acquire_copy(frame)) {
            Ok(lease) => lease,
            Err(error) => return Err(error),
        };

        let record = unsafe {
            RawRxRecord::from_raw(
                lease.buffer,
                lease.len,
                lease.slot,
                Some(release_owned_rx_slot::<AP_RX_BYTES>),
            )
        };

        let result = self.ap_rx.with(|rx| {
            let result = rx.try_push(record);
            self.update_ap_rx_mirror(rx.len());
            result
        });

        match result {
            Ok(()) => {
                self.ap_rx_waker.wake();
                Ok(())
            }
            Err(rejected) => {
                let error = release_rejected_rx(rejected);
                Err(error)
            }
        }
    }

    pub(crate) fn register_rx_waker(&self, waker: &Waker) {
        self.sta_rx_waker.register(waker);
    }

    pub(crate) fn register_ap_rx_waker(&self, waker: &Waker) {
        self.ap_rx_waker.register(waker);
    }

    pub(crate) fn dequeue_rx(&self) -> Option<RawRxRecord> {
        self.sta_rx.with(|rx| {
            let record = rx.pop();
            self.update_sta_rx_mirror(rx.len());
            record
        })
    }

    pub(crate) fn dequeue_ap_rx(&self) -> Option<RawRxRecord> {
        self.ap_rx.with(|rx| {
            let record = rx.pop();
            self.update_ap_rx_mirror(rx.len());
            record
        })
    }

    pub(crate) fn drain_sta_rx(&self) {
        while self.dequeue_rx().is_some() {}
    }

    pub(crate) fn drain_ap_rx(&self) {
        while self.dequeue_ap_rx().is_some() {}
    }

    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn register_esp_now_peer(&self, peer: EspNowPeer) -> Result<bool, DriverError> {
        self.esp_now_peers.with(|peers| peers.insert(peer))
    }

    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn remove_esp_now_peer(&self, peer_addr: &[u8; 6]) -> bool {
        self.esp_now_peers.with(|peers| peers.remove(peer_addr))
    }

    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn contains_esp_now_peer(&self, peer_addr: &[u8; 6]) -> bool {
        self.esp_now_peers
            .with_ref(|peers| peers.contains(peer_addr))
    }

    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn esp_now_peer_count(&self) -> usize {
        self.esp_now_peers.with_ref(EspNowPeerTable::len)
    }

    #[cfg(feature = "wifi-esp-now")]
    #[cfg(any(target_arch = "xtensa", test))]
    pub(crate) fn enqueue_esp_now_rx(&self, frame: EspNowRxFrame) -> Result<(), DriverError> {
        self.esp_now_rx.with(|queue| queue.try_push(frame))
    }

    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn dequeue_esp_now_rx(&self) -> Option<EspNowRxFrame> {
        self.esp_now_rx.with(BoundedQueue::pop)
    }

    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn reserve_esp_now_tx(&self) -> Result<(), DriverError> {
        self.esp_now_tx.with(|slot| match slot {
            EspNowTxSlot::Idle => {
                *slot = EspNowTxSlot::Pending;
                Ok(())
            }
            EspNowTxSlot::Pending | EspNowTxSlot::Complete(_) => Err(DriverError::WouldBlock),
        })
    }

    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn cancel_esp_now_tx(&self) {
        self.esp_now_tx.with(|slot| {
            if matches!(slot, EspNowTxSlot::Pending) {
                *slot = EspNowTxSlot::Idle;
            }
        });
    }

    #[cfg(feature = "wifi-esp-now")]
    #[cfg(any(target_arch = "xtensa", test))]
    pub(crate) fn complete_esp_now_tx(&self, status: EspNowSendStatus) -> Result<(), DriverError> {
        self.esp_now_tx.with(|slot| match slot {
            EspNowTxSlot::Pending => {
                *slot = EspNowTxSlot::Complete(status);
                Ok(())
            }
            EspNowTxSlot::Idle | EspNowTxSlot::Complete(_) => Err(DriverError::BackendRejected),
        })
    }

    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn take_esp_now_tx_status(&self) -> Option<EspNowSendStatus> {
        self.esp_now_tx.with(|slot| match *slot {
            EspNowTxSlot::Complete(status) => {
                *slot = EspNowTxSlot::Idle;
                Some(status)
            }
            EspNowTxSlot::Idle | EspNowTxSlot::Pending => None,
        })
    }

    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn reset_esp_now_runtime(&self) {
        self.esp_now_peers
            .with(|peers| *peers = EspNowPeerTable::new());
        self.esp_now_rx.with(|queue| *queue = BoundedQueue::new());
        self.esp_now_tx.with(|slot| *slot = EspNowTxSlot::Idle);
    }

    #[cfg(feature = "wifi-promiscuous")]
    #[cfg(any(target_arch = "xtensa", test))]
    pub(crate) fn enqueue_promiscuous_rx(
        &self,
        frame: PromiscuousRxFrame,
    ) -> Result<(), DriverError> {
        self.promiscuous_rx.with(|queue| queue.try_push(frame))
    }

    #[cfg(feature = "wifi-promiscuous")]
    pub(crate) fn dequeue_promiscuous_rx(&self) -> Option<PromiscuousRxFrame> {
        self.promiscuous_rx.with(BoundedQueue::pop)
    }

    #[cfg(feature = "wifi-promiscuous")]
    pub(crate) fn reset_promiscuous_runtime(&self) {
        self.promiscuous_rx
            .with(|queue| *queue = BoundedQueue::new());
    }

    #[cfg(feature = "wifi-smartconfig")]
    pub(crate) fn clear_smartconfig_credentials(&self) {
        self.smartconfig_credentials.with(|slot| *slot = None);
    }

    #[cfg(feature = "wifi-smartconfig")]
    #[cfg(any(target_arch = "xtensa", test))]
    pub(crate) fn publish_smartconfig_credentials(
        &self,
        credentials: SmartConfigCredentials,
    ) -> Result<(), DriverError> {
        let stored = self.smartconfig_credentials.with(|slot| {
            if slot.is_some() {
                false
            } else {
                *slot = Some(credentials);
                true
            }
        });
        if !stored {
            return Err(DriverError::EventOverflow);
        }

        if let Err(error) = self.publish_event(WifiEvent::SmartConfigGotCredentials) {
            self.clear_smartconfig_credentials();
            return Err(error);
        }
        Ok(())
    }

    #[cfg(feature = "wifi-smartconfig")]
    pub(crate) fn next_smartconfig_event(
        &self,
        cursor: &mut EventCursor,
    ) -> Option<SmartConfigEvent> {
        self.events.with(|events| {
            while let Some(event) = events.try_next(cursor) {
                let event = match event {
                    WifiEvent::SmartConfigScanDone => SmartConfigEvent::ScanDone,
                    WifiEvent::SmartConfigFoundChannel => SmartConfigEvent::FoundChannel,
                    WifiEvent::SmartConfigGotCredentials => {
                        let credentials = self.smartconfig_credentials.with(Option::take)?;
                        SmartConfigEvent::GotCredentials(credentials)
                    }
                    WifiEvent::SmartConfigSendAckDone => SmartConfigEvent::SendAckDone,
                    _ => continue,
                };
                return Some(event);
            }
            None
        })
    }

    #[cfg(all(feature = "wifi", target_arch = "xtensa"))]
    pub(crate) fn reset_runtime(&self) {
        while let Some(record) = self.dequeue_rx() {
            drop(record);
        }
        while let Some(record) = self.dequeue_ap_rx() {
            drop(record);
        }
        self.sta_rx.with(|rx| *rx = RxRing::new());
        self.ap_rx.with(|rx| *rx = RxRing::new());
        self.ap_rx_owned.with(|owned| *owned = OwnedRxPool::new());
        #[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps"))]
        self.events.with(|events| *events = EventBroker::new());
        #[cfg(feature = "wifi-esp-now")]
        self.reset_esp_now_runtime();
        #[cfg(feature = "wifi-promiscuous")]
        self.reset_promiscuous_runtime();
        #[cfg(feature = "wifi-smartconfig")]
        self.clear_smartconfig_credentials();
        self.event_mirror.store(0, Ordering::Relaxed);
        self.rx_len_mirror.store(0, Ordering::Relaxed);
        self.sta_link_waker.clear();
        self.ap_link_waker.clear();
        self.sta_rx_waker.clear();
        self.ap_rx_waker.clear();
    }

    #[cfg(any(target_arch = "xtensa", test))]
    fn update_event_mirrors(&self, event: WifiEvent) {
        let _ =
            self.event_mirror
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |mut mirror| {
                    match event {
                        WifiEvent::StaConnected => mirror |= STA_LINK_UP,
                        WifiEvent::StaDisconnected { reason } => {
                            mirror &= !STA_LINK_UP;
                            mirror = encode_disconnect_reason(mirror, reason);
                        }
                        WifiEvent::StaStopped => mirror &= !STA_LINK_UP,
                        WifiEvent::ApStarted => mirror |= AP_LINK_UP,
                        WifiEvent::ApStopped => mirror &= !AP_LINK_UP,
                        _ => return None,
                    }
                    Some(mirror)
                });
    }

    fn update_sta_rx_mirror(&self, len: usize) {
        debug_assert!(len <= STA_RX_LEN_MASK as usize);
        let mut mirror = self.rx_len_mirror.load(Ordering::Relaxed);
        mirror &= !STA_RX_LEN_MASK;
        mirror |= (len as u32) & STA_RX_LEN_MASK;
        self.rx_len_mirror.store(mirror, Ordering::Relaxed);
    }

    fn update_ap_rx_mirror(&self, len: usize) {
        debug_assert!(len <= STA_RX_LEN_MASK as usize);
        let mut mirror = self.rx_len_mirror.load(Ordering::Relaxed);
        mirror &= !AP_RX_LEN_MASK;
        mirror |= ((len as u32) << AP_RX_LEN_SHIFT) & AP_RX_LEN_MASK;
        self.rx_len_mirror.store(mirror, Ordering::Relaxed);
    }
}

#[cfg(any(all(feature = "wifi", target_arch = "xtensa"), test))]
fn release_rejected_rx(rejected: RxPushError) -> DriverError {
    let error = rejected.error();
    rejected.release();
    error
}

const fn decode_sta_link_state(mirror: u32) -> LinkState {
    decode_link_bit(mirror & STA_LINK_UP)
}

const fn decode_ap_link_state(mirror: u32) -> LinkState {
    decode_link_bit(mirror & AP_LINK_UP)
}

const fn decode_link_bit(bit: u32) -> LinkState {
    if bit == 0 {
        LinkState::Down
    } else {
        LinkState::Up
    }
}

#[cfg(any(target_arch = "xtensa", test))]
const fn encode_disconnect_reason(mirror: u32, reason: Option<u8>) -> u32 {
    let encoded = match reason {
        Some(reason) => reason as u32 + 1,
        None => 0,
    };
    (mirror & !DISCONNECT_REASON_MASK) | (encoded << DISCONNECT_REASON_SHIFT)
}

const fn decode_disconnect_reason(mirror: u32) -> Option<u8> {
    let reason = (mirror & DISCONNECT_REASON_MASK) >> DISCONNECT_REASON_SHIFT;
    if reason == 0 {
        None
    } else {
        Some((reason - 1) as u8)
    }
}

const fn decode_sta_rx_len(mirror: u32) -> usize {
    (mirror & STA_RX_LEN_MASK) as usize
}

const fn decode_ap_rx_len(mirror: u32) -> usize {
    ((mirror & AP_RX_LEN_MASK) >> AP_RX_LEN_SHIFT) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "wifi-esp-now")]
    use crate::wifi_core::EspNowSendStatus;
    #[cfg(feature = "wifi-smartconfig")]
    use crate::wifi_core::{Ipv4Addr, SmartConfigType};
    #[cfg(feature = "wifi-promiscuous")]
    use crate::wifi_core::{PromiscuousFrameInfo, PromiscuousPacketType};

    #[test]
    fn published_link_events_update_read_only_state() {
        let resources = WifiResources::<4, 2>::new();

        assert_eq!(resources.link_state(), LinkState::Down);
        assert_eq!(resources.last_disconnect_reason(), None);

        resources.publish_event(WifiEvent::StaConnected).unwrap();
        assert_eq!(resources.link_state(), LinkState::Up);

        resources
            .publish_event(WifiEvent::StaDisconnected { reason: Some(7) })
            .unwrap();
        assert_eq!(resources.link_state(), LinkState::Down);
        assert_eq!(resources.last_disconnect_reason(), Some(7));

        resources.publish_event(WifiEvent::ApStarted).unwrap();
        assert_eq!(resources.ap_link_state(), LinkState::Up);
    }

    #[cfg(feature = "wifi-esp-now")]
    #[test]
    fn esp_now_peer_storage_is_bounded_and_deduplicated() {
        let resources = WifiResources::<4, 1, 2, 0, 0>::new();
        let first = EspNowPeer::unencrypted([1, 2, 3, 4, 5, 6]);
        let second = EspNowPeer::encrypted([6, 5, 4, 3, 2, 1]);

        assert!(resources.register_esp_now_peer(first).unwrap());
        assert!(!resources.register_esp_now_peer(first).unwrap());
        assert!(resources.register_esp_now_peer(second).unwrap());
        assert_eq!(resources.esp_now_peer_count(), 2);
        assert_eq!(
            resources.register_esp_now_peer(EspNowPeer::unencrypted([9; 6])),
            Err(DriverError::PeerOverflow)
        );
        assert!(resources.remove_esp_now_peer(&[1, 2, 3, 4, 5, 6]));
        assert!(!resources.contains_esp_now_peer(&[1, 2, 3, 4, 5, 6]));
    }

    #[cfg(feature = "wifi-esp-now")]
    #[test]
    fn esp_now_queues_are_bounded() {
        let resources = WifiResources::<4, 1, 0, 1, 0>::new();
        let frame = EspNowRxFrame::new([1, 2, 3, 4, 5, 6], b"hello").unwrap();
        resources.enqueue_esp_now_rx(frame).unwrap();
        assert_eq!(
            resources.enqueue_esp_now_rx(EspNowRxFrame::new([6; 6], b"again").unwrap()),
            Err(DriverError::PacketOverflow)
        );
        assert_eq!(resources.dequeue_esp_now_rx().unwrap().payload(), b"hello");

        resources.reserve_esp_now_tx().unwrap();
        assert_eq!(resources.reserve_esp_now_tx(), Err(DriverError::WouldBlock));
        resources
            .complete_esp_now_tx(EspNowSendStatus::Success)
            .unwrap();
        assert_eq!(
            resources.take_esp_now_tx_status(),
            Some(EspNowSendStatus::Success)
        );

        resources.enqueue_esp_now_rx(frame).unwrap();
        resources.reserve_esp_now_tx().unwrap();
        resources
            .complete_esp_now_tx(EspNowSendStatus::Failed)
            .unwrap();
        resources.reset_esp_now_runtime();
        assert!(resources.dequeue_esp_now_rx().is_none());
        assert!(resources.take_esp_now_tx_status().is_none());

        assert_eq!(EspNowRxFrame::new([7; 6], &[]).unwrap().payload(), b"");
        assert_eq!(
            EspNowRxFrame::new([7; 6], &[0; super::super::ESP_NOW_MAX_PAYLOAD + 1]),
            Err(DriverError::PayloadTooLong)
        );
    }

    #[cfg(feature = "wifi-promiscuous")]
    #[test]
    fn promiscuous_queue_preserves_frame() {
        let resources = WifiResources::<4, 1, 0, 0, 1>::new();
        let info = PromiscuousFrameInfo {
            rssi: -42,
            channel: 6,
            packet_type: PromiscuousPacketType::Management,
            original_len: 6,
        };

        resources
            .enqueue_promiscuous_rx(PromiscuousRxFrame::new(info, b"beacon").unwrap())
            .unwrap();
        let received = resources.dequeue_promiscuous_rx().unwrap();
        assert_eq!(received.info(), info);
        assert_eq!(received.payload(), b"beacon");

        resources
            .enqueue_promiscuous_rx(PromiscuousRxFrame::new(info, b"again").unwrap())
            .unwrap();
        resources.reset_promiscuous_runtime();
        assert!(resources.dequeue_promiscuous_rx().is_none());

        let exact = PromiscuousRxFrame::new(
            PromiscuousFrameInfo {
                original_len: 112,
                ..info
            },
            &[0; 112],
        )
        .unwrap();
        assert_eq!(exact.payload().len(), 112);
        assert!(!exact.is_truncated());

        let truncated = PromiscuousRxFrame::new(
            PromiscuousFrameInfo {
                original_len: 200,
                ..info
            },
            &[0; 112],
        )
        .unwrap();
        assert_eq!(truncated.original_len(), 200);
        assert!(truncated.is_truncated());
        assert_eq!(
            PromiscuousRxFrame::new(
                PromiscuousFrameInfo {
                    original_len: 113,
                    ..info
                },
                &[0; 113],
            ),
            Err(DriverError::PayloadTooLong)
        );
    }

    #[cfg(feature = "wifi-smartconfig")]
    #[test]
    fn smartconfig_credentials_flow_through_the_private_event_queue() {
        let resources = WifiResources::<4, 1>::new();
        let credentials = SmartConfigCredentials::new(
            b"network",
            b"password123",
            Some([1, 2, 3, 4, 5, 6]),
            SmartConfigType::EspTouch,
            7,
            Ipv4Addr::new(192, 168, 0, 2),
        )
        .unwrap();
        let mut cursor = resources.claim_event_cursor().unwrap();

        resources
            .publish_smartconfig_credentials(credentials)
            .unwrap();
        assert!(matches!(
            resources.next_smartconfig_event(&mut cursor),
            Some(SmartConfigEvent::GotCredentials(actual)) if actual == credentials
        ));
        resources.release_event_cursor(&mut cursor);
    }

    #[test]
    fn copied_ap_rx_is_bounded_and_released_after_receive() {
        let resources = WifiResources::<4, 1, 0, 0, 0, 1, 8>::new();

        resources.enqueue_ap_rx_copy(b"first").unwrap();
        assert_eq!(resources.queued_ap_rx(), 1);
        assert_eq!(
            resources.enqueue_ap_rx_copy(b"second"),
            Err(DriverError::PacketOverflow)
        );

        let mut frame = [0; 8];
        assert_eq!(
            resources.dequeue_ap_rx().unwrap().copy_into(&mut frame),
            Ok(5)
        );
        assert_eq!(&frame[..5], b"first");

        resources.enqueue_ap_rx_copy(b"again").unwrap();
    }

    #[test]
    fn copied_ap_rx_rejects_oversize_frames() {
        let resources = WifiResources::<4, 1, 0, 0, 0, 1, 4>::new();

        assert_eq!(
            resources.enqueue_ap_rx_copy(b"oversize"),
            Err(DriverError::PayloadTooLong)
        );
        assert_eq!(resources.queued_ap_rx(), 0);
    }

    #[test]
    fn copied_ap_rx_uses_its_own_slot_capacity() {
        let resources = WifiResources::<4, 1, 0, 0, 0, 2, 8>::new();

        resources.enqueue_ap_rx_copy(b"first").unwrap();
        resources.enqueue_ap_rx_copy(b"second").unwrap();
        assert_eq!(resources.queued_ap_rx(), 2);
    }
}
