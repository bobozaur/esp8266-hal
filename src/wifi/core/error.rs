#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigError {
    SsidEmpty,
    SsidTooLong,
    SsidContainsNul,
    PassphraseTooShort,
    PassphraseTooLong,
    PassphraseInvalidCharacter,
    PskInvalidHex,
    #[cfg(feature = "wifi-enterprise")]
    EnterpriseIdentityEmpty,
    #[cfg(feature = "wifi-enterprise")]
    EnterpriseIdentityTooLong,
    #[cfg(feature = "wifi-enterprise")]
    EnterpriseUsernameEmpty,
    #[cfg(feature = "wifi-enterprise")]
    EnterpriseUsernameTooLong,
    #[cfg(feature = "wifi-enterprise")]
    EnterprisePasswordEmpty,
    #[cfg(feature = "wifi-enterprise")]
    EnterprisePasswordTooLong,
    InvalidChannel(u8),
    InvalidApClientLimit(u8),
    InvalidBeaconInterval(u16),
    InvalidRxBufferCount(u8),
    InvalidRxPacketCount(u8),
    InvalidTxBufferCount(u8),
    #[cfg(feature = "wifi-smartconfig")]
    InvalidSmartConfigTimeout(u8),
    #[cfg(feature = "wifi-wps")]
    InvalidWpsPin,
    #[cfg(feature = "wifi-wps")]
    WpsManufacturerTooLong,
    #[cfg(feature = "wifi-wps")]
    WpsModelNumberTooLong,
    #[cfg(feature = "wifi-wps")]
    WpsModelNameTooLong,
    #[cfg(feature = "wifi-wps")]
    WpsDeviceNameTooLong,
    #[cfg(feature = "wifi-wps")]
    WpsFactoryInfoContainsNul,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InitError {
    AlreadyInitialized,
    /// Native radio buffers already use different dimensions for this boot.
    BufferConfigurationLocked,
    BackendUnavailable,
    OutOfMemory,
    InvalidConfiguration,
    Sdk(i32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DriverError {
    Timeout,
    Disconnected { reason: Option<u8> },
    EventOverflow,
    PacketOverflow,
    PeerOverflow,
    PayloadTooLong,
    InvalidFrameSize,
    OutOfMemory,
    NotStarted,
    InvalidState,
    Busy,
    WouldBlock,
    Sdk(i32),
}
