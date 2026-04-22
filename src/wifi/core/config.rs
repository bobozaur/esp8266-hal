use heapless::Vec;

use super::error::{ConfigError, StaticIpv4Error};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ssid(Vec<u8, 32>);

impl Ssid {
    pub const MAX_LEN: usize = 32;

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ConfigError> {
        if bytes.len() > Self::MAX_LEN {
            return Err(ConfigError::SsidTooLong);
        }

        let mut ssid = Vec::new();
        ssid.extend_from_slice(bytes)
            .map_err(|_| ConfigError::SsidTooLong)?;
        Ok(Self(ssid))
    }
}

impl TryFrom<&str> for Ssid {
    type Error = ConfigError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::from_bytes(value.as_bytes())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Passphrase(Vec<u8, 64>);

impl Passphrase {
    pub const WPA2_MIN_LEN: usize = 8;
    pub const WPA2_MAX_LEN: usize = 64;

    pub fn wpa2(value: &str) -> Result<Self, ConfigError> {
        Self::wpa2_bytes(value.as_bytes())
    }

    pub fn wpa2_bytes(bytes: &[u8]) -> Result<Self, ConfigError> {
        if bytes.len() < Self::WPA2_MIN_LEN {
            return Err(ConfigError::PassphraseTooShort);
        }
        if bytes.len() > Self::WPA2_MAX_LEN {
            return Err(ConfigError::PassphraseTooLong);
        }

        let mut passphrase = Vec::new();
        passphrase
            .extend_from_slice(bytes)
            .map_err(|_| ConfigError::PassphraseTooLong)?;
        Ok(Self(passphrase))
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Channel(u8);

impl Channel {
    pub const fn get(self) -> u8 {
        self.0
    }
}

impl TryFrom<u8> for Channel {
    type Error = ConfigError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1..=14 => Ok(Self(value)),
            other => Err(ConfigError::InvalidChannel(other)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ipv4Addr([u8; 4]);

impl Ipv4Addr {
    pub const UNSPECIFIED: Self = Self([0, 0, 0, 0]);

    pub const fn new(a: u8, b: u8, c: u8, d: u8) -> Self {
        Self([a, b, c, d])
    }

    pub const fn octets(self) -> [u8; 4] {
        self.0
    }

    pub const fn is_unspecified(self) -> bool {
        self.0[0] == 0 && self.0[1] == 0 && self.0[2] == 0 && self.0[3] == 0
    }

    fn as_u32(self) -> u32 {
        u32::from_be_bytes(self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StaticIpv4Config {
    address: Ipv4Addr,
    prefix_len: u8,
    gateway: Option<Ipv4Addr>,
}

impl StaticIpv4Config {
    pub fn new(
        address: Ipv4Addr,
        prefix_len: u8,
        gateway: Option<Ipv4Addr>,
    ) -> Result<Self, StaticIpv4Error> {
        if prefix_len > 32 {
            return Err(StaticIpv4Error::InvalidPrefix);
        }
        if address.is_unspecified() {
            return Err(StaticIpv4Error::UnspecifiedAddress);
        }
        if let Some(gateway) = gateway {
            if gateway.is_unspecified() {
                return Err(StaticIpv4Error::UnspecifiedGateway);
            }
            if !same_ipv4_subnet(address, gateway, prefix_len) {
                return Err(StaticIpv4Error::GatewayOutsideSubnet);
            }
        }

        Ok(Self {
            address,
            prefix_len,
            gateway,
        })
    }

    pub const fn address(self) -> Ipv4Addr {
        self.address
    }

    pub const fn prefix_len(self) -> u8 {
        self.prefix_len
    }

    pub const fn gateway(self) -> Option<Ipv4Addr> {
        self.gateway
    }
}

fn same_ipv4_subnet(left: Ipv4Addr, right: Ipv4Addr, prefix_len: u8) -> bool {
    let mask = if prefix_len == 0 {
        0
    } else {
        u32::MAX << (32 - u32::from(prefix_len))
    };

    left.as_u32() & mask == right.as_u32() & mask
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApStationInfo {
    mac: [u8; 6],
    phy_11b: bool,
    phy_11g: bool,
    phy_11n: bool,
}

impl ApStationInfo {
    pub const fn new(mac: [u8; 6], phy_11b: bool, phy_11g: bool, phy_11n: bool) -> Self {
        Self {
            mac,
            phy_11b,
            phy_11g,
            phy_11n,
        }
    }

    pub const fn mac(self) -> [u8; 6] {
        self.mac
    }

    pub const fn supports_11b(self) -> bool {
        self.phy_11b
    }

    pub const fn supports_11g(self) -> bool {
        self.phy_11g
    }

    pub const fn supports_11n(self) -> bool {
        self.phy_11n
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanType {
    Active,
    Passive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScanTimings {
    active_min_ms: u32,
    active_max_ms: u32,
    passive_ms: u32,
}

impl ScanTimings {
    pub const fn sdk_default() -> Self {
        Self {
            active_min_ms: 0,
            active_max_ms: 0,
            passive_ms: 0,
        }
    }

    pub const fn active(min_ms: u32, max_ms: u32) -> Self {
        Self {
            active_min_ms: min_ms,
            active_max_ms: max_ms,
            passive_ms: 0,
        }
    }

    pub const fn passive(ms: u32) -> Self {
        Self {
            active_min_ms: 0,
            active_max_ms: 0,
            passive_ms: ms,
        }
    }

    pub const fn active_min_ms(self) -> u32 {
        self.active_min_ms
    }

    pub const fn active_max_ms(self) -> u32 {
        self.active_max_ms
    }

    pub const fn passive_ms(self) -> u32 {
        self.passive_ms
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanOptions {
    ssid: Option<Ssid>,
    bssid: Option<[u8; 6]>,
    channel: Option<Channel>,
    show_hidden: bool,
    scan_type: ScanType,
    timings: ScanTimings,
}

impl ScanOptions {
    pub const fn all() -> Self {
        Self {
            ssid: None,
            bssid: None,
            channel: None,
            show_hidden: false,
            scan_type: ScanType::Active,
            timings: ScanTimings::sdk_default(),
        }
    }

    pub fn ssid(mut self, ssid: Ssid) -> Self {
        self.ssid = Some(ssid);
        self
    }

    pub const fn bssid(mut self, bssid: [u8; 6]) -> Self {
        self.bssid = Some(bssid);
        self
    }

    pub const fn channel(mut self, channel: Channel) -> Self {
        self.channel = Some(channel);
        self
    }

    pub const fn show_hidden(mut self, show_hidden: bool) -> Self {
        self.show_hidden = show_hidden;
        self
    }

    pub const fn scan_type(mut self, scan_type: ScanType) -> Self {
        self.scan_type = scan_type;
        self
    }

    pub const fn timings(mut self, timings: ScanTimings) -> Self {
        self.timings = timings;
        self
    }

    pub const fn target_ssid(&self) -> Option<&Ssid> {
        self.ssid.as_ref()
    }

    pub const fn target_bssid(&self) -> Option<[u8; 6]> {
        self.bssid
    }

    pub const fn target_channel(&self) -> Option<Channel> {
        self.channel
    }

    pub const fn shows_hidden(&self) -> bool {
        self.show_hidden
    }

    pub const fn scan_kind(&self) -> ScanType {
        self.scan_type
    }

    pub const fn scan_timings(&self) -> ScanTimings {
        self.timings
    }
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self::all()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessPointRecord {
    ssid: Ssid,
    bssid: [u8; 6],
    channel: Channel,
    rssi: i8,
    auth_mode: u8,
    wps: bool,
}

impl AccessPointRecord {
    pub fn new(
        ssid: &[u8],
        bssid: [u8; 6],
        channel: Channel,
        rssi: i8,
        auth_mode: u8,
        wps: bool,
    ) -> Result<Self, ConfigError> {
        if ssid.len() > 32 {
            return Err(ConfigError::SsidTooLong);
        }

        let mut ssid_bytes = Vec::new();
        ssid_bytes
            .extend_from_slice(ssid)
            .map_err(|_| ConfigError::SsidTooLong)?;

        Ok(Self {
            ssid: Ssid(ssid_bytes),
            bssid,
            channel,
            rssi,
            auth_mode,
            wps,
        })
    }

    pub fn ssid(&self) -> &[u8] {
        self.ssid.as_bytes()
    }

    pub const fn bssid(&self) -> [u8; 6] {
        self.bssid
    }

    pub const fn channel(&self) -> Channel {
        self.channel
    }

    pub const fn rssi(&self) -> i8 {
        self.rssi
    }

    pub const fn auth_mode(&self) -> u8 {
        self.auth_mode
    }

    pub const fn wps(&self) -> bool {
        self.wps
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApClientLimit(u8);

impl ApClientLimit {
    pub const DEFAULT: Self = Self(4);

    pub const fn get(self) -> u8 {
        self.0
    }
}

impl TryFrom<u8> for ApClientLimit {
    type Error = ConfigError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1..=10 => Ok(Self(value)),
            other => Err(ConfigError::InvalidApClientLimit(other)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BeaconInterval(u16);

impl BeaconInterval {
    pub const DEFAULT: Self = Self(100);

    pub const fn millis(self) -> u16 {
        self.0
    }
}

impl TryFrom<u16> for BeaconInterval {
    type Error = ConfigError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        match value {
            100..=60_000 => Ok(Self(value)),
            other => Err(ConfigError::InvalidBeaconInterval(other)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Credential {
    Open,
    Wpa2(Passphrase),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StationPowerSave {
    None,
    MinModem,
    MaxModem,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientConfig {
    ssid: Ssid,
    credential: Credential,
    bssid: Option<[u8; 6]>,
    channel: Option<Channel>,
    power_save: StationPowerSave,
}

impl ClientConfig {
    pub fn new(ssid: Ssid, credential: Credential) -> Self {
        Self {
            ssid,
            credential,
            bssid: None,
            channel: None,
            power_save: StationPowerSave::None,
        }
    }

    pub const fn with_bssid(mut self, bssid: [u8; 6]) -> Self {
        self.bssid = Some(bssid);
        self
    }

    pub const fn with_channel(mut self, channel: Channel) -> Self {
        self.channel = Some(channel);
        self
    }

    pub const fn with_power_save(mut self, power_save: StationPowerSave) -> Self {
        self.power_save = power_save;
        self
    }

    pub fn ssid(&self) -> &Ssid {
        &self.ssid
    }

    pub fn credential(&self) -> &Credential {
        &self.credential
    }

    pub const fn bssid(&self) -> Option<[u8; 6]> {
        self.bssid
    }

    pub const fn channel(&self) -> Option<Channel> {
        self.channel
    }

    pub const fn power_save(&self) -> StationPowerSave {
        self.power_save
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessPointConfig {
    ssid: Ssid,
    channel: Channel,
    credential: Credential,
    max_clients: ApClientLimit,
    beacon_interval: BeaconInterval,
}

impl AccessPointConfig {
    pub fn open(ssid: Ssid, channel: Channel) -> Self {
        Self::builder(ssid, channel).build()
    }

    pub fn builder(ssid: Ssid, channel: Channel) -> AccessPointConfigBuilder {
        AccessPointConfigBuilder {
            ssid,
            channel,
            credential: Credential::Open,
            max_clients: ApClientLimit::DEFAULT,
            beacon_interval: BeaconInterval::DEFAULT,
        }
    }

    pub fn ssid(&self) -> &Ssid {
        &self.ssid
    }

    pub fn channel(&self) -> Channel {
        self.channel
    }

    pub fn credential(&self) -> &Credential {
        &self.credential
    }

    pub const fn max_clients(&self) -> ApClientLimit {
        self.max_clients
    }

    pub const fn beacon_interval(&self) -> BeaconInterval {
        self.beacon_interval
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessPointConfigBuilder {
    ssid: Ssid,
    channel: Channel,
    credential: Credential,
    max_clients: ApClientLimit,
    beacon_interval: BeaconInterval,
}

impl AccessPointConfigBuilder {
    pub fn credential(mut self, credential: Credential) -> Self {
        self.credential = credential;
        self
    }

    pub fn max_clients(mut self, max_clients: ApClientLimit) -> Self {
        self.max_clients = max_clients;
        self
    }

    pub fn beacon_interval(mut self, beacon_interval: BeaconInterval) -> Self {
        self.beacon_interval = beacon_interval;
        self
    }

    pub fn build(self) -> AccessPointConfig {
        AccessPointConfig {
            ssid: self.ssid,
            channel: self.channel,
            credential: self.credential,
            max_clients: self.max_clients,
            beacon_interval: self.beacon_interval,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MixedConfig {
    client: ClientConfig,
    ap: AccessPointConfig,
}

impl MixedConfig {
    pub fn new(client: ClientConfig, ap: AccessPointConfig) -> Self {
        Self { client, ap }
    }

    pub fn client(&self) -> &ClientConfig {
        &self.client
    }

    pub fn access_point(&self) -> &AccessPointConfig {
        &self.ap
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InitConfig {
    rx_buf_num: u8,
    rx_pkt_num: u8,
    tx_buf_num: u8,
}

impl InitConfig {
    pub const RX_BUF_MIN: u8 = 14;
    pub const RX_BUF_MAX: u8 = 28;
    pub const RX_PKT_MIN: u8 = 4;
    pub const RX_PKT_MAX: u8 = 16;
    pub const TX_BUF_MIN: u8 = 4;
    pub const TX_BUF_MAX: u8 = 16;

    pub fn new(rx_buf_num: u8, rx_pkt_num: u8, tx_buf_num: u8) -> Result<Self, ConfigError> {
        if !(Self::RX_BUF_MIN..=Self::RX_BUF_MAX).contains(&rx_buf_num) {
            return Err(ConfigError::InvalidRxBufferCount(rx_buf_num));
        }
        if !(Self::RX_PKT_MIN..=Self::RX_PKT_MAX).contains(&rx_pkt_num) {
            return Err(ConfigError::InvalidRxPacketCount(rx_pkt_num));
        }
        if !(Self::TX_BUF_MIN..=Self::TX_BUF_MAX).contains(&tx_buf_num) {
            return Err(ConfigError::InvalidTxBufferCount(tx_buf_num));
        }

        Ok(Self {
            rx_buf_num,
            rx_pkt_num,
            tx_buf_num,
        })
    }

    pub const fn rx_buf_num(&self) -> u8 {
        self.rx_buf_num
    }

    pub const fn rx_pkt_num(&self) -> u8 {
        self.rx_pkt_num
    }

    pub const fn tx_buf_num(&self) -> u8 {
        self.tx_buf_num
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SmartConfigTimeout(u8);

impl SmartConfigTimeout {
    pub const fn seconds(self) -> u8 {
        self.0
    }
}

impl TryFrom<u8> for SmartConfigTimeout {
    type Error = ConfigError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            15..=255 => Ok(Self(value)),
            other => Err(ConfigError::InvalidSmartConfigTimeout(other)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WpsPin([u8; 8]);

impl WpsPin {
    pub const fn as_bytes(&self) -> &[u8; 8] {
        &self.0
    }

    pub fn from_digits(digits: [u8; 8]) -> Result<Self, ConfigError> {
        if !digits.iter().all(u8::is_ascii_digit) || !wps_pin_checksum_valid(&digits) {
            return Err(ConfigError::InvalidWpsPin);
        }

        Ok(Self(digits))
    }
}

impl TryFrom<&str> for WpsPin {
    type Error = ConfigError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let bytes = value.as_bytes();
        if bytes.len() != 8 || !bytes.iter().all(u8::is_ascii_digit) {
            return Err(ConfigError::InvalidWpsPin);
        }

        let mut pin = [0; 8];
        pin.copy_from_slice(bytes);
        Self::from_digits(pin)
    }
}

fn wps_pin_checksum_valid(pin: &[u8; 8]) -> bool {
    if pin == b"00000000" {
        return false;
    }

    let mut pin7 = 0u32;
    for digit in &pin[..7] {
        pin7 = pin7 * 10 + u32::from(*digit - b'0');
    }

    let mut value = pin7;
    let mut checksum = 0u32;
    while value != 0 {
        checksum += 3 * (value % 10);
        value /= 10;
        checksum += value % 10;
        value /= 10;
    }

    pin[7] == ((10 - checksum % 10) % 10) as u8 + b'0'
}

impl Default for InitConfig {
    fn default() -> Self {
        Self {
            rx_buf_num: 16,
            rx_pkt_num: 7,
            tx_buf_num: 6,
        }
    }
}
