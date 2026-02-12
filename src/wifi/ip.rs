//! IPv4 address types used across the crate.

use core::fmt;

/// An IPv4 address.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Ipv4Addr(pub [u8; 4]);

impl Ipv4Addr {
    pub const UNSPECIFIED: Self = Self([0, 0, 0, 0]);

    /// Create from individual octets.
    pub const fn new(a: u8, b: u8, c: u8, d: u8) -> Self {
        Self([a, b, c, d])
    }

    /// Create from a raw `u32` in network byte order.
    #[allow(dead_code)]
    pub(crate) fn from_raw(raw: u32) -> Self {
        Self(raw.to_ne_bytes())
    }

    /// Convert to raw `u32` in network byte order.
    #[allow(dead_code)]
    pub(crate) fn to_raw(self) -> u32 {
        u32::from_ne_bytes(self.0)
    }

    /// The four octets of the address.
    pub fn octets(&self) -> [u8; 4] {
        self.0
    }

    /// Convert to a smoltcp IPv4 address.
    pub fn to_smoltcp(self) -> smoltcp::wire::Ipv4Address {
        smoltcp::wire::Ipv4Address::from(self.0)
    }

    /// Create from a smoltcp IPv4 address.
    pub fn from_smoltcp(addr: smoltcp::wire::Ipv4Address) -> Self {
        Self(addr.octets())
    }
}

impl fmt::Debug for Ipv4Addr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}.{}", self.0[0], self.0[1], self.0[2], self.0[3])
    }
}

impl fmt::Display for Ipv4Addr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

/// IPv4 configuration for a network interface.
#[derive(Debug, Clone, Copy)]
pub struct Ipv4Info {
    pub ip: Ipv4Addr,
    pub netmask: Ipv4Addr,
    pub gateway: Ipv4Addr,
}
