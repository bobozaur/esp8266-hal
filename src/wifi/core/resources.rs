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
    packet::{RawRxRecord, RxRing},
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
    rx: [RxRing; 2],
    rx_leases: [usize; 2],
    submissions: usize,
}
impl ControlState {
    fn new(rx: [RxRing; 2]) -> Self {
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
            rx,
            rx_leases: [0; 2],
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
    pub(crate) frame_bytes: usize,
    pub(crate) station_rx_capacity: usize,
    pub(crate) ap_rx_capacity: usize,
    control_waker: CallbackWaker,
    link_wakers: [CallbackWaker; 2],
    rx_wakers: [CallbackWaker; 2],
    #[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps"))]
    events: Locked<ProvisioningEvents>,
    #[cfg(feature = "wifi-esp-now")]
    esp_now_peer_capacity: usize,
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
            state: Locked::new(ControlState::new([
                RxRing::try_new(config.station_rx, allocator)?,
                RxRing::try_new(config.access_point_rx, allocator)?,
            ])),
            #[cfg(target_arch = "xtensa")]
            scan_storage: Locked::new(super::target_map::ScanStorage::new()),
            #[cfg(all(target_arch = "xtensa", feature = "wifi-smartconfig"))]
            smartconfig_storage: Locked::new(super::target_map::SmartConfigStorage::new()),
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
            esp_now_peer_capacity: config.esp_now_peers,
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
                state.generation[0] = state.generation[0].wrapping_add(1);
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
        // SDK lifecycle events are serialized by its event-loop mutex. Close
        // admission first, release queued records without borrowing state, then
        // publish/wake. Checked-out leases and the other interface stay live.
        match event {
            WifiEvent::StaDisconnected { .. } | WifiEvent::StaStopped => self.drain_sta_rx(),
            WifiEvent::ApStopped => self.drain_ap_rx(),
            _ => {}
        }
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
            if s.rx_leases.iter().any(|&leases| leases != 0) || s.submissions != 0 {
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
            if s.rx_leases[InterfaceId::Station.index()] != 0 || s.submissions != 0 {
                return Err(DriverError::Busy);
            }
            s.disconnecting = true;
            s.admission[0] = false;
            s.generation[0] = s.generation[0].wrapping_add(1);
            Ok(())
        })
    }
    pub(crate) fn has_rx(&self, id: InterfaceId) -> bool {
        self.state
            .with(|s| s.admission[id.index()] && s.rx[id.index()].len() != 0)
    }
    pub(crate) fn take_rx(&self, id: InterfaceId) -> Option<RxPacket<'_>> {
        self.state.with(|s| {
            if !s.admission[id.index()] {
                return None;
            }
            let record = s.rx[id.index()].pop()?;
            s.rx_leases[id.index()] += 1;
            Some(RxPacket {
                record: Some(record),
                resources: self,
                id,
            })
        })
    }
    pub(crate) fn enqueue_rx(
        &self,
        id: InterfaceId,
        record: RawRxRecord,
    ) -> Result<(), DriverError> {
        let result = self.state.with(|s| {
            if !s.admission[id.index()] || record.len() > self.frame_bytes {
                return Err(record);
            }
            s.rx[id.index()].try_push(record)
        });
        match result {
            Ok(()) => {
                self.rx_wakers[id.index()].wake();
                Ok(())
            }
            Err(record) => {
                drop(record);
                Err(DriverError::PacketOverflow)
            }
        }
    }
    fn drain_rx(&self, id: InterfaceId) {
        while let Some(record) = self.state.with(|s| s.rx[id.index()].pop()) {
            drop(record);
        }
    }
    pub(crate) fn drain_sta_rx(&self) {
        self.drain_rx(InterfaceId::Station)
    }
    pub(crate) fn drain_ap_rx(&self) {
        self.drain_rx(InterfaceId::AccessPoint)
    }
    pub(crate) fn reset_runtime(&self) {
        self.drain_sta_rx();
        self.drain_ap_rx();
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
    pub(crate) fn esp_now_peer_capacity(&self) -> usize {
        self.esp_now_peer_capacity
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

/// Checked-out native frame whose interface must remain alive until release.
pub(crate) struct RxPacket<'a> {
    record: Option<RawRxRecord>,
    resources: &'a Resources,
    id: InterfaceId,
}
impl RxPacket<'_> {
    pub(crate) fn as_slice(&self) -> &[u8] {
        self.record.as_ref().unwrap().as_slice()
    }
}
impl Drop for RxPacket<'_> {
    fn drop(&mut self) {
        drop(self.record.take());
        self.resources
            .state
            .with(|s| s.rx_leases[self.id.index()] -= 1)
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
    fn record(releases: &Arc<AtomicUsize>, len: u16) -> RawRxRecord {
        unsafe fn release(_: *mut core::ffi::c_void, counter: *mut core::ffi::c_void) {
            // SAFETY: the record owns the Arc transferred by into_raw below.
            let counter = unsafe { Arc::from_raw(counter.cast::<AtomicUsize>()) };
            counter.fetch_add(1, Ordering::SeqCst);
        }
        assert!(usize::from(len) <= BYTES.len());
        // SAFETY: immutable static bytes and an owned Arc keep both pointers
        // alive. The record owns the only invocation of this release hook.
        unsafe {
            RawRxRecord::from_raw(
                BYTES.as_ptr().cast_mut().cast(),
                len,
                Arc::into_raw(releases.clone()).cast_mut().cast(),
                release,
            )
        }
    }

    #[test]
    fn both_rx_interfaces_release_once_on_rejection_drain_and_token_drop() {
        for id in [InterfaceId::Station, InterfaceId::AccessPoint] {
            let releases = Arc::new(AtomicUsize::new(0));
            let r = Resources::try_new(
                &QueueConfig {
                    station_rx: 1,
                    access_point_rx: 1,
                    frame_bytes: 82,
                    ..QueueConfig::default()
                },
                test_allocator(),
            )
            .unwrap();
            // Closed admission releases immediately.
            assert_eq!(
                r.enqueue_rx(id, record(&releases, 64)),
                Err(DriverError::PacketOverflow)
            );
            assert_eq!(releases.load(Ordering::SeqCst), 1);
            r.publish_event(match id {
                InterfaceId::Station => WifiEvent::StaConnected,
                InterfaceId::AccessPoint => WifiEvent::ApStarted,
            })
            .unwrap();
            // Oversize and full queue also return their release responsibility.
            assert_eq!(
                r.enqueue_rx(id, record(&releases, 128)),
                Err(DriverError::PacketOverflow)
            );
            r.enqueue_rx(id, record(&releases, 64)).unwrap();
            assert_eq!(
                r.enqueue_rx(id, record(&releases, 64)),
                Err(DriverError::PacketOverflow)
            );
            assert_eq!(releases.load(Ordering::SeqCst), 3);
            let packet = r.take_rx(id).unwrap();
            assert_eq!(packet.as_slice(), &BYTES[..64]);
            assert_eq!(r.close_admission(), Err(DriverError::Busy));
            assert_eq!(releases.load(Ordering::SeqCst), 3);
            drop(packet);
            assert_eq!(releases.load(Ordering::SeqCst), 4);
            r.enqueue_rx(id, record(&releases, 64)).unwrap();
            r.close_admission().unwrap();
            r.reset_runtime();
            assert_eq!(releases.load(Ordering::SeqCst), 5);
            assert!(!r.has_rx(id));
        }
    }

    #[test]
    fn station_disconnect_and_stop_drain_only_queued_station_frames_before_reconnect() {
        for event in [
            WifiEvent::StaDisconnected { reason: Some(8) },
            WifiEvent::StaStopped,
        ] {
            let sta_releases = Arc::new(AtomicUsize::new(0));
            let ap_releases = Arc::new(AtomicUsize::new(0));
            let r = Resources::try_new(
                &QueueConfig {
                    station_rx: 2,
                    access_point_rx: 2,
                    ..QueueConfig::default()
                },
                test_allocator(),
            )
            .unwrap();
            r.publish_event(WifiEvent::StaConnected).unwrap();
            r.publish_event(WifiEvent::ApStarted).unwrap();
            r.enqueue_rx(InterfaceId::Station, record(&sta_releases, 64))
                .unwrap();
            let held_sta = r.take_rx(InterfaceId::Station).unwrap();
            r.enqueue_rx(InterfaceId::Station, record(&sta_releases, 64))
                .unwrap();
            r.enqueue_rx(InterfaceId::Station, record(&sta_releases, 64))
                .unwrap();
            r.enqueue_rx(InterfaceId::AccessPoint, record(&ap_releases, 64))
                .unwrap();
            let held_ap = r.take_rx(InterfaceId::AccessPoint).unwrap();
            r.enqueue_rx(InterfaceId::AccessPoint, record(&ap_releases, 64))
                .unwrap();
            r.publish_event(event).unwrap();
            assert_eq!(sta_releases.load(Ordering::SeqCst), 2);
            assert_eq!(ap_releases.load(Ordering::SeqCst), 0);
            assert_eq!(held_sta.as_slice(), &BYTES[..64]);
            assert_eq!(held_ap.as_slice(), &BYTES[..64]);
            assert!(r.has_rx(InterfaceId::AccessPoint));
            r.publish_event(WifiEvent::StaConnected).unwrap();
            assert!(r.take_rx(InterfaceId::Station).is_none());
            r.enqueue_rx(InterfaceId::Station, record(&sta_releases, 128))
                .unwrap();
            assert_eq!(r.take_rx(InterfaceId::Station).unwrap().as_slice(), BYTES);
            drop(held_sta);
            drop(held_ap);
            r.drain_ap_rx();
            assert_eq!(sta_releases.load(Ordering::SeqCst), 4);
            assert_eq!(ap_releases.load(Ordering::SeqCst), 2);
        }
    }

    #[test]
    fn ap_stopped_drains_ap_queue_without_discarding_station_frames() {
        let releases = Arc::new(AtomicUsize::new(0));
        let r = resources();
        r.publish_event(WifiEvent::StaConnected).unwrap();
        r.publish_event(WifiEvent::ApStarted).unwrap();
        r.enqueue_rx(InterfaceId::Station, record(&releases, 64))
            .unwrap();
        r.enqueue_rx(InterfaceId::AccessPoint, record(&releases, 64))
            .unwrap();
        r.publish_event(WifiEvent::ApStopped).unwrap();
        assert_eq!(releases.load(Ordering::SeqCst), 1);
        r.publish_event(WifiEvent::ApStarted).unwrap();
        assert!(r.take_rx(InterfaceId::AccessPoint).is_none());
        assert!(r.has_rx(InterfaceId::Station));
        drop(r.take_rx(InterfaceId::Station).unwrap());
        assert_eq!(releases.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn disconnect_releases_outside_state_borrow_and_wakes_after_all_queued_releases() {
        use std::boxed::Box;
        struct ReleaseCheck {
            resources: *const Resources,
            releases: Arc<AtomicUsize>,
        }
        unsafe fn release(_: *mut core::ffi::c_void, context: *mut core::ffi::c_void) {
            // SAFETY: each record owns its context Box, and Resources stays
            // alive until publish_event has released all these queued records.
            let context = unsafe { Box::from_raw(context.cast::<ReleaseCheck>()) };
            let resources = unsafe { &*context.resources };
            assert_eq!(resources.link_state(), LinkState::Down);
            assert!(!resources.has_rx(InterfaceId::Station));
            assert!(resources.take_rx(InterfaceId::Station).is_none());
            context.releases.fetch_add(1, Ordering::SeqCst);
        }
        struct DrainWake {
            releases: Arc<AtomicUsize>,
            wakes: AtomicUsize,
        }
        impl Wake for DrainWake {
            fn wake(self: Arc<Self>) {
                assert_eq!(self.releases.load(Ordering::SeqCst), 2);
                self.wakes.fetch_add(1, Ordering::SeqCst);
            }
        }
        let r = Box::new(
            Resources::try_new(
                &QueueConfig {
                    station_rx: 2,
                    ..QueueConfig::default()
                },
                test_allocator(),
            )
            .unwrap(),
        );
        r.publish_event(WifiEvent::StaConnected).unwrap();
        let releases = Arc::new(AtomicUsize::new(0));
        for _ in 0..2 {
            let context = Box::new(ReleaseCheck {
                resources: &*r,
                releases: releases.clone(),
            });
            // SAFETY: static initialized bytes, one owned context per release,
            // boxed Resources remains stationary and alive throughout the drain.
            let record = unsafe {
                RawRxRecord::from_raw(
                    BYTES.as_ptr().cast_mut().cast(),
                    64,
                    Box::into_raw(context).cast(),
                    release,
                )
            };
            r.enqueue_rx(InterfaceId::Station, record).unwrap();
        }
        let wake = Arc::new(DrainWake {
            releases,
            wakes: AtomicUsize::new(0),
        });
        r.register_control_waker(&wake.clone().into());
        r.register_link_waker(InterfaceId::Station, &wake.clone().into());
        r.publish_event(WifiEvent::StaDisconnected { reason: Some(8) })
            .unwrap();
        assert_eq!(wake.wakes.load(Ordering::SeqCst), 2);
        assert_eq!(r.state.with(|s| s.rx[0].len()), 0);
    }

    #[cfg(feature = "wifi-esp-now")]
    #[test]
    fn esp_now_peer_capacity_is_scalar_and_does_not_allocate_peer_storage() {
        use super::super::allocator::TrackingAllocator;
        let (provider, allocator) = TrackingAllocator::new(0);
        let r = Resources::try_new(
            &QueueConfig {
                station_rx: 0,
                access_point_rx: 0,
                events: 0,
                esp_now_peers: usize::MAX,
                esp_now_rx: 0,
                promiscuous_rx: 0,
                ..QueueConfig::default()
            },
            allocator,
        )
        .unwrap();
        assert_eq!(r.esp_now_peer_capacity(), usize::MAX);
        assert_eq!(provider.live.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn disabled_rx_queues_release_both_interfaces() {
        let releases = Arc::new(AtomicUsize::new(0));
        let r = Resources::try_new(
            &QueueConfig {
                station_rx: 0,
                access_point_rx: 0,
                ..QueueConfig::default()
            },
            test_allocator(),
        )
        .unwrap();
        r.publish_event(WifiEvent::StaConnected).unwrap();
        r.publish_event(WifiEvent::ApStarted).unwrap();
        for id in [InterfaceId::Station, InterfaceId::AccessPoint] {
            assert_eq!(
                r.enqueue_rx(id, record(&releases, 64)),
                Err(DriverError::PacketOverflow)
            );
            assert!(r.take_rx(id).is_none());
        }
        assert_eq!(releases.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn ap_rx_lease_blocks_radio_stop_but_not_station_disconnect() {
        let releases = Arc::new(AtomicUsize::new(0));
        let r = resources();
        r.publish_event(WifiEvent::StaConnected).unwrap();
        r.publish_event(WifiEvent::ApStarted).unwrap();
        r.enqueue_rx(InterfaceId::AccessPoint, record(&releases, 64))
            .unwrap();
        let ap = r.take_rx(InterfaceId::AccessPoint).unwrap();
        assert_eq!(r.close_admission(), Err(DriverError::Busy));
        // A separate station lease still blocks station disconnection.
        r.enqueue_rx(InterfaceId::Station, record(&releases, 64))
            .unwrap();
        let sta = r.take_rx(InterfaceId::Station).unwrap();
        assert_eq!(r.close_station(), Err(DriverError::Busy));
        drop(sta);
        r.close_station().unwrap();
        assert_eq!(ap.as_slice(), &BYTES[..64]);
        assert_eq!(r.close_admission(), Err(DriverError::Busy));
        drop(ap);
        r.close_admission().unwrap();
        assert_eq!(releases.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn ap_rx_storage_scales_with_record_count_not_frame_capacity() {
        use super::super::allocator::TrackingAllocator;
        let allocated = |access_point_rx, frame_bytes| {
            let (provider, allocator) = TrackingAllocator::new(32);
            let r = Resources::try_new(
                &QueueConfig {
                    access_point_rx,
                    frame_bytes,
                    ..QueueConfig::default()
                },
                allocator,
            )
            .unwrap();
            let bytes = provider.live_bytes.load(Ordering::SeqCst);
            drop(r);
            assert_eq!(provider.live.load(Ordering::SeqCst), 0);
            assert_eq!(provider.live_bytes.load(Ordering::SeqCst), 0);
            bytes
        };
        let small = allocated(1, super::super::MIN_IPV4_ETHERNET_FRAME_SIZE);
        let wide = allocated(1, super::super::MAX_ETHERNET_FRAME_SIZE);
        assert_eq!(small, wide);
        assert_eq!(
            allocated(4, super::super::MAX_ETHERNET_FRAME_SIZE) - wide,
            3 * core::mem::size_of::<Option<RawRxRecord>>()
        );
    }
    #[cfg(feature = "wifi-smoltcp")]
    fn empty_queues(frame_bytes: usize) -> QueueConfig {
        QueueConfig {
            station_rx: 0,
            access_point_rx: 0,
            frame_bytes,
            events: 0,
            esp_now_peers: 0,
            esp_now_rx: 0,
            promiscuous_rx: 0,
        }
    }

    #[cfg(feature = "wifi-smoltcp")]
    #[test]
    fn smoltcp_tx_uses_provided_frame_storage_without_allocating() {
        use super::super::{allocator::TrackingAllocator, Interface};
        use crate::wifi::device::WifiDevice;
        use smoltcp::phy::{Device as _, TxToken as _};
        for frame_bytes in [672, super::super::MAX_ETHERNET_FRAME_SIZE] {
            let (provider, allocator) = TrackingAllocator::new(0);
            let r = Resources::try_new(&empty_queues(frame_bytes), allocator).unwrap();
            assert_eq!(provider.live.load(Ordering::SeqCst), 0);
            r.publish_event(WifiEvent::StaConnected).unwrap();
            let mut storage = [0; super::super::MAX_ETHERNET_FRAME_SIZE];
            let interface = {
                let interface = Interface {
                    resources: &r,
                    id: InterfaceId::Station,
                    mac: [2, 0, 0, 0, 0, 1],
                };
                let mut device = WifiDevice::new(interface, &mut storage).unwrap();
                assert_eq!(provider.live.load(Ordering::SeqCst), 0);
                assert_eq!(provider.live_bytes.load(Ordering::SeqCst), 0);
                assert_eq!(device.capabilities().max_transmission_unit, frame_bytes);
                let token = device
                    .transmit(smoltcp::time::Instant::from_millis(0))
                    .unwrap();
                token.consume(frame_bytes, |frame| {
                    assert_eq!(frame.len(), frame_bytes);
                    frame.fill(0xa5);
                });
                // No allocation budget is available. Native TX is unavailable
                // on this host, but the caller's storage holds the full frame.
                assert_eq!(provider.live.load(Ordering::SeqCst), 0);
                assert_eq!(provider.live_bytes.load(Ordering::SeqCst), 0);
                device.into_interface()
            };
            assert!(storage[..frame_bytes].iter().all(|&byte| byte == 0xa5));
            assert!(storage[frame_bytes..].iter().all(|&byte| byte == 0));
            // Consuming the adapter returns its interface and ends the separate
            // TX storage borrow, so both resources can be used independently.
            storage.fill(0);
            assert_eq!(interface.max_frame_size(), frame_bytes);
            assert_eq!(provider.live.load(Ordering::SeqCst), 0);
            assert_eq!(provider.live_bytes.load(Ordering::SeqCst), 0);
        }
    }

    #[cfg(feature = "wifi-smoltcp")]
    #[test]
    fn smoltcp_tx_rejects_undersized_provided_storage_without_allocating() {
        use super::super::{allocator::TrackingAllocator, Interface};
        use crate::wifi::device::WifiDevice;
        for frame_bytes in [672, super::super::MAX_ETHERNET_FRAME_SIZE] {
            let (provider, allocator) = TrackingAllocator::new(0);
            let r = Resources::try_new(&empty_queues(frame_bytes), allocator).unwrap();
            let mut storage = [0; super::super::MAX_ETHERNET_FRAME_SIZE];
            let interface = Interface {
                resources: &r,
                id: InterfaceId::Station,
                mac: [2, 0, 0, 0, 0, 1],
            };
            assert!(matches!(
                WifiDevice::new(interface, &mut storage[..frame_bytes - 1]),
                Err(DriverError::InvalidFrameSize)
            ));
            assert_eq!(provider.live.load(Ordering::SeqCst), 0);
            assert_eq!(provider.live_bytes.load(Ordering::SeqCst), 0);
        }
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
