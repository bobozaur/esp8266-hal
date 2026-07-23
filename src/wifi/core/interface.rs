use super::packet::RawRxRecord;
#[cfg(any(feature = "wifi-esp-now", feature = "wifi-promiscuous"))]
use super::Channel;
#[cfg(all(feature = "wifi", target_arch = "xtensa"))]
use super::WifiRawDataLink;
use super::{
    ApStationInfo, ConnectError, Deadline, DisconnectError, DriverError, HasAp, HasSta, LinkState,
    Started, WifiBackend, WifiDataLink, WifiDriver, WifiResources,
};
#[cfg(feature = "wifi-smartconfig")]
use super::{
    ClientConfig, Ipv4Addr, SmartConfigCredentials, SmartConfigEvent, SmartConfigStartConfig,
    SmartConfigTimeout, SmartConfigType,
};
#[cfg(feature = "wifi-esp-now")]
use super::{
    EspNowAccessPointPeerConfig, EspNowPeerConfig, EspNowRxFrame, EspNowSendStatus,
    EspNowStationPeerConfig, ESP_NOW_MAX_PAYLOAD,
};
#[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps"))]
use super::{EventCursor, WifiEvent};
#[cfg(feature = "wifi-promiscuous")]
use super::{PromiscuousControlFilter, PromiscuousFilter, PromiscuousRxFrame};
#[cfg(feature = "wifi-wps")]
use super::{WpsConfig, WpsEvent};
use core::task::{Poll, Waker};
use core::{cell::RefCell, marker::PhantomData};

#[cfg(feature = "wifi-promiscuous")]
const PROMISCUOUS_SETTLE_SPINS: u8 = 2;
#[cfg(feature = "wifi-smartconfig")]
const ESPTOUCH_ACK_PORT: u16 = 18_266;
#[cfg(feature = "wifi-smartconfig")]
const ESPTOUCH_ACK_FRAME_LEN: usize = 60;
#[cfg(feature = "wifi-smartconfig")]
const ESPTOUCH_ACK_COPIES: usize = 5;
#[cfg(feature = "wifi-smartconfig")]
const ESPTOUCH_ACK_ATTEMPTS: usize = 40;

pub struct StaHandle<
    'a,
    B,
    const EVENTS: usize,
    const RX: usize,
    const ESP_NOW_PEERS: usize = 0,
    const ESP_NOW_RX: usize = 0,
    const PROMISCUOUS_RX: usize = 0,
    const AP_RX_SLOTS: usize = 0,
    const AP_RX_BYTES: usize = 0,
> {
    pub(crate) backend: &'a mut B,
    pub(crate) resources: &'a WifiResources<
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >,
}

pub struct ApHandle<'a, B> {
    pub(crate) backend: &'a mut B,
}

pub struct StaLink<
    'd,
    M,
    B: WifiBackend,
    const EVENTS: usize,
    const RX: usize,
    const ESP_NOW_PEERS: usize = 0,
    const ESP_NOW_RX: usize = 0,
    const PROMISCUOUS_RX: usize = 0,
    const AP_RX_SLOTS: usize = 0,
    const AP_RX_BYTES: usize = 0,
> {
    pub(crate) backend: Option<B>,
    pub(crate) resources: &'d WifiResources<
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >,
    pub(crate) rx_attached: bool,
    pub(crate) _mode: PhantomData<M>,
}

pub struct ApLink<
    'd,
    M,
    B: WifiBackend,
    const EVENTS: usize,
    const RX: usize,
    const ESP_NOW_PEERS: usize = 0,
    const ESP_NOW_RX: usize = 0,
    const PROMISCUOUS_RX: usize = 0,
    const AP_RX_SLOTS: usize = 0,
    const AP_RX_BYTES: usize = 0,
> {
    pub(crate) backend: Option<B>,
    pub(crate) resources: &'d WifiResources<
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >,
    pub(crate) rx_attached: bool,
    pub(crate) _mode: PhantomData<M>,
}

/// Owner for simultaneous station and access-point packet paths.
pub struct MixedLink<
    'd,
    B: WifiBackend,
    const EVENTS: usize,
    const RX: usize,
    const ESP_NOW_PEERS: usize = 0,
    const ESP_NOW_RX: usize = 0,
    const PROMISCUOUS_RX: usize = 0,
    const AP_RX_SLOTS: usize = 0,
    const AP_RX_BYTES: usize = 0,
> {
    pub(crate) backend: RefCell<Option<B>>,
    pub(crate) resources: &'d WifiResources<
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >,
    pub(crate) sta_rx_attached: bool,
    pub(crate) ap_rx_attached: bool,
}

/// Station endpoint borrowed from a [`MixedLink`].
pub struct MixedStaLink<
    'a,
    'd,
    B: WifiBackend,
    const EVENTS: usize,
    const RX: usize,
    const ESP_NOW_PEERS: usize = 0,
    const ESP_NOW_RX: usize = 0,
    const PROMISCUOUS_RX: usize = 0,
    const AP_RX_SLOTS: usize = 0,
    const AP_RX_BYTES: usize = 0,
> {
    backend: &'a RefCell<Option<B>>,
    resources: &'d WifiResources<
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >,
}

/// Access-point endpoint borrowed from a [`MixedLink`].
pub struct MixedApLink<
    'a,
    'd,
    B: WifiBackend,
    const EVENTS: usize,
    const RX: usize,
    const ESP_NOW_PEERS: usize = 0,
    const ESP_NOW_RX: usize = 0,
    const PROMISCUOUS_RX: usize = 0,
    const AP_RX_SLOTS: usize = 0,
    const AP_RX_BYTES: usize = 0,
> {
    backend: &'a RefCell<Option<B>>,
    resources: &'d WifiResources<
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >,
}

#[cfg(feature = "wifi-promiscuous")]
pub struct PromiscuousSession<
    'a,
    B: WifiBackend,
    const EVENTS: usize,
    const RX: usize,
    const ESP_NOW_PEERS: usize = 0,
    const ESP_NOW_RX: usize = 0,
    const PROMISCUOUS_RX: usize = 0,
    const AP_RX_SLOTS: usize = 0,
    const AP_RX_BYTES: usize = 0,
> {
    pub(crate) backend: &'a mut B,
    pub(crate) resources: &'a WifiResources<
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >,
    active: bool,
    filter: PromiscuousFilter,
    control_filter: PromiscuousControlFilter,
}

#[cfg(feature = "wifi-esp-now")]
pub struct EspNowSession<
    'a,
    M,
    B: WifiBackend,
    const EVENTS: usize,
    const RX: usize,
    const ESP_NOW_PEERS: usize = 0,
    const ESP_NOW_RX: usize = 0,
    const PROMISCUOUS_RX: usize = 0,
    const AP_RX_SLOTS: usize = 0,
    const AP_RX_BYTES: usize = 0,
> {
    pub(crate) backend: &'a mut B,
    pub(crate) resources: &'a WifiResources<
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >,
    active: bool,
    cleanup_needed: bool,
    _mode: PhantomData<M>,
}

#[cfg(feature = "wifi-wps")]
pub struct WpsSession<
    'a,
    B: WifiBackend,
    const EVENTS: usize = 0,
    const RX: usize = 0,
    const ESP_NOW_PEERS: usize = 0,
    const ESP_NOW_RX: usize = 0,
    const PROMISCUOUS_RX: usize = 0,
    const AP_RX_SLOTS: usize = 0,
    const AP_RX_BYTES: usize = 0,
> {
    backend: &'a mut B,
    resources: &'a WifiResources<
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >,
    cursor: EventCursor,
    active: bool,
}

#[cfg(feature = "wifi-smartconfig")]
pub struct SmartConfigSession<
    'a,
    B: WifiBackend,
    const EVENTS: usize = 0,
    const RX: usize = 0,
    const ESP_NOW_PEERS: usize = 0,
    const ESP_NOW_RX: usize = 0,
    const PROMISCUOUS_RX: usize = 0,
    const AP_RX_SLOTS: usize = 0,
    const AP_RX_BYTES: usize = 0,
> {
    backend: &'a mut B,
    resources: &'a WifiResources<
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >,
    cursor: EventCursor,
    active: bool,
}

impl<
        'a,
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    >
    StaHandle<
        'a,
        B,
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
{
    pub fn connect(&mut self) -> Result<(), ConnectError> {
        self.backend.connect()
    }

    pub fn connect_blocking(&mut self, timeout_ms: u32) -> Result<(), DriverError> {
        if let Some(result) = self.backend.connect_blocking(timeout_ms) {
            result?;
            return Ok(());
        }

        self.connect().map_err(map_connect_error)?;
        let deadline = Deadline::new(self.backend.now_ms(), timeout_ms);

        loop {
            if self.resources.link_state() == LinkState::Up {
                return Ok(());
            }

            if deadline.is_expired(self.backend.now_ms()) {
                return Err(DriverError::Timeout);
            }

            self.backend.spin_once();
        }
    }

    /// Starts station association and resolves when the SDK reports connected.
    ///
    /// Dropping this future does not cancel the SDK connection attempt or its
    /// auto-retry state. Call [`disconnect`](Self::disconnect) when a timeout or
    /// higher-level cancellation should stop the in-flight association.
    pub async fn connect_async(&mut self) -> Result<(), ConnectError> {
        self.connect()?;
        let resources = self.resources;

        core::future::poll_fn(|cx| {
            resources.register_sta_link_waker(cx.waker());
            if resources.link_state() == LinkState::Up {
                Poll::Ready(Ok(()))
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

impl<B: WifiBackend> ApHandle<'_, B> {
    pub fn connected_stations<const N: usize>(
        &mut self,
    ) -> Result<heapless::Vec<ApStationInfo, N>, DriverError> {
        self.backend.connected_ap_stations()
    }

    pub fn deauth_all(&mut self) -> Result<(), DriverError> {
        self.backend.deauth_all_ap_stations()
    }
}

impl<
        'd,
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    >
    MixedLink<
        'd,
        B,
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
{
    pub fn split(
        &mut self,
    ) -> (
        MixedStaLink<
            '_,
            'd,
            B,
            EVENTS,
            RX,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
        MixedApLink<
            '_,
            'd,
            B,
            EVENTS,
            RX,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
    ) {
        (
            MixedStaLink {
                backend: &self.backend,
                resources: self.resources,
            },
            MixedApLink {
                backend: &self.backend,
                resources: self.resources,
            },
        )
    }

    pub fn into_driver(
        mut self,
    ) -> Result<
        WifiDriver<
            'd,
            Started<super::MixedMode>,
            B,
            EVENTS,
            RX,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
        DriverError,
    > {
        let backend = self.backend.get_mut();
        let backend = backend.as_mut().ok_or(DriverError::BackendRejected)?;
        backend.detach_ap_rx();
        backend.detach_sta_rx();
        self.resources.drain_ap_rx();
        self.resources.drain_sta_rx();
        self.ap_rx_attached = false;
        self.sta_rx_attached = false;
        let backend = self
            .backend
            .get_mut()
            .take()
            .ok_or(DriverError::BackendRejected)?;
        let resources = self.resources;
        core::mem::forget(self);
        Ok(WifiDriver::from_started_parts(resources, backend))
    }
}

impl<
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    > Drop
    for MixedLink<
        '_,
        B,
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
{
    fn drop(&mut self) {
        if let Some(backend) = self.backend.get_mut().as_mut() {
            if self.ap_rx_attached {
                backend.detach_ap_rx();
            }
            if self.sta_rx_attached {
                backend.detach_sta_rx();
            }
        }
    }
}

macro_rules! impl_mixed_endpoint {
    (
        $link:ident,
        $register_link:ident,
        $register_rx:ident,
        $link_state:ident,
        $mac_address:ident,
        $refresh_rx:ident,
        $queued_rx:ident,
        $dequeue_rx:ident,
        $transmit:ident,
        $max_frame_size:expr
    ) => {
        impl<
                'd,
                B: WifiBackend,
                const EVENTS: usize,
                const RX: usize,
                const ESP_NOW_PEERS: usize,
                const ESP_NOW_RX: usize,
                const PROMISCUOUS_RX: usize,
                const AP_RX_SLOTS: usize,
                const AP_RX_BYTES: usize,
            >
            $link<
                '_,
                'd,
                B,
                EVENTS,
                RX,
                ESP_NOW_PEERS,
                ESP_NOW_RX,
                PROMISCUOUS_RX,
                AP_RX_SLOTS,
                AP_RX_BYTES,
            >
        {
            pub fn register_link_waker(&self, waker: &Waker) {
                self.resources.$register_link(waker);
            }

            pub fn register_rx_waker(&self, waker: &Waker) {
                self.resources.$register_rx(waker);
            }

            pub fn link_state(&self) -> LinkState {
                self.resources.$link_state()
            }

            pub fn mac_address(&self) -> Result<[u8; 6], DriverError> {
                let backend = self
                    .backend
                    .try_borrow()
                    .map_err(|_| DriverError::WouldBlock)?;
                backend
                    .as_ref()
                    .ok_or(DriverError::BackendRejected)?
                    .$mac_address()
            }

            pub const fn max_frame_size(&self) -> usize {
                $max_frame_size
            }

            pub fn refresh_rx(&mut self) -> Result<(), DriverError> {
                let mut backend = self
                    .backend
                    .try_borrow_mut()
                    .map_err(|_| DriverError::WouldBlock)?;
                backend
                    .as_mut()
                    .ok_or(DriverError::BackendRejected)?
                    .$refresh_rx()
            }

            pub fn has_rx_data(&self) -> bool {
                self.resources.$queued_rx() != 0
            }

            pub(crate) fn receive_record(&mut self) -> Option<RawRxRecord> {
                self.resources.$dequeue_rx()
            }

            pub fn receive(&mut self, dst: &mut [u8]) -> Result<Option<usize>, DriverError> {
                self.receive_record()
                    .map(|record| record.copy_into(dst))
                    .transpose()
            }

            pub fn transmit(&mut self, frame: &[u8]) -> Result<(), DriverError> {
                super::validate_frame_len(frame.len())?;
                let mut backend = self
                    .backend
                    .try_borrow_mut()
                    .map_err(|_| DriverError::WouldBlock)?;
                backend
                    .as_mut()
                    .ok_or(DriverError::BackendRejected)?
                    .$transmit(frame)
            }
        }

        impl<
                B: WifiBackend,
                const EVENTS: usize,
                const RX: usize,
                const ESP_NOW_PEERS: usize,
                const ESP_NOW_RX: usize,
                const PROMISCUOUS_RX: usize,
                const AP_RX_SLOTS: usize,
                const AP_RX_BYTES: usize,
            > WifiDataLink
            for $link<
                '_,
                '_,
                B,
                EVENTS,
                RX,
                ESP_NOW_PEERS,
                ESP_NOW_RX,
                PROMISCUOUS_RX,
                AP_RX_SLOTS,
                AP_RX_BYTES,
            >
        {
            fn register_link_waker(&self, waker: &Waker) {
                self.register_link_waker(waker);
            }

            fn register_rx_waker(&self, waker: &Waker) {
                self.register_rx_waker(waker);
            }

            fn link_state(&self) -> LinkState {
                self.link_state()
            }

            fn mac_address(&self) -> Result<[u8; 6], DriverError> {
                self.mac_address()
            }

            fn max_frame_size(&self) -> usize {
                self.max_frame_size()
            }

            fn refresh_rx(&mut self) -> Result<(), DriverError> {
                self.refresh_rx()
            }

            fn has_rx_data(&self) -> bool {
                self.has_rx_data()
            }

            fn receive(&mut self, dst: &mut [u8]) -> Result<Option<usize>, DriverError> {
                self.receive(dst)
            }

            fn transmit(&mut self, frame: &[u8]) -> Result<(), DriverError> {
                self.transmit(frame)
            }
        }

        #[cfg(all(feature = "wifi", target_arch = "xtensa"))]
        impl<
                B: WifiBackend,
                const EVENTS: usize,
                const RX: usize,
                const ESP_NOW_PEERS: usize,
                const ESP_NOW_RX: usize,
                const PROMISCUOUS_RX: usize,
                const AP_RX_SLOTS: usize,
                const AP_RX_BYTES: usize,
            > WifiRawDataLink
            for $link<
                '_,
                '_,
                B,
                EVENTS,
                RX,
                ESP_NOW_PEERS,
                ESP_NOW_RX,
                PROMISCUOUS_RX,
                AP_RX_SLOTS,
                AP_RX_BYTES,
            >
        {
            fn receive_record(&mut self) -> Option<RawRxRecord> {
                self.receive_record()
            }
        }
    };
}

impl_mixed_endpoint!(
    MixedStaLink,
    register_sta_link_waker,
    register_rx_waker,
    link_state,
    sta_mac_address,
    refresh_sta_rx,
    queued_rx,
    dequeue_rx,
    transmit,
    super::MAX_ETHERNET_FRAME_SIZE
);

impl_mixed_endpoint!(
    MixedApLink,
    register_ap_link_waker,
    register_ap_rx_waker,
    ap_link_state,
    ap_mac_address,
    refresh_ap_rx,
    queued_ap_rx,
    dequeue_ap_rx,
    transmit_ap,
    if AP_RX_BYTES < super::MAX_ETHERNET_FRAME_SIZE {
        AP_RX_BYTES
    } else {
        super::MAX_ETHERNET_FRAME_SIZE
    }
);

impl<
        'd,
        M: HasSta,
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    >
    StaLink<
        'd,
        M,
        B,
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
{
    fn backend(&self) -> Result<&B, DriverError> {
        self.backend.as_ref().ok_or(DriverError::BackendRejected)
    }

    fn backend_mut(&mut self) -> Result<&mut B, DriverError> {
        self.backend.as_mut().ok_or(DriverError::BackendRejected)
    }

    pub fn register_link_waker(&self, waker: &Waker) {
        self.resources.register_sta_link_waker(waker);
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
        self.backend()?.sta_mac_address()
    }

    pub const fn max_frame_size(&self) -> usize {
        super::MAX_ETHERNET_FRAME_SIZE
    }

    pub fn refresh_sta_rx(&mut self) -> Result<(), DriverError> {
        self.backend_mut()?.refresh_sta_rx()
    }

    pub fn has_rx_data(&self) -> bool {
        self.resources.queued_rx() != 0
    }

    pub(crate) fn receive_record(&mut self) -> Option<RawRxRecord> {
        self.resources.dequeue_rx()
    }

    pub fn receive(&mut self, dst: &mut [u8]) -> Result<Option<usize>, DriverError> {
        self.receive_record()
            .map(|record| record.copy_into(dst))
            .transpose()
    }

    pub fn transmit(&mut self, frame: &[u8]) -> Result<(), DriverError> {
        super::validate_frame_len(frame.len())?;
        self.backend_mut()?.transmit(frame)
    }

    pub fn resources(
        &self,
    ) -> &'d WifiResources<
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    > {
        self.resources
    }

    pub fn into_driver(
        mut self,
    ) -> Result<
        WifiDriver<
            'd,
            Started<M>,
            B,
            EVENTS,
            RX,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
        DriverError,
    > {
        self.backend_mut()?.detach_sta_rx();
        self.resources.drain_sta_rx();
        self.rx_attached = false;
        let resources = self.resources;
        let backend = self.backend.take().ok_or(DriverError::BackendRejected)?;
        core::mem::forget(self);
        Ok(WifiDriver::from_started_parts(resources, backend))
    }
}

impl<
        M,
        B,
        const EVENTS: usize,
        const RX: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    > Drop
    for StaLink<
        '_,
        M,
        B,
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
where
    B: WifiBackend,
{
    fn drop(&mut self) {
        if let Some(backend) = self.backend.as_mut() {
            if self.rx_attached {
                backend.detach_sta_rx();
            }
        }
    }
}

impl<
        'd,
        M: HasAp,
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    >
    ApLink<
        'd,
        M,
        B,
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
{
    fn backend(&self) -> Result<&B, DriverError> {
        self.backend.as_ref().ok_or(DriverError::BackendRejected)
    }

    fn backend_mut(&mut self) -> Result<&mut B, DriverError> {
        self.backend.as_mut().ok_or(DriverError::BackendRejected)
    }

    pub fn register_link_waker(&self, waker: &Waker) {
        self.resources.register_ap_link_waker(waker);
    }

    pub fn register_rx_waker(&self, waker: &Waker) {
        self.resources.register_ap_rx_waker(waker);
    }

    pub fn link_state(&self) -> LinkState {
        self.resources.ap_link_state()
    }

    pub fn mac_address(&self) -> Result<[u8; 6], DriverError> {
        self.backend()?.ap_mac_address()
    }

    pub const fn max_frame_size(&self) -> usize {
        if AP_RX_BYTES < super::MAX_ETHERNET_FRAME_SIZE {
            AP_RX_BYTES
        } else {
            super::MAX_ETHERNET_FRAME_SIZE
        }
    }

    pub fn refresh_ap_rx(&mut self) -> Result<(), DriverError> {
        self.backend_mut()?.refresh_ap_rx()
    }

    pub fn has_rx_data(&self) -> bool {
        self.resources.queued_ap_rx() != 0
    }

    pub(crate) fn receive_record(&mut self) -> Option<RawRxRecord> {
        self.resources.dequeue_ap_rx()
    }

    pub fn receive(&mut self, dst: &mut [u8]) -> Result<Option<usize>, DriverError> {
        self.receive_record()
            .map(|record| record.copy_into(dst))
            .transpose()
    }

    pub fn transmit(&mut self, frame: &[u8]) -> Result<(), DriverError> {
        super::validate_frame_len(frame.len())?;
        self.backend_mut()?.transmit_ap(frame)
    }

    pub fn resources(
        &self,
    ) -> &'d WifiResources<
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    > {
        self.resources
    }

    pub fn into_driver(
        mut self,
    ) -> Result<
        WifiDriver<
            'd,
            Started<M>,
            B,
            EVENTS,
            RX,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
        DriverError,
    > {
        self.backend_mut()?.detach_ap_rx();
        self.resources.drain_ap_rx();
        self.rx_attached = false;
        let resources = self.resources;
        let backend = self.backend.take().ok_or(DriverError::BackendRejected)?;
        core::mem::forget(self);
        Ok(WifiDriver::from_started_parts(resources, backend))
    }
}

impl<
        M,
        B,
        const EVENTS: usize,
        const RX: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    > Drop
    for ApLink<
        '_,
        M,
        B,
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
where
    B: WifiBackend,
{
    fn drop(&mut self) {
        if let Some(backend) = self.backend.as_mut() {
            if self.rx_attached {
                backend.detach_ap_rx();
            }
        }
    }
}

impl<
        M: HasSta,
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    > WifiDataLink
    for StaLink<
        '_,
        M,
        B,
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
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

    fn max_frame_size(&self) -> usize {
        StaLink::max_frame_size(self)
    }

    fn refresh_rx(&mut self) -> Result<(), DriverError> {
        self.refresh_sta_rx()
    }

    fn has_rx_data(&self) -> bool {
        StaLink::has_rx_data(self)
    }

    fn receive(&mut self, dst: &mut [u8]) -> Result<Option<usize>, DriverError> {
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
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    > WifiDataLink
    for ApLink<
        '_,
        M,
        B,
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
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

    fn max_frame_size(&self) -> usize {
        ApLink::max_frame_size(self)
    }

    fn refresh_rx(&mut self) -> Result<(), DriverError> {
        self.refresh_ap_rx()
    }

    fn has_rx_data(&self) -> bool {
        ApLink::has_rx_data(self)
    }

    fn receive(&mut self, dst: &mut [u8]) -> Result<Option<usize>, DriverError> {
        ApLink::receive(self, dst)
    }

    fn transmit(&mut self, frame: &[u8]) -> Result<(), DriverError> {
        ApLink::transmit(self, frame)
    }
}

#[cfg(all(feature = "wifi", target_arch = "xtensa"))]
impl<
        M: HasSta,
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    > WifiRawDataLink
    for StaLink<
        '_,
        M,
        B,
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
{
    fn receive_record(&mut self) -> Option<RawRxRecord> {
        StaLink::receive_record(self)
    }
}

#[cfg(all(feature = "wifi", target_arch = "xtensa"))]
impl<
        M: HasAp,
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    > WifiRawDataLink
    for ApLink<
        '_,
        M,
        B,
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
{
    fn receive_record(&mut self) -> Option<RawRxRecord> {
        ApLink::receive_record(self)
    }
}

#[cfg(feature = "wifi-promiscuous")]
impl<
        'a,
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    >
    PromiscuousSession<
        'a,
        B,
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
{
    pub(crate) fn new(
        backend: &'a mut B,
        resources: &'a WifiResources<
            EVENTS,
            RX,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
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
        if self.active {
            return Err(DriverError::BackendRejected);
        }
        self.resources.reset_promiscuous_runtime();
        for _ in 0..PROMISCUOUS_SETTLE_SPINS {
            self.backend.spin_once();
        }
        self.backend.enable_promiscuous(filter)?;
        self.active = true;
        self.filter = filter;
        if !self.control_filter.is_empty() {
            if let Err(error) = self
                .backend
                .set_promiscuous_control_filter(self.control_filter)
            {
                if self.backend.disable_promiscuous().is_ok() {
                    self.active = false;
                    self.filter = PromiscuousFilter::empty();
                    self.resources.reset_promiscuous_runtime();
                }
                return Err(error);
            }
        }
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
            self.resources.reset_promiscuous_runtime();
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
}

#[cfg(feature = "wifi-promiscuous")]
impl<
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    > Drop
    for PromiscuousSession<
        '_,
        B,
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
{
    fn drop(&mut self) {
        if self.active && self.backend.disable_promiscuous().is_ok() {
            self.resources.reset_promiscuous_runtime();
        }
    }
}

#[cfg(feature = "wifi-esp-now")]
impl<
        'a,
        M,
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    >
    EspNowSession<
        'a,
        M,
        B,
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
{
    pub(crate) fn new(
        backend: &'a mut B,
        resources: &'a WifiResources<
            EVENTS,
            RX,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
    ) -> Self {
        Self {
            backend,
            resources,
            active: false,
            cleanup_needed: false,
            _mode: PhantomData,
        }
    }

    pub fn start(&mut self) -> Result<(), DriverError> {
        if self.active {
            return Ok(());
        }

        if self.cleanup_needed {
            self.backend.disable_esp_now()?;
            self.resources.reset_esp_now_runtime();
            self.cleanup_needed = false;
        }

        self.cleanup_needed = true;
        match self.backend.enable_esp_now() {
            Ok(()) => {
                self.active = true;
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    pub fn stop(&mut self) -> Result<(), DriverError> {
        if self.cleanup_needed {
            self.backend.disable_esp_now()?;
            self.resources.reset_esp_now_runtime();
            self.active = false;
            self.cleanup_needed = false;
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

    fn update_peer_inner(&mut self, peer: &EspNowPeerConfig) -> Result<(), DriverError> {
        self.ensure_active()?;
        if !self.resources.contains_esp_now_peer(peer.peer_addr()) {
            return Err(DriverError::BackendRejected);
        }

        self.backend.modify_esp_now_peer(peer)?;
        self.resources.register_esp_now_peer(peer.peer_record())?;
        Ok(())
    }

    pub fn remove_peer(&mut self, peer_addr: &[u8; 6]) -> Result<bool, DriverError> {
        self.ensure_active()?;
        if !self.resources.contains_esp_now_peer(peer_addr) {
            return Ok(false);
        }
        self.backend.remove_esp_now_peer(peer_addr)?;
        Ok(self.resources.remove_esp_now_peer(peer_addr))
    }

    /// Starts one asynchronous ESP-NOW transmission.
    ///
    /// Only one transmission may be in flight. This returns
    /// [`DriverError::WouldBlock`] until the previous completion has been
    /// consumed with [`send_status`](Self::send_status).
    pub fn send(&mut self, peer_addr: &[u8; 6], payload: &[u8]) -> Result<(), DriverError> {
        self.ensure_active()?;
        if payload.len() > ESP_NOW_MAX_PAYLOAD {
            return Err(DriverError::PayloadTooLong);
        }
        if !self.resources.contains_esp_now_peer(peer_addr) {
            return Err(DriverError::BackendRejected);
        }
        self.resources.reserve_esp_now_tx()?;
        if let Err(error) = self.backend.send_esp_now(peer_addr, payload) {
            self.resources.cancel_esp_now_tx();
            return Err(error);
        }
        Ok(())
    }

    pub fn receive(&mut self) -> Option<EspNowRxFrame> {
        self.resources.dequeue_esp_now_rx()
    }

    /// Takes the completed send status and releases the TX slot.
    ///
    /// `None` means either that no send is active or that its completion is
    /// still pending.
    pub fn send_status(&mut self) -> Option<EspNowSendStatus> {
        self.resources.take_esp_now_tx_status()
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

#[cfg(feature = "wifi-esp-now")]
impl<
        'a,
        M: HasSta,
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    >
    EspNowSession<
        'a,
        M,
        B,
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
{
    pub fn add_station_peer(&mut self, peer: EspNowStationPeerConfig) -> Result<bool, DriverError> {
        self.add_peer_inner(peer.inner())
    }

    pub fn update_station_peer(
        &mut self,
        peer: EspNowStationPeerConfig,
    ) -> Result<(), DriverError> {
        self.update_peer_inner(peer.inner())
    }
}

#[cfg(feature = "wifi-esp-now")]
impl<
        'a,
        M: HasAp,
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    >
    EspNowSession<
        'a,
        M,
        B,
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
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
    ) -> Result<(), DriverError> {
        self.update_peer_inner(peer.inner())
    }
}

#[cfg(feature = "wifi-esp-now")]
impl<
        M,
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    > Drop
    for EspNowSession<
        '_,
        M,
        B,
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
{
    fn drop(&mut self) {
        if self.cleanup_needed && self.backend.disable_esp_now().is_ok() {
            self.resources.reset_esp_now_runtime();
        }
    }
}

#[cfg(feature = "wifi-wps")]
impl<
        'a,
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    >
    WpsSession<
        'a,
        B,
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
{
    pub(crate) fn new(
        backend: &'a mut B,
        resources: &'a WifiResources<
            EVENTS,
            RX,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
    ) -> Result<Self, DriverError> {
        let cursor = resources.claim_event_cursor()?;
        Ok(Self {
            backend,
            resources,
            cursor,
            active: false,
        })
    }

    pub fn start(&mut self, config: &WpsConfig<'_>) -> Result<(), DriverError> {
        if self.active {
            return Err(DriverError::BackendRejected);
        }
        self.resources.clear_event_cursor(&self.cursor);
        self.backend.enable_wps(config)?;
        self.active = true;
        if let Err(error) = self.backend.start_wps() {
            if self.backend.disable_wps().is_ok() {
                self.active = false;
            }
            return Err(error);
        }
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

    pub fn next_event(&mut self) -> Option<WpsEvent> {
        self.resources.events.with(|events| {
            while let Some(event) = events.try_next(&mut self.cursor) {
                let event = match event {
                    WifiEvent::WpsSucceeded => WpsEvent::Succeeded,
                    WifiEvent::WpsFailed { reason } => WpsEvent::Failed { reason },
                    WifiEvent::WpsTimedOut => WpsEvent::TimedOut,
                    WifiEvent::WpsPin { pin } => WpsEvent::Pin { pin },
                    WifiEvent::StaDisconnected { reason } => WpsEvent::Disconnected { reason },
                    _ => continue,
                };
                return Some(event);
            }
            None
        })
    }
}

#[cfg(feature = "wifi-wps")]
impl<
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    > Drop
    for WpsSession<
        '_,
        B,
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
{
    fn drop(&mut self) {
        if self.active {
            let _ = self.backend.disable_wps();
        }
        self.resources.release_event_cursor(&mut self.cursor);
    }
}

#[cfg(feature = "wifi-smartconfig")]
impl<
        'a,
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    >
    SmartConfigSession<
        'a,
        B,
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
{
    pub(crate) fn new(
        backend: &'a mut B,
        resources: &'a WifiResources<
            EVENTS,
            RX,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
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
        if self.active {
            return Err(DriverError::BackendRejected);
        }
        self.resources.clear_event_cursor(&self.cursor);
        self.resources.clear_smartconfig_credentials();
        self.backend.start_smartconfig(config)?;
        self.active = true;
        Ok(())
    }

    pub fn configure_station(&mut self, config: &ClientConfig<'_>) -> Result<(), DriverError> {
        self.backend.configure_sta(config)
    }

    pub fn connect_blocking(&mut self, timeout_ms: u32) -> Result<(), DriverError> {
        if let Some(result) = self.backend.connect_blocking(timeout_ms) {
            result?;
            return Ok(());
        }

        self.backend.connect().map_err(map_connect_error)?;
        let deadline = Deadline::new(self.backend.now_ms(), timeout_ms);

        loop {
            let cursor = &mut self.cursor;
            if let Some(result) = self.resources.events.with(|events| {
                while let Some(event) = events.try_next(cursor) {
                    match event {
                        WifiEvent::StaConnected => {
                            return Some(Ok(()));
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

    /// Sends the UDP acknowledgement expected by ESPTouch senders.
    ///
    /// `source_ip` must be the IPv4 address assigned to this station. The ACK
    /// is sent while SmartConfig remains active so its radio work continues
    /// until all copies have entered the transmit queue.
    pub fn send_esptouch_ack(
        &mut self,
        credentials: &SmartConfigCredentials,
        source_ip: Ipv4Addr,
    ) -> Result<(), DriverError> {
        if !self.active {
            return Err(DriverError::NotStarted);
        }
        if source_ip.is_unspecified() || credentials.cellphone_ip().is_unspecified() {
            return Err(DriverError::BackendRejected);
        }

        let station_mac = self.backend.sta_mac_address()?;
        let bssid = match credentials.bssid() {
            Some(bssid) => bssid,
            None => self.backend.associated_bssid()?,
        };
        let frame = esptouch_ack_frame(credentials, station_mac, bssid, source_ip);

        let mut sent = 0;
        for _ in 0..ESPTOUCH_ACK_ATTEMPTS {
            match self.backend.transmit(&frame) {
                Ok(()) => {
                    sent += 1;
                    if sent == ESPTOUCH_ACK_COPIES {
                        return Ok(());
                    }
                }
                Err(DriverError::WouldBlock) => {}
                Err(error) => return Err(error),
            }
            self.backend.spin_once();
        }

        Err(DriverError::WouldBlock)
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

    pub fn next_event(&mut self) -> Option<SmartConfigEvent> {
        self.resources.next_smartconfig_event(&mut self.cursor)
    }
}

#[cfg(feature = "wifi-smartconfig")]
fn esptouch_ack_frame(
    credentials: &SmartConfigCredentials,
    station_mac: [u8; 6],
    bssid: [u8; 6],
    source_ip: Ipv4Addr,
) -> [u8; ESPTOUCH_ACK_FRAME_LEN] {
    let source_ip = source_ip.octets();
    let destination_ip = credentials.cellphone_ip().octets();
    let mut frame = [0; ESPTOUCH_ACK_FRAME_LEN];

    frame[0..6].copy_from_slice(&bssid);
    frame[6..12].copy_from_slice(&station_mac);
    frame[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());

    let ip = &mut frame[14..34];
    ip[0] = 0x45;
    ip[2..4].copy_from_slice(&(20_u16 + 8 + 11).to_be_bytes());
    ip[4..6].copy_from_slice(&0x1234_u16.to_be_bytes());
    ip[8] = 64;
    ip[9] = 17;
    ip[12..16].copy_from_slice(&source_ip);
    ip[16..20].copy_from_slice(&destination_ip);
    let checksum = ipv4_checksum(ip);
    ip[10..12].copy_from_slice(&checksum.to_be_bytes());

    let udp = &mut frame[34..42];
    udp[0..2].copy_from_slice(&ESPTOUCH_ACK_PORT.to_be_bytes());
    udp[2..4].copy_from_slice(&ESPTOUCH_ACK_PORT.to_be_bytes());
    udp[4..6].copy_from_slice(&(8_u16 + 11).to_be_bytes());

    let payload = &mut frame[42..53];
    payload[0] = credentials.token();
    payload[1..7].copy_from_slice(&station_mac);
    payload[7..11].copy_from_slice(&source_ip);

    frame
}

#[cfg(feature = "wifi-smartconfig")]
fn ipv4_checksum(header: &[u8]) -> u16 {
    let mut sum = 0_u32;
    let mut chunks = header.chunks_exact(2);
    for chunk in chunks.by_ref() {
        sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
    }
    if let Some(byte) = chunks.remainder().first() {
        sum += (*byte as u32) << 8;
    }
    while sum > u16::MAX as u32 {
        sum = (sum & u16::MAX as u32) + (sum >> 16);
    }
    !(sum as u16)
}

#[cfg(feature = "wifi-smartconfig")]
impl<
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    > Drop
    for SmartConfigSession<
        '_,
        B,
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
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
    #[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps"))]
    use core::future::Future;
    extern crate std;
    #[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps"))]
    use std::sync::Arc;
    #[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps"))]
    use std::task::{Context, Wake, Waker};

    use crate::wifi_core::{
        ClientConfig, InitConfig, LinkState, RawMode, RawRxRecord, Ssid, StationCredential,
        WifiApBackend, WifiDriver, WifiEvent, WifiFeatureBackend, WifiLifecycleBackend,
        WifiPacketBackend, WifiResources, WifiRuntimeBackend, WifiScanBackend, WifiStaBackend,
    };
    #[cfg(feature = "wifi-smartconfig")]
    use crate::wifi_core::{Ipv4Addr, SmartConfigCredentials, SmartConfigType};

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
        fn configure_sta(
            &mut self,
            _config: &ClientConfig<'_>,
        ) -> Result<(), crate::wifi_core::DriverError> {
            Ok(())
        }

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
        let resources = WifiResources::<8, 4>::new();
        let driver = WifiDriver::with_backend(&resources, FakeBackend::default());
        let driver = driver.initialize(InitConfig::default()).unwrap();
        let mut driver = driver
            .configure_sta(ClientConfig::new(
                Ssid::try_from("core-net").unwrap(),
                StationCredential::open(),
            ))
            .unwrap()
            .start()
            .unwrap();

        let mut sta = driver.sta();
        sta.connect().unwrap();
    }

    #[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps"))]
    struct NoopWake;

    #[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps"))]
    impl Wake for NoopWake {
        fn wake(self: Arc<Self>) {}
    }

    #[test]
    #[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps"))]
    fn async_connect_uses_link_mirror_when_the_event_ring_overflows() {
        let resources = WifiResources::<1, 1>::new();
        let mut cursor = resources.claim_event_cursor().unwrap();
        resources
            .publish_event(WifiEvent::StaDisconnected { reason: Some(1) })
            .unwrap();
        assert_eq!(
            resources.publish_event(WifiEvent::StaConnected),
            Err(crate::wifi_core::DriverError::EventOverflow)
        );

        let mut driver = WifiDriver::with_backend(&resources, FakeBackend::default())
            .initialize(InitConfig::default())
            .unwrap()
            .configure_sta(ClientConfig::new(
                Ssid::try_from("core-net").unwrap(),
                StationCredential::open(),
            ))
            .unwrap()
            .start()
            .unwrap();
        let mut sta = driver.sta();
        let mut future = core::pin::pin!(sta.connect_async());
        let waker = Waker::from(Arc::new(NoopWake));
        let mut context = Context::from_waker(&waker);

        assert!(matches!(
            future.as_mut().poll(&mut context),
            core::task::Poll::Ready(Ok(()))
        ));
        resources.release_event_cursor(&mut cursor);
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
        fn configure_sta(
            &mut self,
            _config: &ClientConfig<'_>,
        ) -> Result<(), crate::wifi_core::DriverError> {
            Ok(())
        }

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
        fn attach_sta_rx(&mut self) -> Result<(), crate::wifi_core::DriverError> {
            Ok(())
        }

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
        static RX_BYTES: [u8; 24] = [0; 24];
        let resources = WifiResources::<8, 4>::new();
        resources.publish_event(WifiEvent::StaConnected).unwrap();
        resources
            .enqueue_rx(unsafe {
                RawRxRecord::from_raw(
                    RX_BYTES.as_ptr().cast_mut().cast(),
                    RX_BYTES.len() as u16,
                    core::ptr::null_mut(),
                    None,
                )
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
                StationCredential::open(),
            ))
            .unwrap()
            .start()
            .unwrap();

        let mut link = driver.sta_link().unwrap();
        let mut rx_buf = [0u8; 32];

        assert_eq!(link.link_state(), LinkState::Up);
        assert_eq!(link.mac_address().unwrap(), [1, 2, 3, 4, 5, 6]);
        assert_eq!(link.receive(&mut rx_buf), Ok(Some(24)));
        link.transmit(&[9, 8, 7]).unwrap();
    }

    #[cfg(feature = "wifi-smartconfig")]
    #[test]
    fn esptouch_ack_frame_has_valid_ipv4_udp_layout() {
        let credentials = SmartConfigCredentials::new(
            b"network",
            b"password",
            Some([0x10, 0x20, 0x30, 0x40, 0x50, 0x60]),
            SmartConfigType::EspTouch,
            0x42,
            Ipv4Addr::new(192, 168, 4, 3),
        )
        .unwrap();
        let station_mac = [1, 2, 3, 4, 5, 6];
        let source_ip = Ipv4Addr::new(192, 168, 4, 2);

        let frame = super::esptouch_ack_frame(
            &credentials,
            station_mac,
            credentials.bssid().unwrap(),
            source_ip,
        );

        assert_eq!(&frame[0..6], credentials.bssid().unwrap().as_slice());
        assert_eq!(&frame[6..12], &station_mac);
        assert_eq!(&frame[12..14], &0x0800_u16.to_be_bytes());
        assert_eq!(super::ipv4_checksum(&frame[14..34]), 0);
        assert_eq!(&frame[34..36], &super::ESPTOUCH_ACK_PORT.to_be_bytes());
        assert_eq!(&frame[36..38], &super::ESPTOUCH_ACK_PORT.to_be_bytes());
        assert_eq!(frame[42], credentials.token());
        assert_eq!(&frame[43..49], &station_mac);
        assert_eq!(&frame[49..53], &source_ip.octets());
    }
}
