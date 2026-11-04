#[cfg(feature = "wifi-esp-now")]
use super::config::Channel;
#[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps"))]
use super::config::Ssid;
#[cfg(feature = "wifi-wps")]
use super::config::WpsPin;
#[cfg(feature = "wifi-smartconfig")]
use super::config::{ClientConfig, Ipv4Addr, Passphrase, StationCredential};
#[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps"))]
use super::ConfigError;
#[cfg(all(
    any(target_arch = "xtensa", test),
    any(feature = "wifi-esp-now", feature = "wifi-promiscuous")
))]
use super::DriverError;

#[cfg(feature = "wifi-esp-now")]
pub const ESP_NOW_MAX_PAYLOAD: usize = 250;
#[cfg(feature = "wifi-promiscuous")]
pub(crate) const PROMISCUOUS_MAX_PAYLOAD_PREFIX: usize = 112;
#[cfg(feature = "wifi-esp-now")]
pub const ESP_NOW_KEY_LEN: usize = 16;
#[cfg(feature = "wifi-smartconfig")]
pub const SMARTCONFIG_KEY_LEN: usize = 16;

#[cfg(feature = "wifi-esp-now")]
#[cfg(any(target_arch = "xtensa", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EspNowPeer {
    peer_addr: [u8; 6],
    encrypted: bool,
}

#[cfg(feature = "wifi-esp-now")]
#[cfg(any(target_arch = "xtensa", test))]
impl EspNowPeer {
    pub(crate) const fn unencrypted(peer_addr: [u8; 6]) -> Self {
        Self {
            peer_addr,
            encrypted: false,
        }
    }

    pub(crate) const fn encrypted(peer_addr: [u8; 6]) -> Self {
        Self {
            peer_addr,
            encrypted: true,
        }
    }

    pub(crate) const fn peer_addr(&self) -> &[u8; 6] {
        &self.peer_addr
    }
}

#[cfg(feature = "wifi-esp-now")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EspNowInterface {
    Station,
    AccessPoint,
}

#[cfg(feature = "wifi-esp-now")]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct EspNowPeerConfig {
    peer_addr: [u8; 6],
    lmk: Option<[u8; ESP_NOW_KEY_LEN]>,
    channel: Option<Channel>,
    interface: EspNowInterface,
}

#[cfg(feature = "wifi-esp-now")]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct EspNowStationPeerConfig {
    inner: EspNowPeerConfig,
}

#[cfg(feature = "wifi-esp-now")]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct EspNowAccessPointPeerConfig {
    inner: EspNowPeerConfig,
}

#[cfg(feature = "wifi-esp-now")]
impl EspNowPeerConfig {
    pub const BROADCAST_ADDR: [u8; 6] = [0xff; 6];

    pub const fn unencrypted(peer_addr: [u8; 6]) -> Self {
        Self {
            peer_addr,
            lmk: None,
            channel: None,
            interface: EspNowInterface::Station,
        }
    }

    pub const fn encrypted(peer_addr: [u8; 6], lmk: [u8; ESP_NOW_KEY_LEN]) -> Self {
        Self {
            peer_addr,
            lmk: Some(lmk),
            channel: None,
            interface: EspNowInterface::Station,
        }
    }

    pub const fn broadcast() -> Self {
        Self::unencrypted(Self::BROADCAST_ADDR)
    }

    pub const fn channel(mut self, channel: Channel) -> Self {
        self.channel = Some(channel);
        self
    }

    pub const fn for_station(mut self) -> EspNowStationPeerConfig {
        self.interface = EspNowInterface::Station;
        EspNowStationPeerConfig { inner: self }
    }

    pub const fn for_access_point(mut self) -> EspNowAccessPointPeerConfig {
        self.interface = EspNowInterface::AccessPoint;
        EspNowAccessPointPeerConfig { inner: self }
    }

    pub const fn peer_addr(&self) -> &[u8; 6] {
        &self.peer_addr
    }

    #[cfg_attr(not(target_arch = "xtensa"), allow(dead_code))]
    #[cfg(target_arch = "xtensa")]
    pub(crate) const fn lmk(&self) -> Option<&[u8; ESP_NOW_KEY_LEN]> {
        self.lmk.as_ref()
    }

    #[cfg_attr(not(target_arch = "xtensa"), allow(dead_code))]
    #[cfg(target_arch = "xtensa")]
    pub(crate) const fn channel_value(&self) -> u8 {
        match self.channel {
            Some(channel) => channel.get(),
            None => 0,
        }
    }

    #[cfg_attr(not(target_arch = "xtensa"), allow(dead_code))]
    #[cfg(any(target_arch = "xtensa", test))]
    pub(crate) const fn interface_value(&self) -> EspNowInterface {
        self.interface
    }

    #[cfg(any(target_arch = "xtensa", test))]
    pub(crate) const fn is_encrypted(&self) -> bool {
        self.lmk.is_some()
    }

    #[cfg(any(target_arch = "xtensa", test))]
    pub(crate) const fn peer_record(&self) -> EspNowPeer {
        if self.is_encrypted() {
            EspNowPeer::encrypted(self.peer_addr)
        } else {
            EspNowPeer::unencrypted(self.peer_addr)
        }
    }
}

#[cfg(feature = "wifi-esp-now")]
impl EspNowStationPeerConfig {
    #[cfg(any(target_arch = "xtensa", test))]
    pub(crate) const fn inner(&self) -> &EspNowPeerConfig {
        &self.inner
    }
}

#[cfg(feature = "wifi-esp-now")]
impl EspNowAccessPointPeerConfig {
    #[cfg(any(target_arch = "xtensa", test))]
    pub(crate) const fn inner(&self) -> &EspNowPeerConfig {
        &self.inner
    }
}

#[cfg(feature = "wifi-esp-now")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EspNowRxFrame {
    peer_addr: [u8; 6],
    payload: [u8; ESP_NOW_MAX_PAYLOAD],
    len: u8,
}

#[cfg(feature = "wifi-esp-now")]
impl EspNowRxFrame {
    #[cfg(any(target_arch = "xtensa", test))]
    pub(crate) fn new(peer_addr: [u8; 6], payload: &[u8]) -> Result<Self, DriverError> {
        if payload.len() > ESP_NOW_MAX_PAYLOAD {
            return Err(DriverError::PayloadTooLong);
        }

        let mut frame = Self {
            peer_addr,
            payload: [0; ESP_NOW_MAX_PAYLOAD],
            len: payload.len() as u8,
        };
        frame.payload[..payload.len()].copy_from_slice(payload);
        Ok(frame)
    }

    pub const fn peer_addr(&self) -> &[u8; 6] {
        &self.peer_addr
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload[..self.len as usize]
    }
}

#[cfg(feature = "wifi-esp-now")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EspNowSendStatus {
    Success,
    Failed,
}

#[cfg(feature = "wifi-wps")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WpsMode {
    Pbc,
    Pin,
}

#[cfg(feature = "wifi-wps")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WpsFactoryInfo<'a> {
    manufacturer: &'a [u8],
    model_number: &'a [u8],
    model_name: &'a [u8],
    device_name: &'a [u8],
}

#[cfg(feature = "wifi-wps")]
impl<'a> WpsFactoryInfo<'a> {
    pub const MANUFACTURER_MAX_LEN: usize = 64;
    pub const MODEL_NUMBER_MAX_LEN: usize = 32;
    pub const MODEL_NAME_MAX_LEN: usize = 32;
    pub const DEVICE_NAME_MAX_LEN: usize = 32;

    pub const fn empty() -> Self {
        Self {
            manufacturer: &[],
            model_number: &[],
            model_name: &[],
            device_name: &[],
        }
    }

    pub fn new(
        manufacturer: &'a [u8],
        model_number: &'a [u8],
        model_name: &'a [u8],
        device_name: &'a [u8],
    ) -> Result<Self, ConfigError> {
        if manufacturer.len() > Self::MANUFACTURER_MAX_LEN {
            return Err(ConfigError::WpsManufacturerTooLong);
        }
        if model_number.len() > Self::MODEL_NUMBER_MAX_LEN {
            return Err(ConfigError::WpsModelNumberTooLong);
        }
        if model_name.len() > Self::MODEL_NAME_MAX_LEN {
            return Err(ConfigError::WpsModelNameTooLong);
        }
        if device_name.len() > Self::DEVICE_NAME_MAX_LEN {
            return Err(ConfigError::WpsDeviceNameTooLong);
        }
        if [manufacturer, model_number, model_name, device_name]
            .iter()
            .any(|field| field.contains(&0))
        {
            return Err(ConfigError::WpsFactoryInfoContainsNul);
        }
        // The pinned native WPS library treats these fields as printf formats.
        if [manufacturer, model_number, model_name, device_name]
            .iter()
            .any(|field| field.contains(&b'%'))
        {
            return Err(ConfigError::WpsFactoryInfoContainsPercent);
        }
        Ok(Self {
            manufacturer,
            model_number,
            model_name,
            device_name,
        })
    }

    pub const fn manufacturer_bytes(&self) -> &'a [u8] {
        self.manufacturer
    }

    pub const fn model_number_bytes(&self) -> &'a [u8] {
        self.model_number
    }

    pub const fn model_name_bytes(&self) -> &'a [u8] {
        self.model_name
    }

    pub const fn device_name_bytes(&self) -> &'a [u8] {
        self.device_name
    }
}

#[cfg(feature = "wifi-wps")]
impl Default for WpsFactoryInfo<'_> {
    fn default() -> Self {
        Self::empty()
    }
}

#[cfg(feature = "wifi-wps")]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct WpsConfig<'a> {
    mode: WpsMode,
    factory_info: WpsFactoryInfo<'a>,
    pin: Option<WpsPin>,
    target_ssid: Option<&'a Ssid>,
}

#[cfg(feature = "wifi-wps")]
impl<'a> WpsConfig<'a> {
    pub const fn pbc() -> Self {
        Self {
            mode: WpsMode::Pbc,
            factory_info: WpsFactoryInfo::empty(),
            pin: None,
            target_ssid: None,
        }
    }

    pub const fn pin() -> Self {
        Self {
            mode: WpsMode::Pin,
            factory_info: WpsFactoryInfo::empty(),
            pin: None,
            target_ssid: None,
        }
    }

    pub const fn fixed_pin(pin: WpsPin) -> Self {
        Self {
            mode: WpsMode::Pin,
            factory_info: WpsFactoryInfo::empty(),
            pin: Some(pin),
            target_ssid: None,
        }
    }

    pub const fn with_factory_info(mut self, factory_info: WpsFactoryInfo<'a>) -> Self {
        self.factory_info = factory_info;
        self
    }

    pub const fn with_fixed_pin(mut self, pin: WpsPin) -> Self {
        self.mode = WpsMode::Pin;
        self.pin = Some(pin);
        self
    }

    pub const fn with_target_ssid(mut self, ssid: &'a Ssid) -> Self {
        self.target_ssid = Some(ssid);
        self
    }

    pub const fn mode(&self) -> WpsMode {
        self.mode
    }

    pub const fn factory_info(&self) -> &WpsFactoryInfo<'a> {
        &self.factory_info
    }

    pub const fn pin_override(&self) -> Option<WpsPin> {
        self.pin
    }

    pub const fn target_ssid(&self) -> Option<&'a Ssid> {
        self.target_ssid
    }
}

#[cfg(feature = "wifi-smartconfig")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SmartConfigType {
    EspTouch,
    AirKiss,
    EspTouchAirKiss,
    EspTouchV2,
}

#[cfg(feature = "wifi-smartconfig")]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SmartConfigStartConfig {
    esp_touch_v2_key: Option<[u8; SMARTCONFIG_KEY_LEN]>,
}

#[cfg(feature = "wifi-smartconfig")]
impl SmartConfigStartConfig {
    pub const fn new() -> Self {
        Self {
            esp_touch_v2_key: None,
        }
    }

    pub const fn esp_touch_v2_key(mut self, key: [u8; SMARTCONFIG_KEY_LEN]) -> Self {
        self.esp_touch_v2_key = Some(key);
        self
    }

    pub const fn esp_touch_v2_key_value(self) -> Option<[u8; SMARTCONFIG_KEY_LEN]> {
        self.esp_touch_v2_key
    }
}

#[cfg(feature = "wifi-smartconfig")]
impl Default for SmartConfigStartConfig {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "wifi-smartconfig")]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SmartConfigCredentials {
    ssid: [u8; 32],
    ssid_len: u8,
    password: [u8; 64],
    password_len: u8,
    bssid: Option<[u8; 6]>,
    protocol: SmartConfigType,
    token: u8,
    cellphone_ip: Ipv4Addr,
}

#[cfg(feature = "wifi-smartconfig")]
impl SmartConfigCredentials {
    pub fn new(
        ssid: &[u8],
        password: &[u8],
        bssid: Option<[u8; 6]>,
        protocol: SmartConfigType,
        token: u8,
        cellphone_ip: Ipv4Addr,
    ) -> Result<Self, ConfigError> {
        Ssid::from_bytes(ssid)?;
        if !password.is_empty() {
            Passphrase::wpa2_bytes(password)?;
        }

        let mut credentials = Self {
            ssid: [0; 32],
            ssid_len: ssid.len() as u8,
            password: [0; 64],
            password_len: password.len() as u8,
            bssid,
            protocol,
            token,
            cellphone_ip,
        };
        credentials.ssid[..ssid.len()].copy_from_slice(ssid);
        credentials.password[..password.len()].copy_from_slice(password);
        Ok(credentials)
    }

    pub fn ssid(&self) -> &[u8] {
        &self.ssid[..self.ssid_len as usize]
    }

    pub fn password(&self) -> &[u8] {
        &self.password[..self.password_len as usize]
    }

    pub const fn bssid(&self) -> Option<[u8; 6]> {
        self.bssid
    }

    pub const fn protocol(&self) -> SmartConfigType {
        self.protocol
    }

    pub const fn token(&self) -> u8 {
        self.token
    }

    pub const fn cellphone_ip(&self) -> Ipv4Addr {
        self.cellphone_ip
    }

    pub fn client_config(&self) -> Result<ClientConfig<'static>, ConfigError> {
        let ssid = Ssid::from_bytes(self.ssid())?;
        let credential = if self.password().is_empty() {
            StationCredential::open()
        } else {
            StationCredential::wpa2(Passphrase::wpa2_bytes(self.password())?)
        };
        let config = ClientConfig::new(ssid, credential);
        Ok(match self.bssid {
            Some(bssid) => config.with_bssid(bssid),
            None => config,
        })
    }
}

#[cfg(feature = "wifi-smartconfig")]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SmartConfigEvent {
    ScanDone,
    FoundChannel,
    GotCredentials(SmartConfigCredentials),
    SendAckDone,
}

#[cfg(feature = "wifi-wps")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WpsEvent {
    Succeeded,
    Failed { reason: Option<u32> },
    TimedOut,
    Pin { pin: WpsPin },
    Disconnected { reason: Option<u8> },
}

#[cfg(feature = "wifi-promiscuous")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromiscuousPacketType {
    Management,
    Control,
    Data,
    Misc,
}

#[cfg(feature = "wifi-promiscuous")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromiscuousControlSubtype {
    Wrapper,
    BlockAckRequest,
    BlockAck,
    PsPoll,
    Rts,
    Cts,
    Ack,
    CfEnd,
    CfEndAck,
}

#[cfg(feature = "wifi-promiscuous")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct PromiscuousControlFilter {
    bits: u16,
}

#[cfg(feature = "wifi-promiscuous")]
impl PromiscuousControlFilter {
    pub const WRAPPER: Self = Self { bits: 1 << 0 };
    pub const BLOCK_ACK_REQUEST: Self = Self { bits: 1 << 1 };
    pub const BLOCK_ACK: Self = Self { bits: 1 << 2 };
    pub const PS_POLL: Self = Self { bits: 1 << 3 };
    pub const RTS: Self = Self { bits: 1 << 4 };
    pub const CTS: Self = Self { bits: 1 << 5 };
    pub const ACK: Self = Self { bits: 1 << 6 };
    pub const CF_END: Self = Self { bits: 1 << 7 };
    pub const CF_END_ACK: Self = Self { bits: 1 << 8 };

    pub const fn empty() -> Self {
        Self { bits: 0 }
    }

    pub const fn all() -> Self {
        Self {
            bits: Self::WRAPPER.bits
                | Self::BLOCK_ACK_REQUEST.bits
                | Self::BLOCK_ACK.bits
                | Self::PS_POLL.bits
                | Self::RTS.bits
                | Self::CTS.bits
                | Self::ACK.bits
                | Self::CF_END.bits
                | Self::CF_END_ACK.bits,
        }
    }

    pub const fn from_bits_truncate(bits: u16) -> Self {
        Self {
            bits: bits & Self::all().bits,
        }
    }

    pub const fn bits(self) -> u16 {
        self.bits
    }

    pub const fn is_empty(self) -> bool {
        self.bits == 0
    }

    pub const fn contains(self, subtype: PromiscuousControlSubtype) -> bool {
        self.bits & control_subtype_bit(subtype) != 0
    }

    pub const fn contains_all(self, other: Self) -> bool {
        self.bits & other.bits == other.bits
    }

    pub const fn union(self, other: Self) -> Self {
        Self {
            bits: self.bits | other.bits,
        }
    }
}

#[cfg(feature = "wifi-promiscuous")]
impl Default for PromiscuousControlFilter {
    fn default() -> Self {
        Self::empty()
    }
}

#[cfg(feature = "wifi-promiscuous")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct PromiscuousFilter {
    bits: u8,
}

#[cfg(feature = "wifi-promiscuous")]
impl PromiscuousFilter {
    pub const MANAGEMENT: Self = Self { bits: 1 << 0 };
    pub const CONTROL: Self = Self { bits: 1 << 1 };
    pub const DATA: Self = Self { bits: 1 << 2 };
    pub const MISC: Self = Self { bits: 1 << 3 };

    pub const fn empty() -> Self {
        Self { bits: 0 }
    }

    pub const fn all() -> Self {
        Self {
            bits: Self::MANAGEMENT.bits | Self::CONTROL.bits | Self::DATA.bits | Self::MISC.bits,
        }
    }

    pub const fn from_bits_truncate(bits: u8) -> Self {
        Self {
            bits: bits & Self::all().bits,
        }
    }

    pub const fn bits(self) -> u8 {
        self.bits
    }

    pub const fn is_empty(self) -> bool {
        self.bits == 0
    }

    pub const fn contains(self, packet_type: PromiscuousPacketType) -> bool {
        self.bits & packet_type_bit(packet_type) != 0
    }

    pub const fn contains_all(self, other: Self) -> bool {
        self.bits & other.bits == other.bits
    }

    pub const fn union(self, other: Self) -> Self {
        Self {
            bits: self.bits | other.bits,
        }
    }
}

#[cfg(feature = "wifi-promiscuous")]
impl Default for PromiscuousFilter {
    fn default() -> Self {
        Self::all()
    }
}

#[cfg(feature = "wifi-promiscuous")]
const fn packet_type_bit(packet_type: PromiscuousPacketType) -> u8 {
    match packet_type {
        PromiscuousPacketType::Management => PromiscuousFilter::MANAGEMENT.bits,
        PromiscuousPacketType::Control => PromiscuousFilter::CONTROL.bits,
        PromiscuousPacketType::Data => PromiscuousFilter::DATA.bits,
        PromiscuousPacketType::Misc => PromiscuousFilter::MISC.bits,
    }
}

#[cfg(feature = "wifi-promiscuous")]
const fn control_subtype_bit(subtype: PromiscuousControlSubtype) -> u16 {
    match subtype {
        PromiscuousControlSubtype::Wrapper => PromiscuousControlFilter::WRAPPER.bits,
        PromiscuousControlSubtype::BlockAckRequest => {
            PromiscuousControlFilter::BLOCK_ACK_REQUEST.bits
        }
        PromiscuousControlSubtype::BlockAck => PromiscuousControlFilter::BLOCK_ACK.bits,
        PromiscuousControlSubtype::PsPoll => PromiscuousControlFilter::PS_POLL.bits,
        PromiscuousControlSubtype::Rts => PromiscuousControlFilter::RTS.bits,
        PromiscuousControlSubtype::Cts => PromiscuousControlFilter::CTS.bits,
        PromiscuousControlSubtype::Ack => PromiscuousControlFilter::ACK.bits,
        PromiscuousControlSubtype::CfEnd => PromiscuousControlFilter::CF_END.bits,
        PromiscuousControlSubtype::CfEndAck => PromiscuousControlFilter::CF_END_ACK.bits,
    }
}

#[cfg(feature = "wifi-promiscuous")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PromiscuousFrameInfo {
    pub rssi: i8,
    pub channel: u8,
    pub packet_type: PromiscuousPacketType,
    pub original_len: u16,
}

#[cfg(feature = "wifi-promiscuous")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PromiscuousRxFrame {
    info: PromiscuousFrameInfo,
    payload: [u8; PROMISCUOUS_MAX_PAYLOAD_PREFIX],
    len: u16,
}

#[cfg(feature = "wifi-promiscuous")]
impl PromiscuousRxFrame {
    #[cfg(any(target_arch = "xtensa", test))]
    pub(crate) fn new(info: PromiscuousFrameInfo, payload: &[u8]) -> Result<Self, DriverError> {
        if payload.len() > PROMISCUOUS_MAX_PAYLOAD_PREFIX
            || payload.len() > usize::from(info.original_len)
        {
            return Err(DriverError::PayloadTooLong);
        }

        let mut frame = Self {
            info,
            payload: [0; PROMISCUOUS_MAX_PAYLOAD_PREFIX],
            len: payload.len() as u16,
        };
        frame.payload[..payload.len()].copy_from_slice(payload);
        Ok(frame)
    }

    pub const fn info(&self) -> PromiscuousFrameInfo {
        self.info
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload[..self.len as usize]
    }

    pub const fn original_len(&self) -> usize {
        self.info.original_len as usize
    }

    pub const fn is_truncated(&self) -> bool {
        self.len < self.info.original_len
    }
}

#[cfg(feature = "wifi-esp-now")]
#[cfg(any(target_arch = "xtensa", test))]
pub(crate) struct EspNowPeerTable {
    peers: allocator_api2::vec::Vec<EspNowPeer, super::allocator::RuntimeAllocator>,
    capacity: usize,
}

#[cfg(feature = "wifi-esp-now")]
#[cfg(any(target_arch = "xtensa", test))]
impl EspNowPeerTable {
    pub fn try_new(
        capacity: usize,
        allocator: super::allocator::RuntimeAllocator,
    ) -> Result<Self, super::InitError> {
        let mut peers = allocator_api2::vec::Vec::new_in(allocator);
        peers
            .try_reserve_exact(capacity)
            .map_err(|_| super::InitError::OutOfMemory)?;
        Ok(Self { peers, capacity })
    }
    pub fn len(&self) -> usize {
        self.peers.len()
    }
    pub fn capacity(&self) -> usize {
        self.capacity
    }
    pub fn clear(&mut self) {
        self.peers.clear();
    }
    pub fn contains(&self, addr: &[u8; 6]) -> bool {
        self.peers.iter().any(|peer| peer.peer_addr() == addr)
    }
    pub fn insert(&mut self, peer: EspNowPeer) -> Result<bool, DriverError> {
        if let Some(existing) = self
            .peers
            .iter_mut()
            .find(|p| p.peer_addr() == peer.peer_addr())
        {
            let changed = *existing != peer;
            *existing = peer;
            return Ok(changed);
        }
        if self.peers.len() == self.capacity {
            return Err(DriverError::PeerOverflow);
        }
        self.peers.push(peer);
        Ok(true)
    }
    pub fn remove(&mut self, addr: &[u8; 6]) -> bool {
        if let Some(index) = self.peers.iter().position(|peer| peer.peer_addr() == addr) {
            self.peers.swap_remove(index);
            true
        } else {
            false
        }
    }
}
