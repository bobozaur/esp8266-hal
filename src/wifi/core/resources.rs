#[cfg(feature = "wifi-esp-now")]
use super::feature::{EspNowPeer, EspNowPeerTable};
#[cfg(any(
    feature = "wifi-esp-now",
    feature = "wifi-promiscuous",
    feature = "wifi-wps",
    feature = "wifi-smartconfig"
))]
use super::queue::Queue;
#[cfg(feature = "wifi-promiscuous")]
use super::PromiscuousRxFrame;
use super::{
    allocator::RuntimeAllocator,
    event::WifiEvent,
    locked::Locked,
    packet::{release_owned_rx_slot, OwnedRxPool, RawRxRecord, RxRing},
    waker::CallbackWaker,
    DriverError, InitError, LinkState, QueueConfig,
};
#[cfg(feature = "wifi-esp-now")]
use super::{EspNowRxFrame, EspNowSendStatus};
#[cfg(feature = "wifi-smartconfig")]
use super::{SmartConfigCredentials, SmartConfigEvent};
use core::task::Waker;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InterfaceId {
    Station,
    AccessPoint,
}
impl InterfaceId {
    pub(crate) fn index(self) -> usize {
        self as usize
    }
}

pub(crate) struct ControlState {
    pub(crate) sta_started: bool,
    pub(crate) stopping: bool,
    pub(crate) scan_pending: bool,
    pub(crate) scan_done: Option<Result<(), DriverError>>,
    pub(crate) ap_started: bool,
    pub(crate) connected: bool,
    pub(crate) station_configured: bool,
    pub(crate) connecting: bool,
    pub(crate) disconnecting: bool,
    pub(crate) connect_result: Option<Result<(), DriverError>>,
    pub(crate) reason: Option<u8>,
    admission: [bool; 2],
    generation: [u32; 2],
    sdk_leases: usize,
    submissions: usize,
}
impl ControlState {
    fn new() -> Self {
        Self {
            stopping: false,
            scan_pending: false,
            scan_done: None,
            sta_started: false,
            ap_started: false,
            connected: false,
            station_configured: false,
            connecting: false,
            disconnecting: false,
            connect_result: None,
            reason: None,
            admission: [false; 2],
            generation: [0; 2],
            sdk_leases: 0,
            submissions: 0,
        }
    }
}

#[cfg(feature = "wifi-esp-now")]
#[derive(Clone, Copy)]
enum EspNowTxSlot {
    Idle,
    Pending,
    Complete(EspNowSendStatus),
}

pub(crate) struct Resources {
    pub(crate) state: Locked<ControlState>,
    #[cfg(target_arch = "xtensa")]
    pub(super) scan_storage: Locked<super::target_map::ScanStorage>,
    #[cfg(all(target_arch = "xtensa", feature = "wifi-smartconfig"))]
    pub(super) smartconfig_storage: Locked<super::target_map::SmartConfigStorage>,
    sta_rx: Locked<RxRing>,
    ap_rx: Locked<RxRing>,
    // Shared pool access never creates an exclusive borrow of leased bytes.
    ap_owned: OwnedRxPool,
    pub(crate) frame_bytes: usize,
    pub(crate) station_rx_capacity: usize,
    pub(crate) ap_rx_capacity: usize,
    control_waker: CallbackWaker,
    link_wakers: [CallbackWaker; 2],
    rx_wakers: [CallbackWaker; 2],
    #[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps"))]
    events: Locked<ProvisioningEvents>,
    #[cfg(feature = "wifi-esp-now")]
    peers: Locked<EspNowPeerTable>,
    #[cfg(feature = "wifi-esp-now")]
    esp_now_rx: Locked<Queue<EspNowRxFrame>>,
    #[cfg(feature = "wifi-esp-now")]
    esp_now_tx: Locked<EspNowTxSlot>,
    #[cfg(feature = "wifi-promiscuous")]
    promiscuous_rx: Locked<Queue<PromiscuousRxFrame>>,
    #[cfg(feature = "wifi-smartconfig")]
    credentials: Locked<Option<SmartConfigCredentials>>,
}

#[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps"))]
struct ProvisioningEvents {
    queue: Queue<WifiEvent>,
    claimed: bool,
    overflow: bool,
}

impl Resources {
    pub(crate) fn try_new(
        config: &QueueConfig,
        allocator: RuntimeAllocator,
    ) -> Result<Self, InitError> {
        if !(super::MIN_IPV4_ETHERNET_FRAME_SIZE..=super::MAX_ETHERNET_FRAME_SIZE)
            .contains(&config.frame_bytes)
        {
            return Err(InitError::InvalidConfiguration);
        }
        Ok(Self {
            state: Locked::new(ControlState::new()),
            #[cfg(target_arch = "xtensa")]
            scan_storage: Locked::new(super::target_map::ScanStorage::new()),
            #[cfg(all(target_arch = "xtensa", feature = "wifi-smartconfig"))]
            smartconfig_storage: Locked::new(super::target_map::SmartConfigStorage::new()),
            sta_rx: Locked::new(RxRing::try_new(config.station_rx, allocator)?),
            ap_rx: Locked::new(RxRing::try_new(config.access_point_rx, allocator)?),
            ap_owned: OwnedRxPool::try_new(config.access_point_rx, config.frame_bytes, allocator)?,
            frame_bytes: config.frame_bytes,
            station_rx_capacity: config.station_rx,
            ap_rx_capacity: config.access_point_rx,
            control_waker: CallbackWaker::new(),
            link_wakers: [CallbackWaker::new(), CallbackWaker::new()],
            rx_wakers: [CallbackWaker::new(), CallbackWaker::new()],
            #[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps"))]
            events: Locked::new(ProvisioningEvents {
                queue: Queue::try_new(config.events, allocator)?,
                claimed: false,
                overflow: false,
            }),
            #[cfg(feature = "wifi-esp-now")]
            peers: Locked::new(EspNowPeerTable::try_new(config.esp_now_peers, allocator)?),
            #[cfg(feature = "wifi-esp-now")]
            esp_now_rx: Locked::new(Queue::try_new(config.esp_now_rx, allocator)?),
            #[cfg(feature = "wifi-esp-now")]
            esp_now_tx: Locked::new(EspNowTxSlot::Idle),
            #[cfg(feature = "wifi-promiscuous")]
            promiscuous_rx: Locked::new(Queue::try_new(config.promiscuous_rx, allocator)?),
            #[cfg(feature = "wifi-smartconfig")]
            credentials: Locked::new(None),
        })
    }
    pub(crate) fn publish_event(&self, event: WifiEvent) -> Result<(), DriverError> {
        self.state.with(|state| match event {
            WifiEvent::ScanDone { status } => {
                if state.scan_pending {
                    state.scan_done = Some(if status == 0 {
                        Ok(())
                    } else {
                        Err(DriverError::Sdk(status as i32))
                    });
                }
            }
            WifiEvent::StaStarted => state.sta_started = true,
            WifiEvent::StaStopped => {
                state.sta_started = false;
                state.connected = false;
                state.connecting = false;
                state.disconnecting = false;
                state.admission[0] = false;
            }
            WifiEvent::StaConnected => {
                state.connected = true;
                state.connecting = false;
                state.connect_result = Some(Ok(()));
                if !state.disconnecting && !state.stopping {
                    state.admission[0] = true;
                }
            }
            WifiEvent::StaDisconnected { reason } => {
                state.connected = false;
                state.connecting = false;
                state.disconnecting = false;
                state.reason = reason;
                state.connect_result = Some(Err(DriverError::Disconnected { reason }));
                state.admission[0] = false;
                state.generation[0] = state.generation[0].wrapping_add(1);
            }
            WifiEvent::ApStarted => {
                state.ap_started = true;
                if !state.stopping {
                    state.admission[1] = true;
                }
            }
            WifiEvent::ApStopped => {
                state.ap_started = false;
                state.admission[1] = false;
                state.generation[1] = state.generation[1].wrapping_add(1);
            }
            #[cfg(feature = "wifi-wps")]
            WifiEvent::WpsSucceeded => state.station_configured = true,
            #[cfg(feature = "wifi-wps")]
            WifiEvent::WpsFailed { .. } | WifiEvent::WpsTimedOut => {
                state.station_configured = false
            }
            #[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps"))]
            _ => {}
        });
        #[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps"))]
        let result = self.events.with(|events| {
            if events.claimed && events.queue.try_push(event).is_err() {
                events.overflow = true;
                Err(DriverError::EventOverflow)
            } else {
                Ok(())
            }
        });
        #[cfg(not(any(feature = "wifi-smartconfig", feature = "wifi-wps")))]
        let result = Ok(());
        self.control_waker.wake();
        match event {
            WifiEvent::StaStarted
            | WifiEvent::StaStopped
            | WifiEvent::StaConnected
            | WifiEvent::StaDisconnected { .. } => self.link_wakers[0].wake(),
            WifiEvent::ApStarted | WifiEvent::ApStopped => self.link_wakers[1].wake(),
            _ => {}
        }
        result
    }
    pub(crate) fn link_state(&self) -> LinkState {
        self.interface_link_state(InterfaceId::Station)
    }
    pub(crate) fn interface_link_state(&self, id: InterfaceId) -> LinkState {
        self.state.with(|s| {
            if match id {
                InterfaceId::Station => s.connected,
                InterfaceId::AccessPoint => s.ap_started,
            } {
                LinkState::Up
            } else {
                LinkState::Down
            }
        })
    }
    pub(crate) fn last_disconnect_reason(&self) -> Option<u8> {
        self.state.with(|s| s.reason)
    }
    pub(crate) fn register_control_waker(&self, w: &Waker) {
        self.control_waker.register(w)
    }
    pub(crate) fn register_link_waker(&self, id: InterfaceId, w: &Waker) {
        self.link_wakers[id.index()].register(w)
    }
    pub(crate) fn register_rx_waker(&self, id: InterfaceId, w: &Waker) {
        self.rx_wakers[id.index()].register(w)
    }
    pub(crate) fn generation(&self, id: InterfaceId) -> u32 {
        self.state.with(|s| s.generation[id.index()])
    }
    pub(crate) fn begin_submission(
        &self,
        id: InterfaceId,
        generation: u32,
    ) -> Result<Submission<'_>, DriverError> {
        self.state.with(|s| {
            if !s.admission[id.index()] || s.generation[id.index()] != generation {
                return Err(DriverError::NotStarted);
            }
            s.submissions += 1;
            Ok(Submission(self))
        })
    }
    pub(crate) fn close_admission(&self) -> Result<(), DriverError> {
        self.state.with(|s| {
            if s.sdk_leases != 0 || s.submissions != 0 {
                return Err(DriverError::Busy);
            }
            s.stopping = true;
            s.admission = [false; 2];
            for generation in &mut s.generation {
                *generation = generation.wrapping_add(1)
            }
            Ok(())
        })
    }
    pub(crate) fn close_station(&self) -> Result<(), DriverError> {
        self.state.with(|s| {
            if s.sdk_leases != 0 || s.submissions != 0 {
                return Err(DriverError::Busy);
            }
            s.disconnecting = true;
            s.admission[0] = false;
            s.generation[0] = s.generation[0].wrapping_add(1);
            Ok(())
        })
    }
    pub(crate) fn has_rx(&self, id: InterfaceId) -> bool {
        critical_section::with(|_| {
            self.state.with(|state| state.admission[id.index()])
                && self.ring(id).with(|queue| queue.len() != 0)
        })
    }
    fn ring(&self, id: InterfaceId) -> &Locked<RxRing> {
        match id {
            InterfaceId::Station => &self.sta_rx,
            InterfaceId::AccessPoint => &self.ap_rx,
        }
    }
    pub(crate) fn take_rx(&self, id: InterfaceId) -> Option<RxPacket<'_>> {
        critical_section::with(|_| {
            if !self.state.with(|s| s.admission[id.index()]) {
                return None;
            }
            let record = self.ring(id).with(RxRing::pop)?;
            let sdk = id == InterfaceId::Station;
            if sdk {
                self.state.with(|s| s.sdk_leases += 1)
            }
            Some(RxPacket {
                record: Some(record),
                resources: self,
                sdk,
            })
        })
    }
    pub(crate) fn enqueue_rx(&self, record: RawRxRecord) -> Result<(), DriverError> {
        let result = critical_section::with(|_| {
            if !self.state.with(|s| s.admission[0]) || record.len() > self.frame_bytes {
                return Err(record);
            }
            self.sta_rx.with(|queue| queue.try_push(record))
        });
        match result {
            Ok(()) => {
                self.rx_wakers[0].wake();
                Ok(())
            }
            Err(record) => {
                drop(record);
                Err(DriverError::PacketOverflow)
            }
        }
    }
    pub(crate) fn enqueue_ap_rx_copy(&self, frame: &[u8]) -> Result<(), DriverError> {
        let result = critical_section::with(|_| {
            if !self.state.with(|s| s.admission[1]) {
                return Err(DriverError::NotStarted);
            }
            let lease = self.ap_owned.try_acquire_copy(frame)?;
            let record = unsafe {
                RawRxRecord::from_raw(
                    lease.buffer,
                    lease.len,
                    lease.slot,
                    Some(release_owned_rx_slot),
                )
            };
            self.ap_rx.with(|q| {
                q.try_push(record).map_err(|rejected| {
                    drop(rejected);
                    DriverError::PacketOverflow
                })
            })
        });
        if result.is_ok() {
            self.rx_wakers[1].wake()
        }
        result
    }
    pub(crate) fn drain_sta_rx(&self) {
        while let Some(record) = self.sta_rx.with(RxRing::pop) {
            drop(record)
        }
    }
    pub(crate) fn drain_ap_rx(&self) {
        while let Some(record) = self.ap_rx.with(RxRing::pop) {
            drop(record)
        }
    }
    pub(crate) fn reset_runtime(&self) {
        self.drain_sta_rx();
        self.drain_ap_rx();
        // Checked-out AP bytes remain valid. Their leases return slots later.
        self.state.with(|s| {
            s.connected = false;
            s.connecting = false;
            s.disconnecting = false;
            s.sta_started = false;
            s.ap_started = false;
            s.admission = [false; 2];
            s.connect_result = None;
            s.scan_pending = false;
            s.scan_done = None;
        });
        #[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps"))]
        self.events.with(|e| {
            e.queue.clear();
            e.overflow = false;
        });
        #[cfg(feature = "wifi-esp-now")]
        self.reset_esp_now_runtime();
        #[cfg(feature = "wifi-promiscuous")]
        self.promiscuous_rx.with(Queue::clear);
        #[cfg(feature = "wifi-smartconfig")]
        self.credentials.with(|c| *c = None);
        self.control_waker.wake();
        for w in &self.link_wakers {
            w.wake()
        }
        for w in &self.rx_wakers {
            w.wake()
        }
    }
    #[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps"))]
    pub(crate) fn claim_events(&self) -> Result<(), DriverError> {
        self.events.with(|e| {
            if e.claimed || e.queue.capacity() == 0 {
                return Err(DriverError::Busy);
            }
            e.claimed = true;
            e.queue.clear();
            e.overflow = false;
            Ok(())
        })
    }
    #[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps"))]
    pub(crate) fn clear_events(&self) {
        self.events.with(|events| {
            events.queue.clear();
            events.overflow = false;
        })
    }
    #[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps"))]
    pub(crate) fn release_events(&self) {
        self.events.with(|e| {
            e.claimed = false;
            e.queue.clear();
            e.overflow = false;
        })
    }
    #[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps"))]
    pub(crate) fn next_event(&self) -> Result<Option<WifiEvent>, DriverError> {
        self.events.with(|e| {
            if e.overflow {
                e.overflow = false;
                return Err(DriverError::EventOverflow);
            }
            Ok(e.queue.pop())
        })
    }
    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn esp_now_rx_capacity(&self) -> usize {
        self.esp_now_rx.with(|q| q.capacity())
    }
    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn register_esp_now_peer(&self, p: EspNowPeer) -> Result<bool, DriverError> {
        self.peers.with(|t| t.insert(p))
    }
    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn remove_esp_now_peer(&self, p: &[u8; 6]) -> bool {
        self.peers.with(|t| t.remove(p))
    }
    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn contains_esp_now_peer(&self, p: &[u8; 6]) -> bool {
        self.peers.with(|t| t.contains(p))
    }
    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn esp_now_peer_count(&self) -> usize {
        self.peers.with(|t| t.len())
    }
    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn esp_now_peer_capacity(&self) -> usize {
        self.peers.with(|t| t.capacity())
    }
    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn enqueue_esp_now_rx(&self, f: EspNowRxFrame) -> Result<(), DriverError> {
        self.esp_now_rx
            .with(|q| q.try_push(f).map_err(|_| DriverError::PacketOverflow))
    }
    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn dequeue_esp_now_rx(&self) -> Option<EspNowRxFrame> {
        self.esp_now_rx.with(Queue::pop)
    }
    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn reserve_esp_now_tx(&self) -> Result<(), DriverError> {
        self.esp_now_tx.with(|s| match s {
            EspNowTxSlot::Idle => {
                *s = EspNowTxSlot::Pending;
                Ok(())
            }
            _ => Err(DriverError::WouldBlock),
        })
    }
    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn cancel_esp_now_tx(&self) {
        self.esp_now_tx.with(|s| {
            if matches!(s, EspNowTxSlot::Pending) {
                *s = EspNowTxSlot::Idle
            }
        })
    }
    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn complete_esp_now_tx(&self, status: EspNowSendStatus) -> Result<(), DriverError> {
        self.esp_now_tx.with(|s| {
            if matches!(s, EspNowTxSlot::Pending) {
                *s = EspNowTxSlot::Complete(status);
                Ok(())
            } else {
                Err(DriverError::InvalidState)
            }
        })
    }
    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn take_esp_now_tx_status(&self) -> Option<EspNowSendStatus> {
        self.esp_now_tx.with(|s| {
            if let EspNowTxSlot::Complete(status) = *s {
                *s = EspNowTxSlot::Idle;
                Some(status)
            } else {
                None
            }
        })
    }
    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn reset_esp_now_runtime(&self) {
        self.peers.with(EspNowPeerTable::clear);
        self.esp_now_rx.with(Queue::clear);
        self.esp_now_tx.with(|s| *s = EspNowTxSlot::Idle)
    }
    #[cfg(feature = "wifi-promiscuous")]
    pub(crate) fn promiscuous_capacity(&self) -> usize {
        self.promiscuous_rx.with(|q| q.capacity())
    }
    #[cfg(feature = "wifi-promiscuous")]
    pub(crate) fn enqueue_promiscuous_rx(&self, f: PromiscuousRxFrame) -> Result<(), DriverError> {
        self.promiscuous_rx
            .with(|q| q.try_push(f).map_err(|_| DriverError::PacketOverflow))
    }
    #[cfg(feature = "wifi-promiscuous")]
    pub(crate) fn dequeue_promiscuous_rx(&self) -> Option<PromiscuousRxFrame> {
        self.promiscuous_rx.with(Queue::pop)
    }
    #[cfg(feature = "wifi-promiscuous")]
    pub(crate) fn reset_promiscuous_runtime(&self) {
        self.promiscuous_rx.with(Queue::clear)
    }
    #[cfg(feature = "wifi-smartconfig")]
    pub(crate) fn clear_smartconfig_credentials(&self) {
        self.credentials.with(|c| *c = None)
    }
    #[cfg(feature = "wifi-smartconfig")]
    pub(crate) fn publish_smartconfig_credentials(
        &self,
        c: SmartConfigCredentials,
    ) -> Result<(), DriverError> {
        if !self.credentials.with(|slot| {
            if slot.is_some() {
                false
            } else {
                *slot = Some(c);
                true
            }
        }) {
            self.events.with(|events| events.overflow = true);
            self.control_waker.wake();
            return Err(DriverError::EventOverflow);
        }
        self.publish_event(WifiEvent::SmartConfigGotCredentials)
    }
    #[cfg(feature = "wifi-smartconfig")]
    pub(crate) fn next_smartconfig_event(&self) -> Result<Option<SmartConfigEvent>, DriverError> {
        while let Some(event) = self.next_event()? {
            return Ok(Some(match event {
                WifiEvent::SmartConfigScanDone => SmartConfigEvent::ScanDone,
                WifiEvent::SmartConfigFoundChannel => SmartConfigEvent::FoundChannel,
                WifiEvent::SmartConfigSendAckDone => SmartConfigEvent::SendAckDone,
                WifiEvent::SmartConfigGotCredentials => match self.credentials.with(Option::take) {
                    Some(c) => SmartConfigEvent::GotCredentials(c),
                    None => continue,
                },
                _ => continue,
            }));
        }
        Ok(None)
    }
}

pub(crate) struct Submission<'a>(&'a Resources);
impl Drop for Submission<'_> {
    fn drop(&mut self) {
        self.0.state.with(|s| s.submissions -= 1)
    }
}

/// Packet lease keeps its owner alive and distinguishes checked-out SDK buffers.
pub(crate) struct RxPacket<'a> {
    record: Option<RawRxRecord>,
    resources: &'a Resources,
    sdk: bool,
}
impl RxPacket<'_> {
    pub(crate) fn as_slice(&self) -> &[u8] {
        self.record.as_ref().unwrap().as_slice()
    }
}
impl Drop for RxPacket<'_> {
    fn drop(&mut self) {
        drop(self.record.take());
        if self.sdk {
            self.resources.state.with(|s| s.sdk_leases -= 1)
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::super::allocator::test_allocator;
    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use std::{sync::Arc, task::Wake};
    static BYTES: [u8; 128] = [0xa5; 128];
    struct CountWake(AtomicUsize);
    impl Wake for CountWake {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    fn resources() -> Resources {
        Resources::try_new(
            &QueueConfig {
                station_rx: 1,
                access_point_rx: 1,
                ..QueueConfig::default()
            },
            test_allocator(),
        )
        .unwrap()
    }
    #[test]
    fn checked_out_sdk_record_blocks_stop_and_releases_once() {
        static RELEASES: AtomicUsize = AtomicUsize::new(0);
        unsafe fn release(_: *mut core::ffi::c_void, _: *mut core::ffi::c_void) {
            RELEASES.fetch_add(1, Ordering::SeqCst);
        }
        RELEASES.store(0, Ordering::SeqCst);
        let r = resources();
        r.publish_event(WifiEvent::StaConnected).unwrap();
        let record = || unsafe {
            RawRxRecord::from_raw(
                BYTES.as_ptr().cast_mut().cast(),
                128,
                core::ptr::null_mut(),
                Some(release),
            )
        };
        r.enqueue_rx(record()).unwrap();
        assert_eq!(r.enqueue_rx(record()), Err(DriverError::PacketOverflow));
        assert_eq!(RELEASES.load(Ordering::SeqCst), 1);
        let packet = r.take_rx(InterfaceId::Station).unwrap();
        assert_eq!(r.close_admission(), Err(DriverError::Busy));
        assert_eq!(packet.as_slice(), BYTES);
        drop(packet);
        assert_eq!(RELEASES.load(Ordering::SeqCst), 2);
        assert_eq!(r.close_admission(), Ok(()));
        assert_eq!(r.enqueue_rx(record()), Err(DriverError::PacketOverflow));
        assert_eq!(RELEASES.load(Ordering::SeqCst), 3);
    }
    #[test]
    fn ap_lease_survives_stop_reset_and_is_not_reused() {
        let r = resources();
        r.publish_event(WifiEvent::ApStarted).unwrap();
        r.enqueue_ap_rx_copy(b"first").unwrap();
        let packet = r.take_rx(InterfaceId::AccessPoint).unwrap();
        let bytes = packet.as_slice();
        r.close_admission().unwrap();
        r.reset_runtime();
        assert_eq!(bytes, b"first");
        r.state.with(|s| s.stopping = false);
        r.publish_event(WifiEvent::ApStarted).unwrap();
        assert_eq!(
            r.enqueue_ap_rx_copy(b"second"),
            Err(DriverError::PacketOverflow)
        );
        drop(packet);
        r.enqueue_ap_rx_copy(b"second").unwrap();
        assert_eq!(
            r.take_rx(InterfaceId::AccessPoint).unwrap().as_slice(),
            b"second"
        );
    }
    #[test]
    fn stop_generation_rejects_old_tx_and_late_events_cannot_reopen_admission() {
        let r = resources();
        r.publish_event(WifiEvent::StaConnected).unwrap();
        let old = r.generation(InterfaceId::Station);
        let submission = r.begin_submission(InterfaceId::Station, old).unwrap();
        assert_eq!(r.close_admission(), Err(DriverError::Busy));
        drop(submission);
        r.close_admission().unwrap();
        r.publish_event(WifiEvent::StaConnected).unwrap();
        assert!(matches!(
            r.begin_submission(InterfaceId::Station, old),
            Err(DriverError::NotStarted)
        ));
        r.state.with(|s| s.stopping = false);
        r.publish_event(WifiEvent::StaConnected).unwrap();
        assert!(r
            .begin_submission(InterfaceId::Station, r.generation(InterfaceId::Station))
            .is_ok());
    }
    #[test]
    fn controller_and_network_waiters_wake_independently() {
        let r = resources();
        let control = Arc::new(CountWake(AtomicUsize::new(0)));
        let network = Arc::new(CountWake(AtomicUsize::new(0)));
        r.register_control_waker(&control.clone().into());
        r.register_link_waker(InterfaceId::Station, &network.clone().into());
        r.publish_event(WifiEvent::StaConnected).unwrap();
        assert_eq!(control.0.load(Ordering::SeqCst), 1);
        assert_eq!(network.0.load(Ordering::SeqCst), 1);
    }
    #[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps"))]
    #[test]
    fn provisioning_overflow_is_visible_without_losing_connection_state() {
        let r = Resources::try_new(
            &QueueConfig {
                events: 1,
                ..QueueConfig::default()
            },
            test_allocator(),
        )
        .unwrap();
        r.claim_events().unwrap();
        r.publish_event(WifiEvent::StaStarted).unwrap();
        assert_eq!(
            r.publish_event(WifiEvent::StaDisconnected { reason: Some(202) }),
            Err(DriverError::EventOverflow)
        );
        assert_eq!(r.next_event(), Err(DriverError::EventOverflow));
        assert_eq!(r.last_disconnect_reason(), Some(202));
        assert_eq!(r.link_state(), LinkState::Down);
    }
}

#[cfg(all(
    test,
    any(
        feature = "wifi-esp-now",
        feature = "wifi-promiscuous",
        feature = "wifi-smartconfig"
    )
))]
mod feature_rx_tests {
    use super::super::allocator::test_allocator;
    use super::*;
    #[cfg(feature = "wifi-esp-now")]
    #[test]
    fn esp_now_rx_validates_payload_and_preserves_frame_at_saturation() {
        let r = Resources::try_new(
            &QueueConfig {
                esp_now_rx: 1,
                ..QueueConfig::default()
            },
            test_allocator(),
        )
        .unwrap();
        assert_eq!(
            EspNowRxFrame::new([1; 6], &[0; 251]),
            Err(DriverError::PayloadTooLong)
        );
        let frame = EspNowRxFrame::new([1; 6], &[0x5a; 250]).unwrap();
        r.enqueue_esp_now_rx(frame).unwrap();
        assert_eq!(
            r.enqueue_esp_now_rx(frame),
            Err(DriverError::PacketOverflow)
        );
        let received = r.dequeue_esp_now_rx().unwrap();
        assert_eq!(received.peer_addr(), &[1; 6]);
        assert_eq!(received.payload(), &[0x5a; 250]);
        assert!(r.dequeue_esp_now_rx().is_none());
    }
    #[cfg(feature = "wifi-promiscuous")]
    #[test]
    fn promiscuous_rx_preserves_metadata_and_validates_captured_prefix() {
        use super::super::{PromiscuousFrameInfo, PromiscuousPacketType};
        let r = Resources::try_new(
            &QueueConfig {
                promiscuous_rx: 1,
                ..QueueConfig::default()
            },
            test_allocator(),
        )
        .unwrap();
        let info = PromiscuousFrameInfo {
            rssi: -42,
            channel: 6,
            packet_type: PromiscuousPacketType::Data,
            original_len: 128,
        };
        assert_eq!(
            PromiscuousRxFrame::new(info, &[0; 113]),
            Err(DriverError::PayloadTooLong)
        );
        let frame = PromiscuousRxFrame::new(info, &[0x5a; 112]).unwrap();
        r.enqueue_promiscuous_rx(frame).unwrap();
        assert_eq!(
            r.enqueue_promiscuous_rx(frame),
            Err(DriverError::PacketOverflow)
        );
        let received = r.dequeue_promiscuous_rx().unwrap();
        assert_eq!(received.info(), info);
        assert_eq!(received.original_len(), 128);
        assert!(received.is_truncated());
        assert_eq!(received.payload(), &[0x5a; 112]);
    }
    #[cfg(feature = "wifi-smartconfig")]
    #[test]
    fn duplicate_smartconfig_credentials_report_overflow_without_overwriting_first() {
        use super::super::{Ipv4Addr, SmartConfigType};
        let r = Resources::try_new(&QueueConfig::default(), test_allocator()).unwrap();
        r.claim_events().unwrap();
        let credentials = SmartConfigCredentials::new(
            b"first",
            b"password123",
            None,
            SmartConfigType::EspTouch,
            7,
            Ipv4Addr::UNSPECIFIED,
        )
        .unwrap();
        r.publish_smartconfig_credentials(credentials).unwrap();
        assert_eq!(
            r.publish_smartconfig_credentials(credentials),
            Err(DriverError::EventOverflow)
        );
        assert!(matches!(
            r.next_smartconfig_event(),
            Err(DriverError::EventOverflow)
        ));
        match r.next_smartconfig_event() {
            Ok(Some(SmartConfigEvent::GotCredentials(first))) => {
                assert_eq!(first.ssid(), credentials.ssid());
                assert_eq!(first.token(), 7);
            }
            _ => panic!("first credentials remain queued"),
        };
    }
}
