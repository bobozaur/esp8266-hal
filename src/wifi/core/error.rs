#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigError {
    SsidTooLong,
    PassphraseTooShort,
    PassphraseTooLong,
    EnterpriseIdentityEmpty,
    EnterpriseIdentityTooLong,
    EnterpriseUsernameEmpty,
    EnterpriseUsernameTooLong,
    EnterprisePasswordEmpty,
    EnterprisePasswordTooLong,
    InvalidChannel(u8),
    InvalidApClientLimit(u8),
    InvalidBeaconInterval(u16),
    InvalidRxBufferCount(u8),
    InvalidRxPacketCount(u8),
    InvalidTxBufferCount(u8),
    InvalidSmartConfigTimeout(u8),
    InvalidWpsPin,
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
