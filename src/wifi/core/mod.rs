//! Strong-typestate, caller-owned WiFi core API.

mod backend;
mod capability;
mod config;
mod dhcp;
mod diag;
mod driver;
mod error;
mod event;
mod feature;
mod interface;
mod link;
mod locked;
mod packet;
mod resources;
mod session;
mod state;
mod waker;

#[cfg(not(target_arch = "xtensa"))]
mod host_critical;

#[cfg(feature = "wifi")]
mod target_callback;

#[cfg(feature = "wifi")]
mod target_map;

#[cfg(feature = "wifi")]
mod target;

pub use backend::{
    Deadline, RawMode, WifiApBackend, WifiBackend, WifiFeatureBackend, WifiLifecycleBackend,
    WifiPacketBackend, WifiRuntimeBackend, WifiScanBackend, WifiStaBackend,
};
pub use capability::{HasAp, HasSta};
pub use config::{
    AccessPointConfig, AccessPointRecord, ApClientLimit, ApStationInfo, BeaconInterval, Channel,
    ClientConfig, Credential, InitConfig, Ipv4Addr, MixedConfig, Passphrase, ScanOptions,
    ScanTimings, ScanType, SmartConfigTimeout, Ssid, StaticIpv4Config, StationPowerSave, WpsPin,
};
pub use dhcp::{
    DhcpLease, DhcpMessageType, DhcpReply, DhcpServer, DhcpServerConfig, DhcpServerError,
};
#[cfg(feature = "wifi-debug-trace")]
pub use diag::{
    DeliverDataDebugSnapshot, RunnerDebugSnapshot, RxCallbackDebugSnapshot, TxDebugSnapshot,
};
pub use driver::WifiDriver;
pub use error::{
    ConfigError, ConnectError, DisconnectError, DriverError, InitError, StaticIpv4Error,
};
pub use event::{EventBroker, EventCursor, LinkState, WifiEvent, WifiEventKind};
pub use feature::{
    EspNowAccessPointPeerConfig, EspNowInterface, EspNowPeer, EspNowPeerConfig, EspNowRxFrame,
    EspNowSendStatus, EspNowStationPeerConfig, EspNowTxStatus, PromiscuousControlFilter,
    PromiscuousControlSubtype, PromiscuousFilter, PromiscuousFrameInfo, PromiscuousPacketType,
    PromiscuousRxFrame, SmartConfigCredentials, SmartConfigStartConfig, SmartConfigType, WpsConfig,
    WpsFactoryInfo, WpsMode, ESP_NOW_KEY_LEN, ESP_NOW_MAX_PAYLOAD, PROMISCUOUS_MAX_PAYLOAD_PREFIX,
    SMARTCONFIG_KEY_LEN,
};
pub use interface::{ApHandle, ApLink, StaHandle, StaLink};
pub use link::WifiDataLink;
pub use packet::{RawRxRecord, ReleaseToken, RxPushError, RxRing};
pub use resources::WifiResources;
pub use session::{EspNowSession, PromiscuousSession, SmartConfigSession, StaSession, WpsSession};
pub use state::{ApMode, Configured, Initialized, MixedMode, StaMode, Started, Uninitialized};

#[cfg(feature = "wifi")]
pub use target::Esp8266TargetBackend;
#[cfg(all(feature = "wifi", feature = "wifi-debug-trace"))]
pub use target::{deliver_data_debug_snapshot, sta_rx_callback_debug_snapshot, tx_debug_snapshot};

#[cfg(all(feature = "wifi-embassy-net", feature = "wifi-debug-trace"))]
pub(crate) use diag::RunnerDebug;
