#[cfg(all(feature = "wifi-wps", any(target_arch = "xtensa", test)))]
use super::WpsPin;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkState {
    Down,
    Up,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// The SDK double deliberately omits native-only event sources.
#[cfg_attr(test, allow(dead_code))]
#[cfg(any(target_arch = "xtensa", test))]
pub(crate) enum WifiEvent {
    ScanDone {
        status: u32,
    },
    StaStarted,
    StaStopped,
    StaConnected,
    StaDisconnected {
        reason: Option<u8>,
    },
    ApStarted,
    ApStopped,
    #[cfg(feature = "wifi-wps")]
    WpsSucceeded,
    #[cfg(feature = "wifi-wps")]
    WpsFailed {
        reason: Option<u32>,
    },
    #[cfg(feature = "wifi-wps")]
    WpsTimedOut,
    #[cfg(feature = "wifi-wps")]
    WpsPin {
        pin: WpsPin,
    },
    #[cfg(feature = "wifi-smartconfig")]
    SmartConfigScanDone,
    #[cfg(feature = "wifi-smartconfig")]
    SmartConfigFoundChannel,
    #[cfg(feature = "wifi-smartconfig")]
    SmartConfigGotCredentials,
    #[cfg(feature = "wifi-smartconfig")]
    SmartConfigSendAckDone,
}
