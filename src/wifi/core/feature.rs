use super::config::{Channel, ClientConfig, Credential, Ipv4Addr, Passphrase, Ssid, WpsPin};
use super::ConfigError;
use super::DriverError;

pub const ESP_NOW_MAX_PAYLOAD: usize = 250;
pub const PROMISCUOUS_MAX_PAYLOAD_PREFIX: usize = 256;
pub const ESP_NOW_KEY_LEN: usize = 16;
pub const SMARTCONFIG_KEY_LEN: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EspNowPeer {
    peer_addr: [u8; 6],
    encrypted: bool,
}

impl EspNowPeer {
    pub const fn unencrypted(peer_addr: [u8; 6]) -> Self {
        Self {
            peer_addr,
            encrypted: false,
        }
    }

    pub const fn encrypted(peer_addr: [u8; 6]) -> Self {
        Self {
            peer_addr,
            encrypted: true,
        }
    }

    pub const fn peer_addr(&self) -> &[u8; 6] {
        &self.peer_addr
    }

    pub const fn is_encrypted(&self) -> bool {
        self.encrypted
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EspNowInterface {
    Station,
    AccessPoint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EspNowPeerConfig {
    peer_addr: [u8; 6],
    lmk: Option<[u8; ESP_NOW_KEY_LEN]>,
    channel: Option<Channel>,
    interface: EspNowInterface,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EspNowStationPeerConfig {
    inner: EspNowPeerConfig,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EspNowAccessPointPeerConfig {
    inner: EspNowPeerConfig,
}

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

    pub const fn lmk(&self) -> Option<&[u8; ESP_NOW_KEY_LEN]> {
        self.lmk.as_ref()
    }

    pub const fn channel_value(&self) -> u8 {
        match self.channel {
            Some(channel) => channel.get(),
            None => 0,
        }
    }

    pub const fn interface_value(&self) -> EspNowInterface {
        self.interface
    }

    pub const fn is_encrypted(&self) -> bool {
        self.lmk.is_some()
    }

    pub const fn peer_record(&self) -> EspNowPeer {
        if self.is_encrypted() {
            EspNowPeer::encrypted(self.peer_addr)
        } else {
            EspNowPeer::unencrypted(self.peer_addr)
        }
    }
}

impl EspNowStationPeerConfig {
    pub const fn inner(&self) -> &EspNowPeerConfig {
        &self.inner
    }
}

impl EspNowAccessPointPeerConfig {
    pub const fn inner(&self) -> &EspNowPeerConfig {
        &self.inner
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EspNowRxFrame {
    peer_addr: [u8; 6],
    payload: [u8; ESP_NOW_MAX_PAYLOAD],
    len: u8,
}

impl EspNowRxFrame {
    pub fn new(peer_addr: [u8; 6], payload: &[u8]) -> Result<Self, DriverError> {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EspNowSendStatus {
    Success,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EspNowTxStatus {
    peer_addr: [u8; 6],
    status: EspNowSendStatus,
}

impl EspNowTxStatus {
    pub const fn new(peer_addr: [u8; 6], status: EspNowSendStatus) -> Self {
        Self { peer_addr, status }
    }

    pub const fn peer_addr(&self) -> &[u8; 6] {
        &self.peer_addr
    }

    pub const fn status(&self) -> EspNowSendStatus {
        self.status
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WpsMode {
    Pbc,
    Pin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WpsFactoryInfo {
    manufacturer: heapless::Vec<u8, 64>,
    model_number: heapless::Vec<u8, 32>,
    model_name: heapless::Vec<u8, 32>,
    device_name: heapless::Vec<u8, 32>,
}

impl WpsFactoryInfo {
    pub const fn empty() -> Self {
        Self {
            manufacturer: heapless::Vec::new(),
            model_number: heapless::Vec::new(),
            model_name: heapless::Vec::new(),
            device_name: heapless::Vec::new(),
        }
    }

    pub fn manufacturer(mut self, value: &[u8]) -> Self {
        self.manufacturer.clear();
        let _ = self
            .manufacturer
            .extend_from_slice(&value[..value.len().min(64)]);
        self
    }

    pub fn model_number(mut self, value: &[u8]) -> Self {
        self.model_number.clear();
        let _ = self
            .model_number
            .extend_from_slice(&value[..value.len().min(32)]);
        self
    }

    pub fn model_name(mut self, value: &[u8]) -> Self {
        self.model_name.clear();
        let _ = self
            .model_name
            .extend_from_slice(&value[..value.len().min(32)]);
        self
    }

    pub fn device_name(mut self, value: &[u8]) -> Self {
        self.device_name.clear();
        let _ = self
            .device_name
            .extend_from_slice(&value[..value.len().min(32)]);
        self
    }

    pub fn manufacturer_bytes(&self) -> &[u8] {
        &self.manufacturer
    }

    pub fn model_number_bytes(&self) -> &[u8] {
        &self.model_number
    }

    pub fn model_name_bytes(&self) -> &[u8] {
        &self.model_name
    }

    pub fn device_name_bytes(&self) -> &[u8] {
        &self.device_name
    }
}

impl Default for WpsFactoryInfo {
    fn default() -> Self {
        Self::empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WpsConfig {
    mode: WpsMode,
    factory_info: WpsFactoryInfo,
    pin: Option<WpsPin>,
    target_ssid: Option<Ssid>,
}

impl WpsConfig {
    pub fn pbc() -> Self {
        Self {
            mode: WpsMode::Pbc,
            factory_info: WpsFactoryInfo::default(),
            pin: None,
            target_ssid: None,
        }
    }

    pub fn pin() -> Self {
        Self {
            mode: WpsMode::Pin,
            factory_info: WpsFactoryInfo::default(),
            pin: None,
            target_ssid: None,
        }
    }

    pub fn fixed_pin(pin: WpsPin) -> Self {
        Self {
            mode: WpsMode::Pin,
            factory_info: WpsFactoryInfo::default(),
            pin: Some(pin),
            target_ssid: None,
        }
    }

    pub fn with_factory_info(mut self, factory_info: WpsFactoryInfo) -> Self {
        self.factory_info = factory_info;
        self
    }

    pub fn with_fixed_pin(mut self, pin: WpsPin) -> Self {
        self.mode = WpsMode::Pin;
        self.pin = Some(pin);
        self
    }

    pub fn with_target_ssid(mut self, ssid: Ssid) -> Self {
        self.target_ssid = Some(ssid);
        self
    }

    pub const fn mode(&self) -> WpsMode {
        self.mode
    }

    pub const fn factory_info(&self) -> &WpsFactoryInfo {
        &self.factory_info
    }

    pub const fn pin_override(&self) -> Option<WpsPin> {
        self.pin
    }

    pub const fn target_ssid(&self) -> Option<&Ssid> {
        self.target_ssid.as_ref()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SmartConfigType {
    EspTouch,
    AirKiss,
    EspTouchAirKiss,
    EspTouchV2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SmartConfigStartConfig {
    enable_log: bool,
    esp_touch_v2_key: Option<[u8; SMARTCONFIG_KEY_LEN]>,
}

impl SmartConfigStartConfig {
    pub const fn new() -> Self {
        Self {
            enable_log: false,
            esp_touch_v2_key: None,
        }
    }

    pub const fn enable_log(mut self, enable: bool) -> Self {
        self.enable_log = enable;
        self
    }

    pub const fn esp_touch_v2_key(mut self, key: [u8; SMARTCONFIG_KEY_LEN]) -> Self {
        self.esp_touch_v2_key = Some(key);
        self
    }

    pub const fn log_enabled(self) -> bool {
        self.enable_log
    }

    pub const fn esp_touch_v2_key_value(self) -> Option<[u8; SMARTCONFIG_KEY_LEN]> {
        self.esp_touch_v2_key
    }
}

impl Default for SmartConfigStartConfig {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

impl SmartConfigCredentials {
    pub fn new(
        ssid: &[u8],
        password: &[u8],
        bssid: Option<[u8; 6]>,
        protocol: SmartConfigType,
        token: u8,
        cellphone_ip: Ipv4Addr,
    ) -> Result<Self, ConfigError> {
        if ssid.len() > 32 {
            return Err(ConfigError::SsidTooLong);
        }
        if password.len() > 64 {
            return Err(ConfigError::PassphraseTooLong);
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

    pub fn client_config(&self) -> Result<ClientConfig, ConfigError> {
        let ssid = Ssid::from_bytes(self.ssid())?;
        let credential = if self.password().is_empty() {
            Credential::Open
        } else {
            Credential::Wpa2(Passphrase::wpa2_bytes(self.password())?)
        };
        Ok(ClientConfig::new(ssid, credential))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromiscuousPacketType {
    Management,
    Control,
    Data,
    Misc,
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct PromiscuousControlFilter {
    bits: u16,
}

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

impl Default for PromiscuousControlFilter {
    fn default() -> Self {
        Self::empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct PromiscuousFilter {
    bits: u8,
}

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

impl Default for PromiscuousFilter {
    fn default() -> Self {
        Self::all()
    }
}

const fn packet_type_bit(packet_type: PromiscuousPacketType) -> u8 {
    match packet_type {
        PromiscuousPacketType::Management => PromiscuousFilter::MANAGEMENT.bits,
        PromiscuousPacketType::Control => PromiscuousFilter::CONTROL.bits,
        PromiscuousPacketType::Data => PromiscuousFilter::DATA.bits,
        PromiscuousPacketType::Misc => PromiscuousFilter::MISC.bits,
    }
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PromiscuousFrameInfo {
    pub rssi: i8,
    pub channel: u8,
    pub packet_type: PromiscuousPacketType,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PromiscuousRxFrame {
    info: PromiscuousFrameInfo,
    payload: [u8; PROMISCUOUS_MAX_PAYLOAD_PREFIX],
    len: u16,
}

impl PromiscuousRxFrame {
    pub fn new(info: PromiscuousFrameInfo, payload: &[u8]) -> Result<Self, DriverError> {
        if payload.len() > PROMISCUOUS_MAX_PAYLOAD_PREFIX {
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
}

pub(crate) struct EspNowPeerTable<const N: usize> {
    peers: [Option<EspNowPeer>; N],
    len: usize,
}

impl<const N: usize> EspNowPeerTable<N> {
    pub const fn new() -> Self {
        Self {
            peers: [const { None }; N],
            len: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub fn contains(&self, peer_addr: &[u8; 6]) -> bool {
        self.peers
            .iter()
            .flatten()
            .any(|peer| peer.peer_addr() == peer_addr)
    }

    pub fn insert(&mut self, peer: EspNowPeer) -> Result<bool, DriverError> {
        for slot in &mut self.peers {
            if let Some(existing) = slot {
                if existing.peer_addr() == peer.peer_addr() {
                    let changed = *existing != peer;
                    *existing = peer;
                    return Ok(changed);
                }
            }
        }

        for slot in &mut self.peers {
            if slot.is_none() {
                *slot = Some(peer);
                self.len += 1;
                return Ok(true);
            }
        }

        Err(DriverError::PeerOverflow)
    }

    pub fn remove(&mut self, peer_addr: &[u8; 6]) -> bool {
        for slot in &mut self.peers {
            if slot
                .as_ref()
                .is_some_and(|peer| peer.peer_addr() == peer_addr)
            {
                *slot = None;
                self.len -= 1;
                return true;
            }
        }

        false
    }
}

pub(crate) struct BoundedQueue<T: Copy, const N: usize> {
    entries: [Option<T>; N],
    head: usize,
    tail: usize,
    len: usize,
    drops: u32,
}

impl<T: Copy, const N: usize> BoundedQueue<T, N> {
    pub const fn new() -> Self {
        Self {
            entries: [const { None }; N],
            head: 0,
            tail: 0,
            len: 0,
            drops: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn drops(&self) -> u32 {
        self.drops
    }

    pub fn try_push(&mut self, record: T) -> Result<(), DriverError> {
        if N == 0 || self.len == N {
            self.drops = self.drops.wrapping_add(1);
            return Err(DriverError::PacketOverflow);
        }

        self.entries[self.head] = Some(record);
        self.head = (self.head + 1) % N;
        self.len += 1;
        Ok(())
    }

    pub fn pop(&mut self) -> Option<T> {
        if N == 0 || self.len == 0 {
            return None;
        }

        let record = self.entries[self.tail].take();
        self.tail = (self.tail + 1) % N;
        self.len -= 1;
        record
    }
}
