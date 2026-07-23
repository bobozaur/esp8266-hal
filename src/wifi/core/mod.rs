//! Strong-typestate, caller-owned WiFi core API.

mod backend;
mod capability;
mod config;
mod dhcp;
mod driver;
mod error;
mod event;
mod feature;
mod interface;
mod link;
mod locked;
#[cfg(any(
    feature = "wifi-embassy-net",
    all(feature = "wifi", target_arch = "xtensa"),
    test
))]
mod neighbor;
#[cfg_attr(not(target_arch = "xtensa"), allow(dead_code))]
mod packet;
mod resources;
mod session;
mod state;
mod waker;

#[cfg(feature = "wifi-embassy-net")]
#[path = "../embassy_net.rs"]
pub mod embassy_net;

#[cfg(not(target_arch = "xtensa"))]
mod host_critical;

#[cfg(all(
    feature = "wifi",
    feature = "rt",
    feature = "interrupt",
    target_arch = "xtensa"
))]
mod target_callback;

#[cfg(all(
    feature = "wifi",
    feature = "rt",
    feature = "interrupt",
    target_arch = "xtensa"
))]
mod target_map;

#[cfg(all(
    feature = "wifi",
    feature = "rt",
    feature = "interrupt",
    target_arch = "xtensa"
))]
mod target;

pub use backend::{
    Deadline, RawMode, WifiApBackend, WifiBackend, WifiFeatureBackend, WifiLifecycleBackend,
    WifiPacketBackend, WifiRuntimeBackend, WifiScanBackend, WifiStaBackend,
};
pub use capability::{HasAp, HasSta};
#[cfg(feature = "wifi-smartconfig")]
pub use config::SmartConfigTimeout;
#[cfg(feature = "wifi-wps")]
pub use config::WpsPin;
pub use config::{
    AccessPointConfig, AccessPointRecord, ApClientLimit, ApCredential, ApStationInfo,
    AuthenticationMethod, BeaconInterval, Channel, ClientConfig, InitConfig, Ipv4Addr, MixedConfig,
    Passphrase, ScanOptions, ScanTimings, ScanType, Ssid, StaticIpv4Config, StationCredential,
    StationPowerSave,
};
#[cfg(feature = "wifi-enterprise")]
pub use config::{
    EnterpriseIdentity, EnterprisePassword, EnterpriseUsername, Wpa2EnterpriseClientCertificate,
    Wpa2EnterpriseCredentials, Wpa2EnterpriseMaterialFormat,
};
pub use dhcp::{
    DhcpLease, DhcpMessageType, DhcpReply, DhcpServer, DhcpServerConfig, DhcpServerError,
};
pub use driver::WifiDriver;
pub use error::{
    ConfigError, ConnectError, DisconnectError, DriverError, InitError, StaticIpv4Error,
};
#[cfg(any(feature = "wifi-smartconfig", feature = "wifi-wps"))]
pub(crate) use event::EventCursor;
pub use event::LinkState;
#[cfg(any(
    target_arch = "xtensa",
    test,
    feature = "wifi-smartconfig",
    feature = "wifi-wps"
))]
pub(crate) use event::WifiEvent;
#[cfg(feature = "wifi-esp-now")]
pub use feature::{
    EspNowAccessPointPeerConfig, EspNowPeerConfig, EspNowRxFrame, EspNowSendStatus,
    EspNowStationPeerConfig, ESP_NOW_KEY_LEN, ESP_NOW_MAX_PAYLOAD,
};
#[cfg(feature = "wifi-promiscuous")]
pub use feature::{
    PromiscuousControlFilter, PromiscuousControlSubtype, PromiscuousFilter, PromiscuousFrameInfo,
    PromiscuousPacketType, PromiscuousRxFrame,
};
#[cfg(feature = "wifi-smartconfig")]
pub use feature::{
    SmartConfigCredentials, SmartConfigEvent, SmartConfigStartConfig, SmartConfigType,
    SMARTCONFIG_KEY_LEN,
};
#[cfg(feature = "wifi-wps")]
pub use feature::{WpsConfig, WpsEvent, WpsFactoryInfo, WpsMode};
pub use interface::{ApHandle, ApLink, MixedApLink, MixedLink, MixedStaLink, StaHandle, StaLink};
pub(crate) use link::validate_frame_len;
#[cfg(all(feature = "wifi", target_arch = "xtensa"))]
pub(crate) use link::WifiRawDataLink;
pub use link::{WifiDataLink, MAX_ETHERNET_FRAME_SIZE, MIN_IPV4_ETHERNET_FRAME_SIZE};
#[cfg(any(
    all(
        feature = "wifi",
        feature = "rt",
        feature = "interrupt",
        target_arch = "xtensa"
    ),
    test
))]
pub(crate) use packet::RawRxRecord;
pub use resources::WifiResources;
#[cfg(feature = "wifi-esp-now")]
pub use session::EspNowSession;
#[cfg(feature = "wifi-promiscuous")]
pub use session::PromiscuousSession;
#[cfg(feature = "wifi-smartconfig")]
pub use session::SmartConfigSession;
#[cfg(feature = "wifi-wps")]
pub use session::WpsSession;
pub use state::{ApMode, Configured, Initialized, MixedMode, StaMode, Started, Uninitialized};

#[cfg(any(
    feature = "wifi-embassy-net",
    all(
        feature = "wifi",
        feature = "rt",
        feature = "interrupt",
        target_arch = "xtensa"
    )
))]
pub(crate) use neighbor::EthernetNeighborHints;
#[cfg(all(
    feature = "wifi",
    feature = "rt",
    feature = "interrupt",
    target_arch = "xtensa"
))]
pub(crate) use neighbor::{EthernetNeighborHint, ETHERNET_ARP_IPV4_FRAME_LEN};
#[cfg(all(
    feature = "wifi",
    feature = "rt",
    feature = "interrupt",
    target_arch = "xtensa"
))]
pub use target::Esp8266TargetBackend;
