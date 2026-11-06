//! ESP8266 Wi-Fi configuration, radio control and bounded frame interfaces.
pub(crate) mod allocator;
mod config;
#[cfg(any(target_arch = "xtensa", test))]
mod driver;
mod error;
mod event;
mod feature;
#[cfg(all(test, not(target_arch = "xtensa")))]
mod host_critical;
#[cfg(any(target_arch = "xtensa", test))]
mod interface;
mod link;
#[cfg(any(target_arch = "xtensa", test))]
mod locked;
#[cfg(any(target_arch = "xtensa", test))]
mod packet;
#[cfg(any(target_arch = "xtensa", test))]
mod queue;
#[cfg(any(target_arch = "xtensa", test))]
mod resources;
#[cfg(all(
    any(target_arch = "xtensa", test),
    any(
        feature = "wifi-esp-now",
        feature = "wifi-promiscuous",
        feature = "wifi-wps",
        feature = "wifi-smartconfig"
    )
))]
mod session;
#[cfg(any(target_arch = "xtensa", test))]
mod state;
#[cfg(target_arch = "xtensa")]
mod target;
#[cfg(all(test, not(target_arch = "xtensa")))]
#[path = "test_sdk.rs"]
pub(crate) mod target;
#[cfg(any(target_arch = "xtensa", test))]
mod target_callback;
#[cfg(any(target_arch = "xtensa", test))]
mod target_map;
#[cfg(any(target_arch = "xtensa", test))]
mod waker;

pub use allocator::WifiAllocator;
#[cfg(feature = "wifi-smartconfig")]
pub use config::SmartConfigTimeout;
#[cfg(feature = "wifi-wps")]
pub use config::WpsPin;
pub use config::{
    AccessPointConfig, AccessPointRecord, ApClientLimit, ApCredential, ApStationInfo,
    AuthenticationMethod, BeaconInterval, Channel, ClientConfig, Configuration, InitConfig,
    Ipv4Addr, MixedConfig, Passphrase, QueueConfig, ScanOptions, ScanTimings, ScanType, Ssid,
    StationCredential, StationPowerSave,
};
#[cfg(feature = "wifi-enterprise")]
pub use config::{
    EnterpriseIdentity, EnterprisePassword, EnterpriseUsername, Wpa2EnterpriseClientCertificate,
    Wpa2EnterpriseCredentials, Wpa2EnterpriseMaterialFormat,
};
#[cfg(any(target_arch = "xtensa", test))]
pub use driver::{Controller, Wifi};
pub use error::{ConfigError, DriverError, InitError};
pub use event::LinkState;
#[cfg(any(target_arch = "xtensa", test))]
pub(crate) use event::WifiEvent;
#[cfg(feature = "wifi-esp-now")]
pub use feature::{
    EspNowPeerConfig, EspNowRxFrame, EspNowSendStatus, ESP_NOW_KEY_LEN, ESP_NOW_MAX_PAYLOAD,
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
#[cfg(any(target_arch = "xtensa", test))]
pub use interface::{Interface, Interfaces};
#[cfg(any(target_arch = "xtensa", test))]
pub(crate) use link::validate_frame_len;
pub use link::{MAX_ETHERNET_FRAME_SIZE, MIN_IPV4_ETHERNET_FRAME_SIZE};
#[cfg(target_arch = "xtensa")]
pub(crate) use packet::RawRxRecord;
#[cfg(all(any(target_arch = "xtensa", test), feature = "wifi-smoltcp"))]
pub(crate) use resources::RxPacket;
#[cfg(all(any(target_arch = "xtensa", test), feature = "wifi-esp-now"))]
pub use session::EspNowSession;
#[cfg(all(any(target_arch = "xtensa", test), feature = "wifi-promiscuous"))]
pub use session::PromiscuousSession;
#[cfg(all(any(target_arch = "xtensa", test), feature = "wifi-smartconfig"))]
pub use session::SmartConfigSession;
#[cfg(all(any(target_arch = "xtensa", test), feature = "wifi-wps"))]
pub use session::WpsSession;
#[cfg(any(target_arch = "xtensa", test))]
pub(crate) use state::{Deadline, RawMode};
