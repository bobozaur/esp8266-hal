use core::task::Waker;

use super::event::{EventBroker, EventCursor, LinkState, WifiEvent};
use super::feature::{
    BoundedQueue, EspNowPeer, EspNowPeerTable, EspNowRxFrame, EspNowTxStatus, PromiscuousRxFrame,
};
use super::locked::Locked;
use super::packet::{release_owned_rx_slot, OwnedRxPool, RawRxRecord, RxPushError, RxRing};
use super::waker::CallbackWaker;
use super::{Deadline, DriverError, WifiEventKind};

pub struct WifiResources<
    const EVENTS: usize,
    const RX: usize,
    const SUBS: usize,
    const ESP_NOW_PEERS: usize = 0,
    const ESP_NOW_RX: usize = 0,
    const PROMISCUOUS_RX: usize = 0,
    const AP_RX_SLOTS: usize = 0,
    const AP_RX_BYTES: usize = 0,
> {
    pub(crate) events: Locked<EventBroker<EVENTS, SUBS>>,
    pub(crate) sta_rx: Locked<RxRing<RX>>,
    pub(crate) ap_rx: Locked<RxRing<RX>>,
    pub(crate) ap_rx_owned: Locked<OwnedRxPool<AP_RX_SLOTS, AP_RX_BYTES>>,
    esp_now_peers: Locked<EspNowPeerTable<ESP_NOW_PEERS>>,
    esp_now_rx: Locked<BoundedQueue<EspNowRxFrame, ESP_NOW_RX>>,
    esp_now_tx: Locked<BoundedQueue<EspNowTxStatus, ESP_NOW_RX>>,
    promiscuous_rx: Locked<BoundedQueue<PromiscuousRxFrame, PROMISCUOUS_RX>>,
    link_waker: CallbackWaker,
    sta_rx_waker: CallbackWaker,
    ap_rx_waker: CallbackWaker,
}

impl<
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    >
    WifiResources<
        EVENTS,
        RX,
        SUBS,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
{
    pub const fn new() -> Self {
        Self {
            events: Locked::new(EventBroker::new()),
            sta_rx: Locked::new(RxRing::new()),
            ap_rx: Locked::new(RxRing::new()),
            ap_rx_owned: Locked::new(OwnedRxPool::new()),
            esp_now_peers: Locked::new(EspNowPeerTable::new()),
            esp_now_rx: Locked::new(BoundedQueue::new()),
            esp_now_tx: Locked::new(BoundedQueue::new()),
            promiscuous_rx: Locked::new(BoundedQueue::new()),
            link_waker: CallbackWaker::new(),
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

    pub const fn subscriber_capacity(&self) -> usize {
        SUBS
    }

    pub const fn esp_now_peer_capacity(&self) -> usize {
        ESP_NOW_PEERS
    }

    pub const fn esp_now_rx_capacity(&self) -> usize {
        ESP_NOW_RX
    }

    pub const fn promiscuous_rx_capacity(&self) -> usize {
        PROMISCUOUS_RX
    }

    pub const fn ap_rx_copy_capacity(&self) -> usize {
        AP_RX_SLOTS
    }

    pub const fn ap_rx_copy_frame_bytes(&self) -> usize {
        AP_RX_BYTES
    }

    pub fn queued_rx(&self) -> usize {
        self.sta_rx.with_ref(RxRing::len)
    }

    pub fn rx_drops(&self) -> u32 {
        self.sta_rx.with_ref(RxRing::drops)
    }

    pub fn queued_ap_rx(&self) -> usize {
        self.ap_rx.with_ref(RxRing::len)
    }

    pub fn ap_rx_drops(&self) -> u32 {
        self.ap_rx.with_ref(RxRing::drops)
    }

    pub fn claim_event_cursor(&self) -> Result<EventCursor, DriverError> {
        self.events.with(EventBroker::claim_cursor)
    }

    pub fn release_event_cursor(&self, cursor: &mut EventCursor) {
        self.events.with(|events| events.release_cursor(cursor));
    }

    pub fn publish_event(&self, event: WifiEvent) -> Result<(), DriverError> {
        let result = self.events.with(|events| events.publish(event));
        self.link_waker.wake();
        result
    }

    pub fn register_link_waker(&self, waker: &Waker) {
        self.link_waker.register(waker);
    }

    pub fn link_state(&self) -> LinkState {
        self.events.with_ref(EventBroker::link_state)
    }

    pub fn ap_link_state(&self) -> LinkState {
        self.events.with_ref(EventBroker::ap_link_state)
    }

    pub fn last_disconnect_reason(&self) -> Option<u8> {
        self.events.with_ref(EventBroker::last_disconnect_reason)
    }

    pub fn event_overflows(&self) -> u32 {
        self.events.with_ref(EventBroker::overflow_count)
    }

    pub fn event_discards(&self) -> u32 {
        self.events.with_ref(EventBroker::discard_count)
    }

    pub fn enqueue_rx(&self, record: RawRxRecord) -> Result<(), DriverError> {
        match self.sta_rx.with(|rx| rx.try_push(record)) {
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

    pub fn enqueue_ap_rx(&self, record: RawRxRecord) -> Result<(), DriverError> {
        match self.ap_rx.with(|rx| rx.try_push(record)) {
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

    pub fn enqueue_ap_rx_copy(
        &self,
        buffer: *mut core::ffi::c_void,
        len: u16,
    ) -> Result<(), DriverError> {
        let lease = self
            .ap_rx_owned
            .with(|owned| owned.try_acquire_copy(buffer as *const u8, len))?;

        let record = unsafe {
            RawRxRecord::from_raw(
                lease.buffer,
                lease.len,
                lease.slot,
                Some(release_owned_rx_slot::<AP_RX_BYTES>),
            )
        };

        match self.ap_rx.with(|rx| rx.try_push(record)) {
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

    pub fn register_rx_waker(&self, waker: &Waker) {
        self.sta_rx_waker.register(waker);
    }

    pub fn register_ap_rx_waker(&self, waker: &Waker) {
        self.ap_rx_waker.register(waker);
    }

    pub fn dequeue_rx(&self) -> Option<RawRxRecord> {
        self.sta_rx.with(RxRing::pop)
    }

    pub fn dequeue_ap_rx(&self) -> Option<RawRxRecord> {
        self.ap_rx.with(RxRing::pop)
    }

    pub fn register_esp_now_peer(&self, peer: EspNowPeer) -> Result<bool, DriverError> {
        self.esp_now_peers.with(|peers| peers.insert(peer))
    }

    pub fn remove_esp_now_peer(&self, peer_addr: &[u8; 6]) -> bool {
        self.esp_now_peers.with(|peers| peers.remove(peer_addr))
    }

    pub fn contains_esp_now_peer(&self, peer_addr: &[u8; 6]) -> bool {
        self.esp_now_peers
            .with_ref(|peers| peers.contains(peer_addr))
    }

    pub fn esp_now_peer_count(&self) -> usize {
        self.esp_now_peers.with_ref(EspNowPeerTable::len)
    }

    pub fn enqueue_esp_now_rx(&self, frame: EspNowRxFrame) -> Result<(), DriverError> {
        self.esp_now_rx.with(|queue| queue.try_push(frame))
    }

    pub fn dequeue_esp_now_rx(&self) -> Option<EspNowRxFrame> {
        self.esp_now_rx.with(BoundedQueue::pop)
    }

    pub fn queued_esp_now_rx(&self) -> usize {
        self.esp_now_rx.with_ref(BoundedQueue::len)
    }

    pub fn esp_now_rx_drops(&self) -> u32 {
        self.esp_now_rx.with_ref(BoundedQueue::drops)
    }

    pub fn enqueue_esp_now_tx_status(&self, status: EspNowTxStatus) -> Result<(), DriverError> {
        self.esp_now_tx.with(|queue| queue.try_push(status))
    }

    pub fn dequeue_esp_now_tx_status(&self) -> Option<EspNowTxStatus> {
        self.esp_now_tx.with(BoundedQueue::pop)
    }

    pub fn queued_esp_now_tx_status(&self) -> usize {
        self.esp_now_tx.with_ref(BoundedQueue::len)
    }

    pub fn esp_now_tx_status_drops(&self) -> u32 {
        self.esp_now_tx.with_ref(BoundedQueue::drops)
    }

    pub fn enqueue_promiscuous_rx(&self, frame: PromiscuousRxFrame) -> Result<(), DriverError> {
        self.promiscuous_rx.with(|queue| queue.try_push(frame))
    }

    pub fn dequeue_promiscuous_rx(&self) -> Option<PromiscuousRxFrame> {
        self.promiscuous_rx.with(BoundedQueue::pop)
    }

    pub fn queued_promiscuous_rx(&self) -> usize {
        self.promiscuous_rx.with_ref(BoundedQueue::len)
    }

    pub fn promiscuous_rx_drops(&self) -> u32 {
        self.promiscuous_rx.with_ref(BoundedQueue::drops)
    }

    pub fn wait_for_event_blocking<B: super::WifiRuntimeBackend>(
        &self,
        cursor: &mut EventCursor,
        backend: &mut B,
        wanted: WifiEvent,
        timeout_ms: u32,
    ) -> Result<WifiEvent, DriverError> {
        let deadline = Deadline::new(backend.now_ms(), timeout_ms);

        loop {
            if let Some(event) = self.events.with(|events| {
                while let Some(event) = events.try_next(cursor) {
                    if event == wanted {
                        return Some(event);
                    }
                }
                None
            }) {
                return Ok(event);
            }

            if deadline.is_expired(backend.now_ms()) {
                return Err(DriverError::Timeout);
            }

            backend.spin_once();
        }
    }

    pub fn wait_for_event_kind_blocking<B: super::WifiRuntimeBackend>(
        &self,
        cursor: &mut EventCursor,
        backend: &mut B,
        wanted: WifiEventKind,
        timeout_ms: u32,
    ) -> Result<WifiEvent, DriverError> {
        let deadline = Deadline::new(backend.now_ms(), timeout_ms);

        loop {
            if let Some(event) = self.events.with(|events| {
                while let Some(event) = events.try_next(cursor) {
                    if event.kind() == wanted {
                        return Some(event);
                    }
                }
                None
            }) {
                return Ok(event);
            }

            if deadline.is_expired(backend.now_ms()) {
                return Err(DriverError::Timeout);
            }

            backend.spin_once();
        }
    }

    pub async fn wait_for_event_async(
        &self,
        cursor: &mut EventCursor,
        wanted: WifiEventKind,
    ) -> WifiEvent {
        core::future::poll_fn(|cx| {
            self.register_link_waker(cx.waker());

            if let Some(event) = self.events.with(|events| {
                while let Some(event) = events.try_next(cursor) {
                    if event.kind() == wanted {
                        return Some(event);
                    }
                }
                None
            }) {
                return core::task::Poll::Ready(event);
            }

            core::task::Poll::Pending
        })
        .await
    }
}

fn release_rejected_rx(rejected: RxPushError) -> DriverError {
    let error = rejected.error();
    rejected.release();
    error
}
