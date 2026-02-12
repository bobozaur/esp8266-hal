//! Error types for the ESP8266 WiFi driver.

use core::fmt;
use esp8266_wifi_sys as sys;

/// A raw error code returned by the ESP8266 RTOS SDK.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct EspError(pub sys::esp_err_t);

impl EspError {
    /// Wrap a raw SDK return value, returning `Ok(())` for `ESP_OK`.
    #[inline]
    pub fn check(code: sys::esp_err_t) -> Result<(), Self> {
        if code == sys::ESP_OK {
            Ok(())
        } else {
            Err(Self(code))
        }
    }

    /// The raw numeric error code.
    #[inline]
    pub fn code(self) -> sys::esp_err_t {
        self.0
    }
}

impl fmt::Debug for EspError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EspError(0x{:04X})", self.0)
    }
}

impl fmt::Display for EspError {
    #[inline(never)]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self.0 {
            sys::ESP_ERR_NO_MEM => "out of memory",
            sys::ESP_ERR_INVALID_ARG => "invalid argument",
            sys::ESP_ERR_INVALID_STATE => "invalid state",
            sys::ESP_ERR_NOT_FOUND => "not found",
            sys::ESP_ERR_NOT_SUPPORTED => "not supported",
            sys::ESP_ERR_TIMEOUT => "timeout",
            sys::ESP_ERR_WIFI_NOT_INIT => "WiFi not initialized",
            sys::ESP_ERR_WIFI_NOT_STARTED => "WiFi not started",
            sys::ESP_ERR_WIFI_NOT_STOPPED => "WiFi not stopped",
            sys::ESP_ERR_WIFI_IF => "invalid WiFi interface",
            sys::ESP_ERR_WIFI_MODE => "invalid WiFi mode",
            sys::ESP_ERR_WIFI_STATE => "WiFi state error",
            sys::ESP_ERR_WIFI_CONN => "WiFi connection error",
            sys::ESP_ERR_WIFI_NVS => "WiFi NVS error",
            sys::ESP_ERR_WIFI_MAC => "invalid MAC address",
            sys::ESP_ERR_WIFI_SSID => "invalid SSID",
            sys::ESP_ERR_WIFI_PASSWORD => "invalid password",
            sys::ESP_ERR_WIFI_TIMEOUT => "WiFi timeout",
            sys::ESP_ERR_WIFI_WAKE_FAIL => "WiFi wake failed",
            sys::ESP_ERR_WIFI_WOULD_BLOCK => "would block",
            sys::ESP_ERR_WIFI_NOT_CONNECT => "not connected",
            sys::ESP_FAIL => "general failure",
            _ => return write!(f, "ESP error 0x{:04X}", self.0),
        };
        f.write_str(msg)
    }
}

/// High-level WiFi error.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum WifiError {
    /// The WiFi subsystem has not been initialized.
    NotInitialized,
    /// An SDK function returned an error code.
    Internal(EspError),
    /// The station disconnected or failed to connect, with the reason.
    Disconnected(crate::wifi::event::DisconnectReason),
    /// The current WiFi mode does not support the requested operation.
    WrongMode,
    /// A supplied parameter is invalid (SSID too long, bad channel, etc.).
    InvalidArgument,
    /// A buffer was too small for the requested data.
    BufferTooSmall,
    /// The operation timed out.
    Timeout,
}

// Manual Debug impl to avoid a large derived match that triggers an Xtensa
// LLVM register-spill codegen bug when compiled with LTO.
impl fmt::Debug for WifiError {
    #[inline(never)]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInitialized => f.write_str("NotInitialized"),
            Self::Internal(e) => f.debug_tuple("Internal").field(e).finish(),
            Self::Disconnected(reason) => f.debug_tuple("Disconnected").field(reason).finish(),
            Self::WrongMode => f.write_str("WrongMode"),
            Self::InvalidArgument => f.write_str("InvalidArgument"),
            Self::BufferTooSmall => f.write_str("BufferTooSmall"),
            Self::Timeout => f.write_str("Timeout"),
        }
    }
}

impl fmt::Display for WifiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInitialized => f.write_str("WiFi not initialized"),
            Self::Internal(e) => write!(f, "internal WiFi error: {e}"),
            Self::Disconnected(reason) => write!(f, "WiFi disconnected: {reason:?}"),
            Self::WrongMode => f.write_str("wrong WiFi mode for this operation"),
            Self::InvalidArgument => f.write_str("invalid argument"),
            Self::BufferTooSmall => f.write_str("buffer too small"),
            Self::Timeout => f.write_str("operation timed out"),
        }
    }
}

impl From<EspError> for WifiError {
    fn from(e: EspError) -> Self {
        match e.0 {
            sys::ESP_ERR_WIFI_NOT_INIT => Self::NotInitialized,
            sys::ESP_ERR_WIFI_MODE => Self::WrongMode,
            sys::ESP_ERR_INVALID_ARG
            | sys::ESP_ERR_WIFI_SSID
            | sys::ESP_ERR_WIFI_PASSWORD
            | sys::ESP_ERR_WIFI_MAC => Self::InvalidArgument,
            sys::ESP_ERR_WIFI_TIMEOUT | sys::ESP_ERR_TIMEOUT => Self::Timeout,
            sys::ESP_ERR_WIFI_NOT_CONNECT => {
                Self::Disconnected(crate::wifi::event::DisconnectReason::Unspecified)
            }
            _ => Self::Internal(e),
        }
    }
}
