//! Minimal error wrapper for the ESP8266 SDK ABI.

use core::fmt;
use esp8266_wifi_sys as sys;

/// A raw error code returned by the pinned ESP8266 SDK.
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
