//! Concrete SDK double for exercising production control/session logic on host.
use super::{resources::Resources, *};
pub(crate) struct Sdk {
    resources: *const Resources,
    pub(crate) started: bool,
    pub(crate) mode: Option<RawMode>,
    pub(crate) session_claimed: bool,
    pub(crate) commands: usize,
    pub(crate) fail_next: Option<DriverError>,
    pub(crate) now: u64,
    #[cfg(feature = "wifi-esp-now")]
    pub(crate) esp_now_initialized: bool,
    #[cfg(feature = "wifi-promiscuous")]
    pub(crate) promiscuous_enabled: bool,
    #[cfg(feature = "wifi-wps")]
    pub(crate) wps_enabled: bool,
    #[cfg(feature = "wifi-smartconfig")]
    pub(crate) smartconfig_active: bool,
}
impl Sdk {
    pub(crate) fn new(resources: &Resources) -> Self {
        Self {
            resources,
            started: false,
            mode: None,
            session_claimed: false,
            commands: 0,
            fail_next: None,
            now: 0,
            #[cfg(feature = "wifi-esp-now")]
            esp_now_initialized: false,
            #[cfg(feature = "wifi-promiscuous")]
            promiscuous_enabled: false,
            #[cfg(feature = "wifi-wps")]
            wps_enabled: false,
            #[cfg(feature = "wifi-smartconfig")]
            smartconfig_active: false,
        }
    }
    fn command(&mut self) -> Result<(), DriverError> {
        self.commands += 1;
        self.fail_next.take().map_or(Ok(()), Err)
    }
    pub(crate) fn set_mode(&mut self, mode: RawMode) -> Result<(), DriverError> {
        self.command()?;
        self.mode = Some(mode);
        Ok(())
    }
    pub(crate) fn configure_sta(&mut self, _: &ClientConfig<'_>) -> Result<(), DriverError> {
        self.set_mode(RawMode::Sta)
    }
    pub(crate) fn configure_ap(&mut self, _: &AccessPointConfig) -> Result<(), DriverError> {
        self.set_mode(RawMode::Ap)
    }
    pub(crate) fn configure_mixed(
        &mut self,
        _: &ClientConfig<'_>,
        _: &AccessPointConfig,
    ) -> Result<(), DriverError> {
        self.set_mode(RawMode::ApSta)
    }
    pub(crate) fn start(&mut self) -> Result<(), DriverError> {
        self.command()?;
        self.started = true;
        let r = unsafe { &*self.resources };
        if self.mode.is_some_and(RawMode::has_station) {
            r.publish_event(WifiEvent::StaStarted)?
        }
        if self.mode.is_some_and(RawMode::has_ap) {
            r.publish_event(WifiEvent::ApStarted)?
        }
        Ok(())
    }
    pub(crate) fn stop(&mut self) -> Result<(), DriverError> {
        self.command()?;
        self.started = false;
        unsafe { &*self.resources }.reset_runtime();
        Ok(())
    }
    pub(crate) fn shutdown(&mut self) -> bool {
        self.stop().is_ok()
    }
    pub(crate) fn attach_sta_rx(&mut self) -> Result<(), DriverError> {
        self.command()
    }
    pub(crate) fn attach_ap_rx(&mut self) -> Result<(), DriverError> {
        self.command()
    }
    pub(crate) fn connect(&mut self) -> Result<(), DriverError> {
        self.command()
    }
    pub(crate) fn disconnect(&mut self) -> Result<(), DriverError> {
        self.command()
    }
    pub(crate) fn now_ms(&self) -> u64 {
        self.now
    }
    pub(crate) fn spin_once(&mut self) {
        self.now += 10;
    }
    pub(crate) fn connected_ap_stations(
        &mut self,
        _: &mut [ApStationInfo],
    ) -> Result<usize, DriverError> {
        self.command()?;
        Ok(0)
    }
    pub(crate) fn deauth_all_ap_stations(&mut self) -> Result<(), DriverError> {
        self.command()
    }
    #[cfg(any(feature = "wifi-wps", feature = "wifi-smartconfig"))]
    pub(crate) fn synchronize_events(&mut self) -> Result<(), DriverError> {
        self.command()
    }
    pub(crate) fn begin_scan(&mut self, _: &ScanOptions) -> Result<(), DriverError> {
        self.command()
    }
    pub(crate) fn cancel_scan(&mut self) -> Result<(), DriverError> {
        self.command()
    }
    pub(crate) fn collect_scan(
        &mut self,
        _: &mut [AccessPointRecord],
    ) -> Result<usize, DriverError> {
        self.command()?;
        Ok(0)
    }
    pub(crate) fn discard_scan(&mut self) {
        self.commands += 1;
    }
    #[cfg(any(feature = "wifi-esp-now", feature = "wifi-promiscuous"))]
    pub(crate) fn set_channel(&mut self, _: Channel) -> Result<(), DriverError> {
        self.command()
    }
    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn enable_esp_now(&mut self) -> Result<(), DriverError> {
        self.command()?;
        self.esp_now_initialized = true;
        Ok(())
    }
    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn disable_esp_now(&mut self) -> Result<(), DriverError> {
        self.command()?;
        self.esp_now_initialized = false;
        Ok(())
    }
    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn set_esp_now_pmk(&mut self, _: &[u8; 16]) -> Result<(), DriverError> {
        self.command()
    }
    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn add_esp_now_peer(&mut self, _: &EspNowPeerConfig) -> Result<(), DriverError> {
        self.command()
    }
    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn modify_esp_now_peer(&mut self, _: &EspNowPeerConfig) -> Result<(), DriverError> {
        self.command()
    }
    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn remove_esp_now_peer(&mut self, _: &[u8; 6]) -> Result<(), DriverError> {
        self.command()
    }
    #[cfg(feature = "wifi-esp-now")]
    pub(crate) fn send_esp_now(&mut self, _: &[u8; 6], _: &[u8]) -> Result<(), DriverError> {
        self.command()
    }
    #[cfg(feature = "wifi-promiscuous")]
    pub(crate) fn enable_promiscuous(&mut self, _: PromiscuousFilter) -> Result<(), DriverError> {
        self.command()?;
        self.promiscuous_enabled = true;
        Ok(())
    }
    #[cfg(feature = "wifi-promiscuous")]
    pub(crate) fn disable_promiscuous(&mut self) -> Result<(), DriverError> {
        self.command()?;
        self.promiscuous_enabled = false;
        Ok(())
    }
    #[cfg(feature = "wifi-promiscuous")]
    pub(crate) fn set_promiscuous_control_filter(
        &mut self,
        _: PromiscuousControlFilter,
    ) -> Result<(), DriverError> {
        self.command()
    }
    #[cfg(feature = "wifi-wps")]
    pub(crate) fn enable_wps(&mut self, _: &WpsConfig<'_>) -> Result<(), DriverError> {
        self.command()?;
        self.wps_enabled = true;
        Ok(())
    }
    #[cfg(feature = "wifi-wps")]
    pub(crate) fn start_wps(&mut self) -> Result<(), DriverError> {
        self.command()
    }
    #[cfg(feature = "wifi-wps")]
    pub(crate) fn disable_wps(&mut self) -> Result<(), DriverError> {
        self.command()?;
        self.wps_enabled = false;
        Ok(())
    }
    #[cfg(feature = "wifi-smartconfig")]
    pub(crate) fn set_smartconfig_type(&mut self, _: SmartConfigType) -> Result<(), DriverError> {
        self.command()
    }
    #[cfg(feature = "wifi-smartconfig")]
    pub(crate) fn set_smartconfig_timeout(
        &mut self,
        _: SmartConfigTimeout,
    ) -> Result<(), DriverError> {
        self.command()
    }
    #[cfg(feature = "wifi-smartconfig")]
    pub(crate) fn set_smartconfig_fast_mode(&mut self, _: bool) -> Result<(), DriverError> {
        self.command()
    }
    #[cfg(feature = "wifi-smartconfig")]
    pub(crate) fn start_smartconfig(
        &mut self,
        _: &SmartConfigStartConfig,
    ) -> Result<(), DriverError> {
        self.command()?;
        self.smartconfig_active = true;
        Ok(())
    }
    #[cfg(feature = "wifi-smartconfig")]
    pub(crate) fn stop_smartconfig(&mut self) -> Result<(), DriverError> {
        self.command()?;
        self.smartconfig_active = false;
        Ok(())
    }
    #[cfg(feature = "wifi-smartconfig")]
    pub(crate) fn read_smartconfig_reserved_data(
        &mut self,
        _: &mut [u8],
    ) -> Result<(), DriverError> {
        self.command()
    }
}
