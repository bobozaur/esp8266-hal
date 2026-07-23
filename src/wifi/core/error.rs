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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StaticIpv4Error {
    InvalidPrefix,
    UnspecifiedAddress,
    UnspecifiedGateway,
    GatewayOutsideSubnet,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InitError {
    AlreadyInitialized,
    BackendUnavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DriverError {
    Timeout,
    StaStartTimeout,
    Disconnected,
    EventOverflow,
    PacketOverflow,
    PeerOverflow,
    PayloadTooLong,
    InvalidFrameSize,
    OutOfMemory,
    NotStarted,
    WouldBlock,
    BackendRejected,
    BackendError(i32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectError {
    Timeout,
    Disconnected,
    BackendRejected,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisconnectError {
    BackendRejected,
}
