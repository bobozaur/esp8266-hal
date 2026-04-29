use core::marker::PhantomData;

use super::{
    interface::{
        ApHandle, ApLink, EspNowSession, PromiscuousSession, SmartConfigSession, StaHandle,
        StaLink, WpsSession,
    },
    AccessPointConfig, AccessPointRecord, ApMode, ClientConfig, Configured, DriverError, HasAp,
    HasSta, InitConfig, InitError, Initialized, MixedConfig, MixedMode, RawMode, ScanOptions,
    StaMode, Started, Uninitialized, WifiBackend, WifiResources,
};

pub struct WifiDriver<
    'd,
    S,
    B,
    const EVENTS: usize,
    const RX: usize,
    const SUBS: usize,
    const ESP_NOW_PEERS: usize = 0,
    const ESP_NOW_RX: usize = 0,
    const PROMISCUOUS_RX: usize = 0,
    const AP_RX_SLOTS: usize = 0,
    const AP_RX_BYTES: usize = 0,
> {
    backend: B,
    resources: &'d WifiResources<
        EVENTS,
        RX,
        SUBS,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >,
    _state: PhantomData<S>,
}

impl<
        'd,
        B,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    >
    WifiDriver<
        'd,
        Uninitialized,
        B,
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
    pub fn with_backend(
        resources: &'d WifiResources<
            EVENTS,
            RX,
            SUBS,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
        backend: B,
    ) -> Self {
        Self {
            backend,
            resources,
            _state: PhantomData,
        }
    }
}

impl<
        'd,
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    >
    WifiDriver<
        'd,
        Uninitialized,
        B,
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
    pub fn initialize(
        mut self,
        config: InitConfig,
    ) -> Result<
        WifiDriver<
            'd,
            Initialized,
            B,
            EVENTS,
            RX,
            SUBS,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
        InitError,
    > {
        self.backend.initialize(&config)?;
        Ok(WifiDriver {
            backend: self.backend,
            resources: self.resources,
            _state: PhantomData,
        })
    }
}

impl<
        'd,
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    >
    WifiDriver<
        'd,
        Initialized,
        B,
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
    pub fn configure_sta(
        mut self,
        config: ClientConfig,
    ) -> Result<
        WifiDriver<
            'd,
            Configured<StaMode>,
            B,
            EVENTS,
            RX,
            SUBS,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
        DriverError,
    > {
        self.backend.configure_sta(&config)?;
        Ok(WifiDriver {
            backend: self.backend,
            resources: self.resources,
            _state: PhantomData,
        })
    }

    pub fn enable_sta(
        mut self,
    ) -> Result<
        WifiDriver<
            'd,
            Configured<StaMode>,
            B,
            EVENTS,
            RX,
            SUBS,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
        DriverError,
    > {
        self.backend.set_mode(RawMode::Sta)?;
        Ok(WifiDriver {
            backend: self.backend,
            resources: self.resources,
            _state: PhantomData,
        })
    }

    pub fn configure_ap(
        mut self,
        config: AccessPointConfig,
    ) -> Result<
        WifiDriver<
            'd,
            Configured<ApMode>,
            B,
            EVENTS,
            RX,
            SUBS,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
        DriverError,
    > {
        self.backend.configure_ap(&config)?;
        Ok(WifiDriver {
            backend: self.backend,
            resources: self.resources,
            _state: PhantomData,
        })
    }

    pub fn configure_mixed(
        mut self,
        config: MixedConfig,
    ) -> Result<
        WifiDriver<
            'd,
            Configured<MixedMode>,
            B,
            EVENTS,
            RX,
            SUBS,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
        DriverError,
    > {
        self.backend
            .configure_mixed(config.client(), config.access_point())?;
        Ok(WifiDriver {
            backend: self.backend,
            resources: self.resources,
            _state: PhantomData,
        })
    }
}

impl<
        'd,
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    >
    WifiDriver<
        'd,
        Configured<StaMode>,
        B,
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
    pub fn start(
        mut self,
    ) -> Result<
        WifiDriver<
            'd,
            Started<StaMode>,
            B,
            EVENTS,
            RX,
            SUBS,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
        DriverError,
    > {
        self.backend.start()?;
        Ok(WifiDriver {
            backend: self.backend,
            resources: self.resources,
            _state: PhantomData,
        })
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
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    >
    WifiDriver<
        'd,
        Started<M>,
        B,
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
    pub fn sta(
        &mut self,
    ) -> Result<
        StaHandle<
            '_,
            B,
            EVENTS,
            RX,
            SUBS,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
        DriverError,
    > {
        let cursor = self.resources.claim_event_cursor()?;
        Ok(StaHandle {
            backend: &mut self.backend,
            resources: self.resources,
            cursor,
        })
    }

    pub fn resources(
        &self,
    ) -> &WifiResources<
        EVENTS,
        RX,
        SUBS,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    > {
        self.resources
    }

    pub fn scan_blocking<const N: usize>(
        &mut self,
        options: &ScanOptions,
    ) -> Result<heapless::Vec<AccessPointRecord, N>, DriverError> {
        self.backend.scan(options)
    }

    pub fn scan_blocking_into<const N: usize>(
        &mut self,
        options: &ScanOptions,
        records: &mut heapless::Vec<AccessPointRecord, N>,
    ) -> Result<(), DriverError> {
        self.backend.scan_into(options, records)
    }

    pub fn sta_link(
        mut self,
    ) -> Result<
        StaLink<
            'd,
            M,
            B,
            EVENTS,
            RX,
            SUBS,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
        DriverError,
    > {
        self.backend.attach_sta_rx()?;
        Ok(StaLink {
            backend: Some(self.backend),
            resources: self.resources,
            rx_attached: true,
            _mode: PhantomData,
        })
    }

    pub fn esp_now(
        &mut self,
    ) -> Result<
        EspNowSession<
            '_,
            M,
            B,
            EVENTS,
            RX,
            SUBS,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
        DriverError,
    > {
        if self.resources.esp_now_peer_capacity() == 0 || self.resources.esp_now_rx_capacity() == 0
        {
            return Err(DriverError::PacketOverflow);
        }

        Ok(EspNowSession::new(&mut self.backend, self.resources))
    }

    pub fn wps(
        &mut self,
    ) -> Result<
        WpsSession<
            '_,
            B,
            EVENTS,
            RX,
            SUBS,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
        DriverError,
    > {
        WpsSession::new(&mut self.backend, self.resources)
    }

    pub fn smart_config(
        &mut self,
    ) -> Result<
        SmartConfigSession<
            '_,
            B,
            EVENTS,
            RX,
            SUBS,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
        DriverError,
    > {
        SmartConfigSession::new(&mut self.backend, self.resources)
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
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    >
    WifiDriver<
        'd,
        Started<M>,
        B,
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
    pub fn ap(
        &mut self,
    ) -> Result<
        ApHandle<
            '_,
            B,
            EVENTS,
            RX,
            SUBS,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
        DriverError,
    > {
        let cursor = self.resources.claim_event_cursor()?;
        Ok(ApHandle {
            backend: &mut self.backend,
            resources: self.resources,
            cursor,
        })
    }

    pub fn ap_link(
        mut self,
    ) -> Result<
        ApLink<
            'd,
            M,
            B,
            EVENTS,
            RX,
            SUBS,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
        DriverError,
    > {
        self.backend.attach_ap_rx()?;
        Ok(ApLink {
            backend: Some(self.backend),
            resources: self.resources,
            rx_attached: true,
            _mode: PhantomData,
        })
    }
}

impl<
        'd,
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    >
    WifiDriver<
        'd,
        Configured<ApMode>,
        B,
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
    pub fn start(
        mut self,
    ) -> Result<
        WifiDriver<
            'd,
            Started<ApMode>,
            B,
            EVENTS,
            RX,
            SUBS,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
        DriverError,
    > {
        self.backend.start()?;
        Ok(WifiDriver {
            backend: self.backend,
            resources: self.resources,
            _state: PhantomData,
        })
    }
}

impl<
        'd,
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    >
    WifiDriver<
        'd,
        Configured<MixedMode>,
        B,
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
    pub fn start(
        mut self,
    ) -> Result<
        WifiDriver<
            'd,
            Started<MixedMode>,
            B,
            EVENTS,
            RX,
            SUBS,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
        DriverError,
    > {
        self.backend.start()?;
        Ok(WifiDriver {
            backend: self.backend,
            resources: self.resources,
            _state: PhantomData,
        })
    }
}

impl<
        'd,
        M,
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    >
    WifiDriver<
        'd,
        Started<M>,
        B,
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
    pub(crate) fn from_started_parts(
        resources: &'d WifiResources<
            EVENTS,
            RX,
            SUBS,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
        backend: B,
    ) -> Self {
        Self {
            backend,
            resources,
            _state: PhantomData,
        }
    }

    pub fn promiscuous(
        &mut self,
    ) -> Result<
        PromiscuousSession<
            '_,
            B,
            EVENTS,
            RX,
            SUBS,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
        DriverError,
    > {
        if self.resources.promiscuous_rx_capacity() == 0 {
            return Err(DriverError::PacketOverflow);
        }

        Ok(PromiscuousSession::new(&mut self.backend, self.resources))
    }

    pub fn stop(
        mut self,
    ) -> Result<
        WifiDriver<
            'd,
            Initialized,
            B,
            EVENTS,
            RX,
            SUBS,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
        DriverError,
    > {
        self.backend.stop()?;
        Ok(WifiDriver {
            backend: self.backend,
            resources: self.resources,
            _state: PhantomData,
        })
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use std::{cell::RefCell, rc::Rc, vec::Vec};

    use crate::wifi_core::{
        AccessPointConfig, Channel, ClientConfig, Credential, InitConfig, MixedConfig, RawMode,
        Ssid, WifiApBackend, WifiFeatureBackend, WifiLifecycleBackend, WifiPacketBackend,
        WifiResources, WifiRuntimeBackend, WifiScanBackend, WifiStaBackend,
    };

    #[derive(Default)]
    struct FakeBackend {
        started: bool,
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

        fn set_mode(
            &mut self,
            _mode: crate::wifi_core::RawMode,
        ) -> Result<(), crate::wifi_core::DriverError> {
            Ok(())
        }

        fn start(&mut self) -> Result<(), crate::wifi_core::DriverError> {
            self.started = true;
            Ok(())
        }

        fn stop(&mut self) -> Result<(), crate::wifi_core::DriverError> {
            self.started = false;
            Ok(())
        }
    }

    impl WifiStaBackend for FakeBackend {
        fn connect(&mut self) -> Result<(), crate::wifi_core::ConnectError> {
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
    fn lifecycle_moves_between_explicit_driver_states() {
        let resources = WifiResources::<8, 4, 2>::new();
        let driver = WifiDriver::with_backend(&resources, FakeBackend::default());
        let driver = driver.initialize(InitConfig::default()).unwrap();
        let driver = driver
            .configure_sta(ClientConfig::new(
                Ssid::try_from("core-net").unwrap(),
                Credential::Open,
            ))
            .unwrap();
        let driver = driver.start().unwrap();
        let _driver = driver.stop().unwrap();
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Op {
        Initialize,
        ConfigureSta,
        ConfigureAp,
        ConfigureMixed,
        SetMode(RawMode),
        Start,
        AttachStaRx,
        DetachStaRx,
        AttachApRx,
        DetachApRx,
    }

    #[derive(Clone)]
    struct TrackingBackend {
        ops: Rc<RefCell<Vec<Op>>>,
    }

    impl TrackingBackend {
        fn new(ops: Rc<RefCell<Vec<Op>>>) -> Self {
            Self { ops }
        }

        fn push(&self, op: Op) {
            self.ops.borrow_mut().push(op);
        }
    }

    impl WifiRuntimeBackend for TrackingBackend {
        fn now_ms(&self) -> u64 {
            0
        }

        fn spin_once(&mut self) {}
    }

    impl WifiLifecycleBackend for TrackingBackend {
        fn initialize(&mut self, _config: &InitConfig) -> Result<(), crate::wifi_core::InitError> {
            self.push(Op::Initialize);
            Ok(())
        }

        fn set_mode(
            &mut self,
            mode: crate::wifi_core::RawMode,
        ) -> Result<(), crate::wifi_core::DriverError> {
            self.push(Op::SetMode(mode));
            Ok(())
        }

        fn start(&mut self) -> Result<(), crate::wifi_core::DriverError> {
            self.push(Op::Start);
            Ok(())
        }

        fn stop(&mut self) -> Result<(), crate::wifi_core::DriverError> {
            Ok(())
        }
    }

    impl WifiStaBackend for TrackingBackend {
        fn configure_sta(
            &mut self,
            _config: &ClientConfig,
        ) -> Result<(), crate::wifi_core::DriverError> {
            self.push(Op::ConfigureSta);
            Ok(())
        }

        fn connect(&mut self) -> Result<(), crate::wifi_core::ConnectError> {
            Ok(())
        }

        fn disconnect(&mut self) -> Result<(), crate::wifi_core::DisconnectError> {
            Ok(())
        }
    }

    impl WifiApBackend for TrackingBackend {
        fn configure_ap(
            &mut self,
            _config: &AccessPointConfig,
        ) -> Result<(), crate::wifi_core::DriverError> {
            self.push(Op::ConfigureAp);
            Ok(())
        }

        fn configure_mixed(
            &mut self,
            _client: &ClientConfig,
            _access_point: &AccessPointConfig,
        ) -> Result<(), crate::wifi_core::DriverError> {
            self.push(Op::ConfigureMixed);
            Ok(())
        }
    }

    impl WifiPacketBackend for TrackingBackend {
        fn attach_sta_rx(&mut self) -> Result<(), crate::wifi_core::DriverError> {
            self.push(Op::AttachStaRx);
            Ok(())
        }

        fn detach_sta_rx(&mut self) {
            self.push(Op::DetachStaRx);
        }

        fn attach_ap_rx(&mut self) -> Result<(), crate::wifi_core::DriverError> {
            self.push(Op::AttachApRx);
            Ok(())
        }

        fn detach_ap_rx(&mut self) {
            self.push(Op::DetachApRx);
        }
    }

    impl WifiScanBackend for TrackingBackend {}
    impl WifiFeatureBackend for TrackingBackend {}

    #[test]
    fn sta_start_does_not_reissue_mode_after_configuration() {
        let ops = Rc::new(RefCell::new(Vec::new()));
        let resources = WifiResources::<8, 4, 2>::new();
        let driver = WifiDriver::with_backend(&resources, TrackingBackend::new(ops.clone()));
        let driver = driver.initialize(InitConfig::default()).unwrap();
        let driver = driver
            .configure_sta(ClientConfig::new(
                Ssid::try_from("core-net").unwrap(),
                Credential::Open,
            ))
            .unwrap();

        let _driver = driver.start().unwrap();

        assert_eq!(
            &*ops.borrow(),
            &[Op::Initialize, Op::ConfigureSta, Op::Start]
        );
    }

    #[test]
    fn ap_start_does_not_reissue_mode_after_configuration() {
        let ops = Rc::new(RefCell::new(Vec::new()));
        let resources = WifiResources::<8, 4, 2>::new();
        let driver = WifiDriver::with_backend(&resources, TrackingBackend::new(ops.clone()));
        let driver = driver.initialize(InitConfig::default()).unwrap();
        let driver = driver
            .configure_ap(AccessPointConfig::open(
                Ssid::try_from("core-ap").unwrap(),
                Channel::try_from(6).unwrap(),
            ))
            .unwrap();

        let _driver = driver.start().unwrap();

        assert_eq!(
            &*ops.borrow(),
            &[Op::Initialize, Op::ConfigureAp, Op::Start]
        );
    }

    #[test]
    fn mixed_start_does_not_reissue_mode_after_configuration() {
        let ops = Rc::new(RefCell::new(Vec::new()));
        let resources = WifiResources::<8, 4, 2>::new();
        let driver = WifiDriver::with_backend(&resources, TrackingBackend::new(ops.clone()));
        let driver = driver.initialize(InitConfig::default()).unwrap();
        let driver = driver
            .configure_mixed(MixedConfig::new(
                ClientConfig::new(Ssid::try_from("core-net").unwrap(), Credential::Open),
                AccessPointConfig::open(
                    Ssid::try_from("core-ap").unwrap(),
                    Channel::try_from(6).unwrap(),
                ),
            ))
            .unwrap();

        let _driver = driver.start().unwrap();

        assert_eq!(
            &*ops.borrow(),
            &[Op::Initialize, Op::ConfigureMixed, Op::Start]
        );
    }

    #[test]
    fn sta_link_attaches_rx_on_typestate_transfer() {
        let ops = Rc::new(RefCell::new(Vec::new()));
        let resources = WifiResources::<8, 4, 2>::new();
        let driver = WifiDriver::with_backend(&resources, TrackingBackend::new(ops.clone()));
        let driver = driver.initialize(InitConfig::default()).unwrap();
        let driver = driver
            .configure_sta(ClientConfig::new(
                Ssid::try_from("core-net").unwrap(),
                Credential::Open,
            ))
            .unwrap()
            .start()
            .unwrap();

        let _link = driver.sta_link().unwrap();

        assert_eq!(
            &*ops.borrow(),
            &[Op::Initialize, Op::ConfigureSta, Op::Start, Op::AttachStaRx]
        );
    }

    #[test]
    fn link_into_driver_detaches_rx_before_returning_started_driver() {
        let ops = Rc::new(RefCell::new(Vec::new()));
        let resources = WifiResources::<8, 4, 2>::new();
        let driver = WifiDriver::with_backend(&resources, TrackingBackend::new(ops.clone()));
        let driver = driver.initialize(InitConfig::default()).unwrap();
        let driver = driver
            .configure_sta(ClientConfig::new(
                Ssid::try_from("core-net").unwrap(),
                Credential::Open,
            ))
            .unwrap()
            .start()
            .unwrap();

        let link = driver.sta_link().unwrap();
        let _driver = link.into_driver().unwrap();

        assert_eq!(
            &*ops.borrow(),
            &[
                Op::Initialize,
                Op::ConfigureSta,
                Op::Start,
                Op::AttachStaRx,
                Op::DetachStaRx,
            ]
        );
    }

    #[test]
    fn dropped_sta_link_detaches_rx_callback() {
        let ops = Rc::new(RefCell::new(Vec::new()));
        let resources = WifiResources::<8, 4, 2>::new();
        let driver = WifiDriver::with_backend(&resources, TrackingBackend::new(ops.clone()));
        let driver = driver.initialize(InitConfig::default()).unwrap();
        let driver = driver
            .configure_sta(ClientConfig::new(
                Ssid::try_from("core-net").unwrap(),
                Credential::Open,
            ))
            .unwrap()
            .start()
            .unwrap();

        {
            let _link = driver.sta_link().unwrap();
        }

        assert_eq!(
            &*ops.borrow(),
            &[
                Op::Initialize,
                Op::ConfigureSta,
                Op::Start,
                Op::AttachStaRx,
                Op::DetachStaRx,
            ]
        );
    }

    #[test]
    fn dropped_ap_link_detaches_rx_callback() {
        let ops = Rc::new(RefCell::new(Vec::new()));
        let resources = WifiResources::<8, 4, 2>::new();
        let driver = WifiDriver::with_backend(&resources, TrackingBackend::new(ops.clone()));
        let driver = driver.initialize(InitConfig::default()).unwrap();
        let driver = driver
            .configure_ap(AccessPointConfig::open(
                Ssid::try_from("core-ap").unwrap(),
                Channel::try_from(6).unwrap(),
            ))
            .unwrap()
            .start()
            .unwrap();

        {
            let _link = driver.ap_link().unwrap();
        }

        assert_eq!(
            &*ops.borrow(),
            &[
                Op::Initialize,
                Op::ConfigureAp,
                Op::Start,
                Op::AttachApRx,
                Op::DetachApRx,
            ]
        );
    }
}
