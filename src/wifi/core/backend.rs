use super::{
    AccessPointConfig, AccessPointRecord, ApStationInfo, Channel, ClientConfig, ConnectError,
    DisconnectError, DriverError, EspNowPeerConfig, InitConfig, InitError,
    PromiscuousControlFilter, PromiscuousFilter, ScanOptions, SmartConfigStartConfig,
    SmartConfigTimeout, SmartConfigType, WpsConfig,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RawMode {
    Sta,
    Ap,
    ApSta,
}

impl RawMode {
    pub const fn for_sta() -> Self {
        Self::Sta
    }

    pub const fn for_ap() -> Self {
        Self::Ap
    }

    pub const fn for_mixed() -> Self {
        Self::ApSta
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Deadline {
    start_ms: u64,
    timeout_ms: u32,
}

impl Deadline {
    pub const fn new(start_ms: u64, timeout_ms: u32) -> Self {
        Self {
            start_ms,
            timeout_ms,
        }
    }

    pub fn is_expired(self, now_ms: u64) -> bool {
        now_ms.saturating_sub(self.start_ms) >= self.timeout_ms as u64
    }
}

pub trait WifiRuntimeBackend {
    fn now_ms(&self) -> u64;
    fn spin_once(&mut self);
}

pub trait WifiLifecycleBackend: WifiRuntimeBackend {
    fn initialize(&mut self, config: &InitConfig) -> Result<(), InitError>;
    fn set_mode(&mut self, mode: RawMode) -> Result<(), DriverError>;
    fn start(&mut self) -> Result<(), DriverError>;
    fn stop(&mut self) -> Result<(), DriverError>;
}

pub trait WifiStaBackend {
    fn configure_sta(&mut self, _config: &ClientConfig) -> Result<(), DriverError> {
        Ok(())
    }
    fn connect(&mut self) -> Result<(), ConnectError> {
        Err(ConnectError::BackendRejected)
    }
    fn disconnect(&mut self) -> Result<(), DisconnectError> {
        Err(DisconnectError::BackendRejected)
    }
    fn sta_mac_address(&self) -> Result<[u8; 6], DriverError> {
        Err(DriverError::BackendRejected)
    }
    fn connect_blocking(&mut self, _timeout_ms: u32) -> Option<Result<(), DriverError>> {
        None
    }
}

pub trait WifiApBackend {
    fn configure_ap(&mut self, _config: &AccessPointConfig) -> Result<(), DriverError> {
        Ok(())
    }
    fn configure_mixed(
        &mut self,
        _client: &ClientConfig,
        _access_point: &AccessPointConfig,
    ) -> Result<(), DriverError> {
        Err(DriverError::BackendRejected)
    }
    fn ap_mac_address(&self) -> Result<[u8; 6], DriverError> {
        Err(DriverError::BackendRejected)
    }
    fn connected_ap_stations<const N: usize>(
        &mut self,
    ) -> Result<heapless::Vec<ApStationInfo, N>, DriverError> {
        Err(DriverError::BackendRejected)
    }
    fn deauth_ap_station(&mut self, _aid: u16) -> Result<(), DriverError> {
        Err(DriverError::BackendRejected)
    }
}

pub trait WifiPacketBackend {
    fn attach_sta_rx(&mut self) -> Result<(), DriverError> {
        Ok(())
    }
    fn attach_ap_rx(&mut self) -> Result<(), DriverError> {
        Ok(())
    }
    fn refresh_sta_rx(&mut self) -> Result<(), DriverError> {
        self.attach_sta_rx()
    }
    fn refresh_ap_rx(&mut self) -> Result<(), DriverError> {
        self.attach_ap_rx()
    }
    fn detach_sta_rx(&mut self) {}
    fn detach_ap_rx(&mut self) {}
    fn transmit(&mut self, _frame: &[u8]) -> Result<(), DriverError> {
        Err(DriverError::BackendRejected)
    }
    fn transmit_ap(&mut self, _frame: &[u8]) -> Result<(), DriverError> {
        Err(DriverError::BackendRejected)
    }
}

pub trait WifiScanBackend {
    fn scan_into<const N: usize>(
        &mut self,
        _options: &ScanOptions,
        _records: &mut heapless::Vec<AccessPointRecord, N>,
    ) -> Result<(), DriverError> {
        Err(DriverError::BackendRejected)
    }

    fn scan<const N: usize>(
        &mut self,
        options: &ScanOptions,
    ) -> Result<heapless::Vec<AccessPointRecord, N>, DriverError> {
        let mut records = heapless::Vec::new();
        self.scan_into(options, &mut records)?;
        Ok(records)
    }
}

pub trait WifiFeatureBackend {
    fn set_channel(&mut self, _channel: Channel) -> Result<(), DriverError> {
        Err(DriverError::BackendRejected)
    }
    fn enable_promiscuous(&mut self, _filter: PromiscuousFilter) -> Result<(), DriverError> {
        Err(DriverError::BackendRejected)
    }
    fn set_promiscuous_control_filter(
        &mut self,
        _filter: PromiscuousControlFilter,
    ) -> Result<(), DriverError> {
        Err(DriverError::BackendRejected)
    }
    fn disable_promiscuous(&mut self) -> Result<(), DriverError> {
        Err(DriverError::BackendRejected)
    }
    fn enable_esp_now(&mut self) -> Result<(), DriverError> {
        Err(DriverError::BackendRejected)
    }
    fn disable_esp_now(&mut self) -> Result<(), DriverError> {
        Err(DriverError::BackendRejected)
    }
    fn set_esp_now_pmk(&mut self, _pmk: &[u8; 16]) -> Result<(), DriverError> {
        Err(DriverError::BackendRejected)
    }
    fn add_esp_now_peer(&mut self, _peer: &EspNowPeerConfig) -> Result<(), DriverError> {
        Err(DriverError::BackendRejected)
    }
    fn modify_esp_now_peer(&mut self, _peer: &EspNowPeerConfig) -> Result<(), DriverError> {
        Err(DriverError::BackendRejected)
    }
    fn remove_esp_now_peer(&mut self, _peer_addr: &[u8; 6]) -> Result<(), DriverError> {
        Err(DriverError::BackendRejected)
    }
    fn send_esp_now(&mut self, _peer_addr: &[u8; 6], _payload: &[u8]) -> Result<(), DriverError> {
        Err(DriverError::BackendRejected)
    }
    fn enable_wps(&mut self, _config: &WpsConfig) -> Result<(), DriverError> {
        Err(DriverError::BackendRejected)
    }
    fn start_wps(&mut self, _timeout_ms: i32) -> Result<(), DriverError> {
        Err(DriverError::BackendRejected)
    }
    fn disable_wps(&mut self) -> Result<(), DriverError> {
        Err(DriverError::BackendRejected)
    }
    fn set_smartconfig_type(&mut self, _kind: SmartConfigType) -> Result<(), DriverError> {
        Err(DriverError::BackendRejected)
    }
    fn set_smartconfig_timeout(&mut self, _timeout: SmartConfigTimeout) -> Result<(), DriverError> {
        Err(DriverError::BackendRejected)
    }
    fn set_smartconfig_fast_mode(&mut self, _enabled: bool) -> Result<(), DriverError> {
        Err(DriverError::BackendRejected)
    }
    fn start_smartconfig(&mut self, _config: &SmartConfigStartConfig) -> Result<(), DriverError> {
        Err(DriverError::BackendRejected)
    }
    fn stop_smartconfig(&mut self) -> Result<(), DriverError> {
        Err(DriverError::BackendRejected)
    }
    fn read_smartconfig_reserved_data(&mut self, _buffer: &mut [u8]) -> Result<(), DriverError> {
        Err(DriverError::BackendRejected)
    }
}

pub trait WifiBackend:
    WifiLifecycleBackend
    + WifiStaBackend
    + WifiApBackend
    + WifiPacketBackend
    + WifiScanBackend
    + WifiFeatureBackend
{
}

impl<T> WifiBackend for T where
    T: WifiLifecycleBackend
        + WifiStaBackend
        + WifiApBackend
        + WifiPacketBackend
        + WifiScanBackend
        + WifiFeatureBackend
{
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadline_reports_timeout_at_or_after_limit() {
        let deadline = Deadline::new(1_000, 250);
        assert!(!deadline.is_expired(1_249));
        assert!(deadline.is_expired(1_250));
        assert!(deadline.is_expired(1_251));
    }

    #[test]
    fn raw_mode_constructors_match_expected_variants() {
        assert_eq!(RawMode::for_sta(), RawMode::Sta);
        assert_eq!(RawMode::for_ap(), RawMode::Ap);
        assert_eq!(RawMode::for_mixed(), RawMode::ApSta);
    }
}
