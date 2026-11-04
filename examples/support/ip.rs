use core::net::Ipv4Addr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StaticIpv4Error {
    InvalidPrefix,
    UnspecifiedAddress,
    UnspecifiedGateway,
    GatewayOutsideSubnet,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StaticIpv4Config {
    address: Ipv4Addr,
    prefix_len: u8,
    gateway: Option<Ipv4Addr>,
}

impl StaticIpv4Config {
    pub fn new(
        address: Ipv4Addr,
        prefix_len: u8,
        gateway: Option<Ipv4Addr>,
    ) -> Result<Self, StaticIpv4Error> {
        if prefix_len > 32 {
            return Err(StaticIpv4Error::InvalidPrefix);
        }
        if address.is_unspecified() {
            return Err(StaticIpv4Error::UnspecifiedAddress);
        }
        if let Some(gateway) = gateway {
            if gateway.is_unspecified() {
                return Err(StaticIpv4Error::UnspecifiedGateway);
            }
            if !same_ipv4_subnet(address, gateway, prefix_len) {
                return Err(StaticIpv4Error::GatewayOutsideSubnet);
            }
        }

        Ok(Self {
            address,
            prefix_len,
            gateway,
        })
    }

    pub const fn address(self) -> Ipv4Addr {
        self.address
    }

    pub const fn prefix_len(self) -> u8 {
        self.prefix_len
    }

    pub const fn gateway(self) -> Option<Ipv4Addr> {
        self.gateway
    }
}

fn same_ipv4_subnet(left: Ipv4Addr, right: Ipv4Addr, prefix_len: u8) -> bool {
    let mask = if prefix_len == 0 {
        0
    } else {
        u32::MAX << (32 - u32::from(prefix_len))
    };

    u32::from_be_bytes(left.octets()) & mask == u32::from_be_bytes(right.octets()) & mask
}
