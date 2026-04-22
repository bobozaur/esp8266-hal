use super::{
    ApStationInfo, Channel, ClientConfig, ConnectError, Deadline, DisconnectError, DriverError,
    EspNowAccessPointPeerConfig, EspNowPeerConfig, EspNowRxFrame, EspNowStationPeerConfig,
    EspNowTxStatus, EventCursor, HasAp, HasSta, LinkState, PromiscuousControlFilter,
    PromiscuousFilter, PromiscuousRxFrame, SmartConfigStartConfig, SmartConfigTimeout,
    SmartConfigType, Started, WifiBackend, WifiDataLink, WifiDriver, WifiEvent, WifiResources,
    WpsConfig, ESP_NOW_MAX_PAYLOAD,
};
use core::marker::PhantomData;
use core::mem::ManuallyDrop;
use core::task::{Poll, Waker};

const PROMISCUOUS_SETTLE_SPINS: u8 = 2;

pub struct StaSession {
    connected: bool,
}

impl StaSession {
    pub const fn is_connected(&self) -> bool {
        self.connected
    }
}

pub struct StaHandle<
    'a,
    B,
    const EVENTS: usize,
    const RX: usize,
    const SUBS: usize,
    const ESP_NOW_PEERS: usize = 0,
    const ESP_NOW_RX: usize = 0,
    const PROMISCUOUS_RX: usize = 0,
> {
    pub(crate) backend: &'a mut B,
    pub(crate) resources:
        &'a WifiResources<EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX>,
    pub(crate) cursor: EventCursor,
}

pub struct ApHandle<
    'a,
    B,
    const EVENTS: usize,
    const RX: usize,
    const SUBS: usize,
    const ESP_NOW_PEERS: usize = 0,
    const ESP_NOW_RX: usize = 0,
    const PROMISCUOUS_RX: usize = 0,
> {
    pub(crate) backend: &'a mut B,
    pub(crate) resources:
        &'a WifiResources<EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX>,
    pub(crate) cursor: EventCursor,
}

pub struct StaLink<
    'd,
    M,
    B: WifiBackend,
    const EVENTS: usize,
    const RX: usize,
    const SUBS: usize,
    const ESP_NOW_PEERS: usize = 0,
    const ESP_NOW_RX: usize = 0,
    const PROMISCUOUS_RX: usize = 0,
> {
    pub(crate) backend: ManuallyDrop<B>,
    pub(crate) resources:
        &'d WifiResources<EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX>,
    pub(crate) rx_attached: bool,
    pub(crate) _mode: PhantomData<M>,
}

pub struct ApLink<
    'd,
    M,
    B: WifiBackend,
    const EVENTS: usize,
    const RX: usize,
    const SUBS: usize,
    const ESP_NOW_PEERS: usize = 0,
    const ESP_NOW_RX: usize = 0,
    const PROMISCUOUS_RX: usize = 0,
> {
    pub(crate) backend: ManuallyDrop<B>,
    pub(crate) resources:
        &'d WifiResources<EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX>,
    pub(crate) rx_attached: bool,
    pub(crate) _mode: PhantomData<M>,
}

pub struct PromiscuousSession<
    'a,
    B: WifiBackend,
    const EVENTS: usize,
    const RX: usize,
    const SUBS: usize,
    const ESP_NOW_PEERS: usize = 0,
    const ESP_NOW_RX: usize = 0,
    const PROMISCUOUS_RX: usize = 0,
> {
    pub(crate) backend: &'a mut B,
    pub(crate) resources:
        &'a WifiResources<EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX>,
    active: bool,
    filter: PromiscuousFilter,
    control_filter: PromiscuousControlFilter,
}

pub struct EspNowSession<
    'a,
    M,
    B: WifiBackend,
    const EVENTS: usize,
    const RX: usize,
    const SUBS: usize,
    const ESP_NOW_PEERS: usize = 0,
    const ESP_NOW_RX: usize = 0,
    const PROMISCUOUS_RX: usize = 0,
> {
    pub(crate) backend: &'a mut B,
    pub(crate) resources:
        &'a WifiResources<EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX>,
    active: bool,
    _mode: PhantomData<M>,
}

pub struct WpsSession<
    'a,
    B: WifiBackend,
    const EVENTS: usize = 0,
    const RX: usize = 0,
    const SUBS: usize = 0,
    const ESP_NOW_PEERS: usize = 0,
    const ESP_NOW_RX: usize = 0,
    const PROMISCUOUS_RX: usize = 0,
> {
    backend: &'a mut B,
    resources: &'a WifiResources<EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX>,
    cursor: EventCursor,
    active: bool,
}

pub struct SmartConfigSession<
    'a,
    B: WifiBackend,
    const EVENTS: usize = 0,
    const RX: usize = 0,
    const SUBS: usize = 0,
    const ESP_NOW_PEERS: usize = 0,
    const ESP_NOW_RX: usize = 0,
    const PROMISCUOUS_RX: usize = 0,
> {
    backend: &'a mut B,
    resources: &'a WifiResources<EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX>,
    cursor: EventCursor,
    active: bool,
}

impl<
        'a,
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
    > StaHandle<'a, B, EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX>
{
    pub fn connect(&mut self) -> Result<(), ConnectError> {
        self.backend.connect()
    }

    pub fn connect_blocking(&mut self, timeout_ms: u32) -> Result<StaSession, DriverError> {
        if let Some(result) = self.backend.connect_blocking(timeout_ms) {
            result?;
            return Ok(StaSession { connected: true });
        }

        self.connect().map_err(map_connect_error)?;
        let deadline = Deadline::new(self.backend.now_ms(), timeout_ms);

        loop {
            let cursor = &mut self.cursor;
            if let Some(result) = self.resources.events.with(|events| {
                while let Some(event) = events.try_next(cursor) {
                    match event {
                        WifiEvent::StaConnected => {
                            return Some(Ok(StaSession { connected: true }));
                        }
                        WifiEvent::StaDisconnected { .. } => {
                            return Some(Err(DriverError::Disconnected));
                        }
                        _ => {}
                    }
                }

                None
            }) {
                return result;
            }

            if deadline.is_expired(self.backend.now_ms()) {
                return Err(DriverError::Timeout);
            }

            self.backend.spin_once();
        }
    }

    pub async fn connect_async(&mut self) -> Result<StaSession, ConnectError> {
        self.connect()?;
        let resources = self.resources;
        let cursor = &mut self.cursor;

        core::future::poll_fn(|cx| {
            resources.register_link_waker(cx.waker());

            if let Some(result) = resources.events.with(|events| {
                while let Some(event) = events.try_next(cursor) {
                    match event {
                        WifiEvent::StaConnected => {
                            return Some(Ok(StaSession { connected: true }));
                        }
                        WifiEvent::StaDisconnected { .. } => {
                            return Some(Err(ConnectError::Disconnected));
                        }
                        _ => {}
                    }
                }

                None
            }) {
                Poll::Ready(result)
            } else {
                Poll::Pending
            }
        })
        .await
    }

    pub fn disconnect(&mut self) -> Result<(), DisconnectError> {
        self.backend.disconnect()
    }
}

impl<
        B,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
    > Drop for StaHandle<'_, B, EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX>
{
    fn drop(&mut self) {
        self.resources.release_event_cursor(&mut self.cursor);
    }
}

impl<
        'a,
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
    > ApHandle<'a, B, EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX>
{
    pub fn connected_stations<const N: usize>(
        &mut self,
    ) -> Result<heapless::Vec<ApStationInfo, N>, DriverError> {
        self.backend.connected_ap_stations()
    }

    pub fn deauth_station(&mut self, aid: u16) -> Result<(), DriverError> {
        self.backend.deauth_ap_station(aid)
    }

    pub fn deauth_all(&mut self) -> Result<(), DriverError> {
        self.backend.deauth_ap_station(0)
    }
}

impl<
        B,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
    > Drop for ApHandle<'_, B, EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX>
{
    fn drop(&mut self) {
        self.resources.release_event_cursor(&mut self.cursor);
    }
}

impl<
        'd,
        M: HasSta,
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
    > StaLink<'d, M, B, EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX>
{
    pub fn register_link_waker(&self, waker: &Waker) {
        self.resources.register_link_waker(waker);
    }

    pub fn register_rx_waker(&self, waker: &Waker) {
        self.resources.register_rx_waker(waker);
    }

    pub fn link_state(&self) -> LinkState {
        self.resources.link_state()
    }

    pub fn last_disconnect_reason(&self) -> Option<u8> {
        self.resources.last_disconnect_reason()
    }

    pub fn mac_address(&self) -> Result<[u8; 6], DriverError> {
        self.backend.sta_mac_address()
    }

    pub fn refresh_sta_rx(&mut self) -> Result<(), DriverError> {
        self.backend.refresh_sta_rx()
    }

    pub fn has_rx_data(&self) -> bool {
        self.resources.queued_rx() != 0
    }

    pub fn queued_rx(&self) -> usize {
        self.resources.queued_rx()
    }

    pub fn rx_drops(&self) -> u32 {
        self.resources.rx_drops()
    }

    pub fn receive(&mut self, dst: &mut [u8]) -> Option<usize> {
        self.resources
            .dequeue_rx()
            .map(|record| record.copy_into(dst))
    }

    pub fn transmit(&mut self, frame: &[u8]) -> Result<(), DriverError> {
        self.backend.transmit(frame)
    }

    pub fn resources(
        &self,
    ) -> &'d WifiResources<EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX> {
        self.resources
    }

    pub fn into_driver(
        mut self,
    ) -> WifiDriver<'d, Started<M>, B, EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX>
    {
        self.backend.detach_sta_rx();
        self.rx_attached = false;
        let resources = self.resources;
        // SAFETY: `self` is immediately forgotten, so the backend is moved out
        // exactly once and will not be dropped by `StaLink::drop`.
        let backend = unsafe { ManuallyDrop::take(&mut self.backend) };
        core::mem::forget(self);
        WifiDriver::from_started_parts(resources, backend)
    }
}

impl<
        M,
        B,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
    > Drop for StaLink<'_, M, B, EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX>
where
    B: WifiBackend,
{
    fn drop(&mut self) {
        if self.rx_attached {
            self.backend.detach_sta_rx();
        }
        // SAFETY: `StaLink` owns the backend unless `into_driver` forgot `self`;
        // this drop path runs only for the owned case.
        unsafe { ManuallyDrop::drop(&mut self.backend) };
    }
}

impl<
        'd,
        M: HasAp,
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
    > ApLink<'d, M, B, EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX>
{
    pub fn register_link_waker(&self, waker: &Waker) {
        self.resources.register_link_waker(waker);
    }

    pub fn register_rx_waker(&self, waker: &Waker) {
        self.resources.register_ap_rx_waker(waker);
    }

    pub fn link_state(&self) -> LinkState {
        self.resources.ap_link_state()
    }

    pub fn mac_address(&self) -> Result<[u8; 6], DriverError> {
        self.backend.ap_mac_address()
    }

    pub fn refresh_ap_rx(&mut self) -> Result<(), DriverError> {
        self.backend.refresh_ap_rx()
    }

    pub fn has_rx_data(&self) -> bool {
        self.resources.queued_ap_rx() != 0
    }

    pub fn queued_rx(&self) -> usize {
        self.resources.queued_ap_rx()
    }

    pub fn rx_drops(&self) -> u32 {
        self.resources.ap_rx_drops()
    }

    pub fn receive(&mut self, dst: &mut [u8]) -> Option<usize> {
        self.resources
            .dequeue_ap_rx()
            .map(|record| record.copy_into(dst))
    }

    pub fn transmit(&mut self, frame: &[u8]) -> Result<(), DriverError> {
        self.backend.transmit_ap(frame)
    }

    pub fn resources(
        &self,
    ) -> &'d WifiResources<EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX> {
        self.resources
    }

    pub fn into_driver(
        mut self,
    ) -> WifiDriver<'d, Started<M>, B, EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX>
    {
        self.backend.detach_ap_rx();
        self.rx_attached = false;
        let resources = self.resources;
        // SAFETY: `self` is immediately forgotten, so the backend is moved out
        // exactly once and will not be dropped by `ApLink::drop`.
        let backend = unsafe { ManuallyDrop::take(&mut self.backend) };
        core::mem::forget(self);
        WifiDriver::from_started_parts(resources, backend)
    }
}

impl<
        M,
        B,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
    > Drop for ApLink<'_, M, B, EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX>
where
    B: WifiBackend,
{
    fn drop(&mut self) {
        if self.rx_attached {
            self.backend.detach_ap_rx();
        }
        // SAFETY: `ApLink` owns the backend unless `into_driver` forgot `self`;
        // this drop path runs only for the owned case.
        unsafe { ManuallyDrop::drop(&mut self.backend) };
    }
}

impl<
        M: HasSta,
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
    > WifiDataLink
    for StaLink<'_, M, B, EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX>
{
    fn register_link_waker(&self, waker: &Waker) {
        StaLink::register_link_waker(self, waker);
    }

    fn register_rx_waker(&self, waker: &Waker) {
        StaLink::register_rx_waker(self, waker);
    }

    fn link_state(&self) -> LinkState {
        StaLink::link_state(self)
    }

    fn mac_address(&self) -> Result<[u8; 6], DriverError> {
        StaLink::mac_address(self)
    }

    fn refresh_rx(&mut self) -> Result<(), DriverError> {
        self.refresh_sta_rx()
    }

    fn has_rx_data(&self) -> bool {
        StaLink::has_rx_data(self)
    }

    fn queued_rx(&self) -> usize {
        StaLink::queued_rx(self)
    }

    fn rx_drops(&self) -> u32 {
        StaLink::rx_drops(self)
    }

    fn receive(&mut self, dst: &mut [u8]) -> Option<usize> {
        StaLink::receive(self, dst)
    }

    fn transmit(&mut self, frame: &[u8]) -> Result<(), DriverError> {
        StaLink::transmit(self, frame)
    }
}

impl<
        M: HasAp,
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
    > WifiDataLink
    for ApLink<'_, M, B, EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX>
{
    fn register_link_waker(&self, waker: &Waker) {
        ApLink::register_link_waker(self, waker);
    }

    fn register_rx_waker(&self, waker: &Waker) {
        ApLink::register_rx_waker(self, waker);
    }

    fn link_state(&self) -> LinkState {
        ApLink::link_state(self)
    }

    fn mac_address(&self) -> Result<[u8; 6], DriverError> {
        ApLink::mac_address(self)
    }

    fn refresh_rx(&mut self) -> Result<(), DriverError> {
        self.refresh_ap_rx()
    }

    fn has_rx_data(&self) -> bool {
        ApLink::has_rx_data(self)
    }

    fn queued_rx(&self) -> usize {
        ApLink::queued_rx(self)
    }

    fn rx_drops(&self) -> u32 {
        ApLink::rx_drops(self)
    }

    fn receive(&mut self, dst: &mut [u8]) -> Option<usize> {
        ApLink::receive(self, dst)
    }

    fn transmit(&mut self, frame: &[u8]) -> Result<(), DriverError> {
        ApLink::transmit(self, frame)
    }
}

impl<
        'a,
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
    > PromiscuousSession<'a, B, EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX>
{
    pub(crate) fn new(
        backend: &'a mut B,
        resources: &'a WifiResources<EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX>,
    ) -> Self {
        Self {
            backend,
            resources,
            active: false,
            filter: PromiscuousFilter::empty(),
            control_filter: PromiscuousControlFilter::empty(),
        }
    }

    pub fn start(&mut self, filter: PromiscuousFilter) -> Result<(), DriverError> {
        for _ in 0..PROMISCUOUS_SETTLE_SPINS {
            self.backend.spin_once();
        }
        self.backend.enable_promiscuous(filter)?;
        if !self.control_filter.is_empty() {
            if let Err(error) = self
                .backend
                .set_promiscuous_control_filter(self.control_filter)
            {
                let _ = self.backend.disable_promiscuous();
                return Err(error);
            }
        }
        self.active = true;
        self.filter = filter;
        Ok(())
    }

    pub fn set_channel(&mut self, channel: Channel) -> Result<(), DriverError> {
        self.backend.set_channel(channel)?;
        for _ in 0..PROMISCUOUS_SETTLE_SPINS {
            self.backend.spin_once();
        }
        Ok(())
    }

    pub fn set_control_filter(
        &mut self,
        filter: PromiscuousControlFilter,
    ) -> Result<(), DriverError> {
        if self.active {
            self.backend.set_promiscuous_control_filter(filter)?;
            self.control_filter = filter;
            return Ok(());
        }

        self.control_filter = filter;
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), DriverError> {
        if self.active {
            self.backend.disable_promiscuous()?;
            self.active = false;
            self.filter = PromiscuousFilter::empty();
        }
        Ok(())
    }

    pub const fn is_active(&self) -> bool {
        self.active
    }

    pub const fn filter(&self) -> PromiscuousFilter {
        self.filter
    }

    pub const fn control_filter(&self) -> PromiscuousControlFilter {
        self.control_filter
    }

    pub fn receive(&mut self) -> Option<PromiscuousRxFrame> {
        self.resources.dequeue_promiscuous_rx()
    }

    pub fn queued(&self) -> usize {
        self.resources.queued_promiscuous_rx()
    }

    pub fn drops(&self) -> u32 {
        self.resources.promiscuous_rx_drops()
    }
}

impl<
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
    > Drop
    for PromiscuousSession<'_, B, EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX>
{
    fn drop(&mut self) {
        if self.active {
            let _ = self.backend.disable_promiscuous();
        }
    }
}

impl<
        'a,
        M,
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
    > EspNowSession<'a, M, B, EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX>
{
    pub(crate) fn new(
        backend: &'a mut B,
        resources: &'a WifiResources<EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX>,
    ) -> Self {
        Self {
            backend,
            resources,
            active: false,
            _mode: PhantomData,
        }
    }

    pub fn start(&mut self) -> Result<(), DriverError> {
        if self.active {
            return Ok(());
        }

        self.backend.enable_esp_now()?;
        self.active = true;
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), DriverError> {
        if self.active {
            self.backend.disable_esp_now()?;
            self.active = false;
        }
        Ok(())
    }

    pub const fn is_active(&self) -> bool {
        self.active
    }

    pub fn set_channel(&mut self, channel: Channel) -> Result<(), DriverError> {
        self.backend.set_channel(channel)
    }

    pub fn set_pmk(&mut self, pmk: &[u8; 16]) -> Result<(), DriverError> {
        self.ensure_active()?;
        self.backend.set_esp_now_pmk(pmk)
    }

    fn add_peer_inner(&mut self, peer: &EspNowPeerConfig) -> Result<bool, DriverError> {
        self.ensure_active()?;
        if self.resources.contains_esp_now_peer(peer.peer_addr()) {
            return Ok(false);
        }
        if self.resources.esp_now_peer_count() == self.resources.esp_now_peer_capacity() {
            return Err(DriverError::PeerOverflow);
        }

        self.backend.add_esp_now_peer(peer)?;
        self.resources.register_esp_now_peer(peer.peer_record())
    }

    fn update_peer_inner(&mut self, peer: &EspNowPeerConfig) -> Result<bool, DriverError> {
        self.ensure_active()?;
        if !self.resources.contains_esp_now_peer(peer.peer_addr()) {
            return Err(DriverError::BackendRejected);
        }

        self.backend.modify_esp_now_peer(peer)?;
        self.resources.register_esp_now_peer(peer.peer_record())
    }

    pub fn remove_peer(&mut self, peer_addr: &[u8; 6]) -> Result<bool, DriverError> {
        self.ensure_active()?;
        self.backend.remove_esp_now_peer(peer_addr)?;
        Ok(self.resources.remove_esp_now_peer(peer_addr))
    }

    pub fn send(&mut self, peer_addr: &[u8; 6], payload: &[u8]) -> Result<(), DriverError> {
        self.ensure_active()?;
        if payload.len() > ESP_NOW_MAX_PAYLOAD {
            return Err(DriverError::PayloadTooLong);
        }
        if !self.resources.contains_esp_now_peer(peer_addr) {
            return Err(DriverError::BackendRejected);
        }
        self.backend.send_esp_now(peer_addr, payload)
    }

    pub fn receive(&mut self) -> Option<EspNowRxFrame> {
        self.resources.dequeue_esp_now_rx()
    }

    pub fn send_status(&mut self) -> Option<EspNowTxStatus> {
        self.resources.dequeue_esp_now_tx_status()
    }

    pub fn queued(&self) -> usize {
        self.resources.queued_esp_now_rx()
    }

    pub fn drops(&self) -> u32 {
        self.resources.esp_now_rx_drops()
    }

    pub fn queued_send_status(&self) -> usize {
        self.resources.queued_esp_now_tx_status()
    }

    pub fn send_status_drops(&self) -> u32 {
        self.resources.esp_now_tx_status_drops()
    }

    pub fn peer_count(&self) -> usize {
        self.resources.esp_now_peer_count()
    }

    fn ensure_active(&self) -> Result<(), DriverError> {
        if self.active {
            Ok(())
        } else {
            Err(DriverError::NotStarted)
        }
    }
}

impl<
        'a,
        M: HasSta,
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
    > EspNowSession<'a, M, B, EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX>
{
    pub fn add_station_peer(&mut self, peer: EspNowStationPeerConfig) -> Result<bool, DriverError> {
        self.add_peer_inner(peer.inner())
    }

    pub fn update_station_peer(
        &mut self,
        peer: EspNowStationPeerConfig,
    ) -> Result<bool, DriverError> {
        self.update_peer_inner(peer.inner())
    }
}

impl<
        'a,
        M: HasAp,
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
    > EspNowSession<'a, M, B, EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX>
{
    pub fn add_access_point_peer(
        &mut self,
        peer: EspNowAccessPointPeerConfig,
    ) -> Result<bool, DriverError> {
        self.add_peer_inner(peer.inner())
    }

    pub fn update_access_point_peer(
        &mut self,
        peer: EspNowAccessPointPeerConfig,
    ) -> Result<bool, DriverError> {
        self.update_peer_inner(peer.inner())
    }
}

impl<
        M,
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
    > Drop
    for EspNowSession<'_, M, B, EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX>
{
    fn drop(&mut self) {
        if self.active {
            let _ = self.backend.disable_esp_now();
        }
    }
}

impl<
        'a,
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
    > WpsSession<'a, B, EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX>
{
    pub(crate) fn new(
        backend: &'a mut B,
        resources: &'a WifiResources<EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX>,
    ) -> Result<Self, DriverError> {
        let cursor = resources.claim_event_cursor()?;
        Ok(Self {
            backend,
            resources,
            cursor,
            active: false,
        })
    }

    pub fn start(&mut self, config: &WpsConfig, timeout_ms: i32) -> Result<(), DriverError> {
        self.backend.enable_wps(config)?;
        if let Err(error) = self.backend.start_wps(timeout_ms) {
            let _ = self.backend.disable_wps();
            return Err(error);
        }
        self.active = true;
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), DriverError> {
        if self.active {
            self.backend.disable_wps()?;
            self.active = false;
        }
        Ok(())
    }

    pub const fn is_active(&self) -> bool {
        self.active
    }

    pub fn next_event(&mut self) -> Option<WifiEvent> {
        self.resources
            .events
            .with(|events| events.try_next(&mut self.cursor))
    }
}

impl<
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
    > Drop for WpsSession<'_, B, EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX>
{
    fn drop(&mut self) {
        if self.active {
            let _ = self.backend.disable_wps();
        }
        self.resources.release_event_cursor(&mut self.cursor);
    }
}

impl<
        'a,
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
    > SmartConfigSession<'a, B, EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX>
{
    pub(crate) fn new(
        backend: &'a mut B,
        resources: &'a WifiResources<EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX>,
    ) -> Result<Self, DriverError> {
        let cursor = resources.claim_event_cursor()?;
        Ok(Self {
            backend,
            resources,
            cursor,
            active: false,
        })
    }

    pub fn set_type(&mut self, kind: SmartConfigType) -> Result<(), DriverError> {
        self.backend.set_smartconfig_type(kind)
    }

    pub fn set_timeout(&mut self, timeout: SmartConfigTimeout) -> Result<(), DriverError> {
        self.backend.set_smartconfig_timeout(timeout)
    }

    pub fn set_fast_mode(&mut self, enabled: bool) -> Result<(), DriverError> {
        self.backend.set_smartconfig_fast_mode(enabled)
    }

    pub fn start(&mut self, config: &SmartConfigStartConfig) -> Result<(), DriverError> {
        self.backend.start_smartconfig(config)?;
        self.active = true;
        Ok(())
    }

    pub fn configure_station(&mut self, config: &ClientConfig) -> Result<(), DriverError> {
        self.backend.configure_sta(config)
    }

    pub fn connect_blocking(&mut self, timeout_ms: u32) -> Result<StaSession, DriverError> {
        if let Some(result) = self.backend.connect_blocking(timeout_ms) {
            result?;
            return Ok(StaSession { connected: true });
        }

        self.backend.connect().map_err(map_connect_error)?;
        let deadline = Deadline::new(self.backend.now_ms(), timeout_ms);

        loop {
            let cursor = &mut self.cursor;
            if let Some(result) = self.resources.events.with(|events| {
                while let Some(event) = events.try_next(cursor) {
                    match event {
                        WifiEvent::StaConnected => {
                            return Some(Ok(StaSession { connected: true }));
                        }
                        WifiEvent::StaDisconnected { .. } => {
                            return Some(Err(DriverError::Disconnected));
                        }
                        _ => {}
                    }
                }

                None
            }) {
                return result;
            }

            if deadline.is_expired(self.backend.now_ms()) {
                return Err(DriverError::Timeout);
            }

            self.backend.spin_once();
        }
    }

    pub fn stop(&mut self) -> Result<(), DriverError> {
        if self.active {
            self.backend.stop_smartconfig()?;
            self.active = false;
        }
        Ok(())
    }

    pub const fn is_active(&self) -> bool {
        self.active
    }

    pub fn read_reserved_data(&mut self, buffer: &mut [u8]) -> Result<(), DriverError> {
        self.backend.read_smartconfig_reserved_data(buffer)
    }

    pub fn next_event(&mut self) -> Option<WifiEvent> {
        self.resources
            .events
            .with(|events| events.try_next(&mut self.cursor))
    }
}

impl<
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
    > Drop
    for SmartConfigSession<'_, B, EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX>
{
    fn drop(&mut self) {
        if self.active {
            let _ = self.backend.stop_smartconfig();
        }
        self.resources.release_event_cursor(&mut self.cursor);
    }
}

fn map_connect_error(error: ConnectError) -> DriverError {
    match error {
        ConnectError::Timeout => DriverError::Timeout,
        ConnectError::Disconnected => DriverError::Disconnected,
        ConnectError::BackendRejected => DriverError::BackendRejected,
    }
}

#[cfg(test)]
mod tests {
    use crate::wifi_core::{
        ClientConfig, Credential, InitConfig, LinkState, RawMode, RawRxRecord, Ssid, WifiApBackend,
        WifiDriver, WifiEvent, WifiFeatureBackend, WifiLifecycleBackend, WifiPacketBackend,
        WifiResources, WifiRuntimeBackend, WifiScanBackend, WifiStaBackend,
    };

    #[derive(Default)]
    struct FakeBackend {
        connect_called: bool,
    }

    impl WifiRuntimeBackend for FakeBackend {
        fn now_ms(&self) -> u64 {
            0
        }

        fn spin_once(&mut self) {}
    }

    impl WifiLifecycleBackend for FakeBackend {
        fn initialize(&mut self, _config: &InitConfig) -> Result<(), crate::wifi_core::InitError> {
            Ok(())
        }

        fn set_mode(&mut self, _mode: RawMode) -> Result<(), crate::wifi_core::DriverError> {
            Ok(())
        }

        fn start(&mut self) -> Result<(), crate::wifi_core::DriverError> {
            Ok(())
        }

        fn stop(&mut self) -> Result<(), crate::wifi_core::DriverError> {
            Ok(())
        }
    }

    impl WifiStaBackend for FakeBackend {
        fn connect(&mut self) -> Result<(), crate::wifi_core::ConnectError> {
            self.connect_called = true;
            Ok(())
        }

        fn disconnect(&mut self) -> Result<(), crate::wifi_core::DisconnectError> {
            Ok(())
        }
    }

    impl WifiApBackend for FakeBackend {}
    impl WifiPacketBackend for FakeBackend {}
    impl WifiScanBackend for FakeBackend {}
    impl WifiFeatureBackend for FakeBackend {}

    #[test]
    fn sta_handle_invokes_backend_connect() {
        let resources = WifiResources::<8, 4, 2>::new();
        let driver = WifiDriver::with_backend(&resources, FakeBackend::default());
        let driver = driver.initialize(InitConfig::default()).unwrap();
        let mut driver = driver
            .configure_sta(ClientConfig::new(
                Ssid::try_from("core-net").unwrap(),
                Credential::Open,
            ))
            .unwrap()
            .start()
            .unwrap();

        let mut sta = driver.sta().unwrap();
        sta.connect().unwrap();
    }

    #[derive(Default)]
    struct LinkBackend {
        transmitted: heapless::Vec<usize, 4>,
        mac: [u8; 6],
    }

    impl WifiRuntimeBackend for LinkBackend {
        fn now_ms(&self) -> u64 {
            0
        }

        fn spin_once(&mut self) {}
    }

    impl WifiLifecycleBackend for LinkBackend {
        fn initialize(&mut self, _config: &InitConfig) -> Result<(), crate::wifi_core::InitError> {
            Ok(())
        }

        fn set_mode(&mut self, _mode: RawMode) -> Result<(), crate::wifi_core::DriverError> {
            Ok(())
        }

        fn start(&mut self) -> Result<(), crate::wifi_core::DriverError> {
            Ok(())
        }

        fn stop(&mut self) -> Result<(), crate::wifi_core::DriverError> {
            Ok(())
        }
    }

    impl WifiStaBackend for LinkBackend {
        fn connect(&mut self) -> Result<(), crate::wifi_core::ConnectError> {
            Ok(())
        }

        fn disconnect(&mut self) -> Result<(), crate::wifi_core::DisconnectError> {
            Ok(())
        }

        fn sta_mac_address(&self) -> Result<[u8; 6], crate::wifi_core::DriverError> {
            Ok(self.mac)
        }
    }

    impl WifiPacketBackend for LinkBackend {
        fn transmit(&mut self, frame: &[u8]) -> Result<(), crate::wifi_core::DriverError> {
            self.transmitted.push(frame.len()).unwrap();
            Ok(())
        }
    }

    impl WifiApBackend for LinkBackend {}
    impl WifiScanBackend for LinkBackend {}
    impl WifiFeatureBackend for LinkBackend {}

    #[test]
    fn sta_link_reads_core_state_and_transmits_frames() {
        let resources = WifiResources::<8, 4, 2>::new();
        resources.publish_event(WifiEvent::StaConnected).unwrap();
        resources
            .enqueue_rx(unsafe {
                RawRxRecord::from_raw(core::ptr::null_mut(), 24, core::ptr::null_mut(), None)
            })
            .unwrap();

        let backend = LinkBackend {
            mac: [1, 2, 3, 4, 5, 6],
            ..Default::default()
        };

        let driver = WifiDriver::with_backend(&resources, backend)
            .initialize(InitConfig::default())
            .unwrap()
            .configure_sta(ClientConfig::new(
                Ssid::try_from("core-net").unwrap(),
                Credential::Open,
            ))
            .unwrap()
            .start()
            .unwrap();

        let mut link = driver.sta_link().unwrap();
        let mut rx_buf = [0u8; 32];

        assert_eq!(link.link_state(), LinkState::Up);
        assert_eq!(link.mac_address().unwrap(), [1, 2, 3, 4, 5, 6]);
        assert_eq!(link.receive(&mut rx_buf), Some(24));
        link.transmit(&[9, 8, 7]).unwrap();
    }
}
