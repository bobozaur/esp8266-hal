use super::Ipv4Addr;

const DHCP_MIN_LEN: usize = 240;
const DHCP_MAGIC: [u8; 4] = 0x6382_5363u32.to_be_bytes();
const DHCP_HTYPE_ETHERNET: u8 = 1;
const DHCP_HLEN_ETHERNET: u8 = 6;
const DHCP_FLAG_BROADCAST: u16 = 0x8000;

const OPT_PAD: u8 = 0;
const OPT_SUBNET_MASK: u8 = 1;
const OPT_ROUTER: u8 = 3;
const OPT_DOMAIN_NAME_SERVER: u8 = 6;
const OPT_REQUESTED_IP: u8 = 50;
const OPT_IP_LEASE_TIME: u8 = 51;
const OPT_MESSAGE_TYPE: u8 = 53;
const OPT_SERVER_IDENTIFIER: u8 = 54;
const OPT_RENEWAL_TIME: u8 = 58;
const OPT_REBINDING_TIME: u8 = 59;
const OPT_END: u8 = 255;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DhcpServerError {
    InvalidPacket,
    InvalidPool,
    LeasePoolFull,
    BufferTooSmall,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DhcpMessageType {
    Discover,
    Offer,
    Request,
    Ack,
    Nak,
    Release,
}

impl DhcpMessageType {
    const fn from_option(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Discover),
            2 => Some(Self::Offer),
            3 => Some(Self::Request),
            5 => Some(Self::Ack),
            6 => Some(Self::Nak),
            7 => Some(Self::Release),
            _ => None,
        }
    }

    const fn option_value(self) -> u8 {
        match self {
            Self::Discover => 1,
            Self::Offer => 2,
            Self::Request => 3,
            Self::Ack => 5,
            Self::Nak => 6,
            Self::Release => 7,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DhcpServerConfig {
    server_ip: Ipv4Addr,
    subnet_mask: Ipv4Addr,
    lease_start: Ipv4Addr,
    lease_count: u8,
    router: Option<Ipv4Addr>,
    dns_server: Option<Ipv4Addr>,
    lease_seconds: u32,
}

impl DhcpServerConfig {
    pub fn new(
        server_ip: Ipv4Addr,
        subnet_mask: Ipv4Addr,
        lease_start: Ipv4Addr,
        lease_count: u8,
    ) -> Result<Self, DhcpServerError> {
        let Some(prefix_len) = prefix_len_from_mask(subnet_mask) else {
            return Err(DhcpServerError::InvalidPool);
        };
        if server_ip == Ipv4Addr::UNSPECIFIED
            || lease_start == Ipv4Addr::UNSPECIFIED
            || lease_count == 0
            || prefix_len > 30
        {
            return Err(DhcpServerError::InvalidPool);
        }

        let start = ipv4_to_u32(lease_start);
        let Some(end) = start.checked_add(u32::from(lease_count) - 1) else {
            return Err(DhcpServerError::InvalidPool);
        };

        let server = ipv4_to_u32(server_ip);
        let mask = ipv4_to_u32(subnet_mask);
        let network = server & mask;
        let broadcast = network | !mask;

        if start & mask != network
            || end & mask != network
            || start <= network
            || end >= broadcast
            || (start <= server && server <= end)
        {
            return Err(DhcpServerError::InvalidPool);
        }

        Ok(Self {
            server_ip,
            subnet_mask,
            lease_start,
            lease_count,
            router: Some(server_ip),
            dns_server: None,
            lease_seconds: 24 * 60 * 60,
        })
    }

    #[must_use]
    pub const fn router(mut self, router: Ipv4Addr) -> Self {
        self.router = Some(router);
        self
    }

    #[must_use]
    pub const fn no_router(mut self) -> Self {
        self.router = None;
        self
    }

    #[must_use]
    pub const fn dns_server(mut self, dns_server: Ipv4Addr) -> Self {
        self.dns_server = Some(dns_server);
        self
    }

    #[must_use]
    pub const fn no_dns_server(mut self) -> Self {
        self.dns_server = None;
        self
    }

    #[must_use]
    pub const fn lease_seconds(mut self, seconds: u32) -> Self {
        self.lease_seconds = seconds;
        self
    }

    pub const fn server_ip(self) -> Ipv4Addr {
        self.server_ip
    }

    pub const fn subnet_mask(self) -> Ipv4Addr {
        self.subnet_mask
    }

    pub const fn lease_start(self) -> Ipv4Addr {
        self.lease_start
    }

    pub const fn lease_count(self) -> u8 {
        self.lease_count
    }

    pub const fn router_ip(self) -> Option<Ipv4Addr> {
        self.router
    }

    pub const fn dns_server_ip(self) -> Option<Ipv4Addr> {
        self.dns_server
    }

    pub const fn lease_duration_seconds(self) -> u32 {
        self.lease_seconds
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DhcpLease {
    mac: [u8; 6],
    ip: Ipv4Addr,
    expires_at_seconds: u64,
}

impl DhcpLease {
    pub const fn mac(self) -> [u8; 6] {
        self.mac
    }

    pub const fn ip(self) -> Ipv4Addr {
        self.ip
    }

    pub const fn expires_at_seconds(self) -> u64 {
        self.expires_at_seconds
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DhcpReply {
    len: usize,
    message_type: DhcpMessageType,
    leased_ip: Ipv4Addr,
    client_mac: [u8; 6],
    broadcast: bool,
}

impl DhcpReply {
    pub const fn len(self) -> usize {
        self.len
    }

    pub const fn is_empty(self) -> bool {
        self.len == 0
    }

    pub const fn message_type(self) -> DhcpMessageType {
        self.message_type
    }

    pub const fn leased_ip(self) -> Ipv4Addr {
        self.leased_ip
    }

    pub const fn client_mac(self) -> [u8; 6] {
        self.client_mac
    }

    pub const fn broadcast(self) -> bool {
        self.broadcast
    }
}

pub struct DhcpServer<const LEASES: usize> {
    config: DhcpServerConfig,
    leases: [Option<DhcpLease>; LEASES],
}

impl<const LEASES: usize> DhcpServer<LEASES> {
    pub const fn new(config: DhcpServerConfig) -> Self {
        Self {
            config,
            leases: [const { None }; LEASES],
        }
    }

    pub const fn config(&self) -> DhcpServerConfig {
        self.config
    }

    pub fn leases(&self) -> impl Iterator<Item = DhcpLease> + '_ {
        self.leases.iter().flatten().copied()
    }

    pub fn handle(
        &mut self,
        now_seconds: u64,
        request: &[u8],
        response: &mut [u8],
    ) -> Result<Option<DhcpReply>, DhcpServerError> {
        self.expire_leases(now_seconds);
        let request = DhcpRequest::parse(request)?;
        match request.message_type {
            DhcpMessageType::Discover => {
                let lease = self.lease_or_allocate(request.client_mac, now_seconds)?;
                let len = render_reply(
                    self.config,
                    &request,
                    DhcpMessageType::Offer,
                    lease.ip,
                    response,
                )?;
                Ok(Some(DhcpReply {
                    len,
                    message_type: DhcpMessageType::Offer,
                    leased_ip: lease.ip,
                    client_mac: request.client_mac,
                    broadcast: request.broadcast(),
                }))
            }
            DhcpMessageType::Request => {
                if request
                    .server_identifier
                    .is_some_and(|server| server != self.config.server_ip)
                {
                    return Ok(None);
                }

                let Some(requested_ip) = request
                    .requested_ip
                    .or_else(|| self.lookup_ip(request.client_mac))
                else {
                    let len = render_reply(
                        self.config,
                        &request,
                        DhcpMessageType::Nak,
                        Ipv4Addr::UNSPECIFIED,
                        response,
                    )?;
                    return Ok(Some(DhcpReply {
                        len,
                        message_type: DhcpMessageType::Nak,
                        leased_ip: Ipv4Addr::UNSPECIFIED,
                        client_mac: request.client_mac,
                        broadcast: true,
                    }));
                };

                if !self.ip_in_pool(requested_ip)
                    || self.ip_is_leased_to_other(requested_ip, request.client_mac)
                {
                    let len = render_reply(
                        self.config,
                        &request,
                        DhcpMessageType::Nak,
                        Ipv4Addr::UNSPECIFIED,
                        response,
                    )?;
                    return Ok(Some(DhcpReply {
                        len,
                        message_type: DhcpMessageType::Nak,
                        leased_ip: Ipv4Addr::UNSPECIFIED,
                        client_mac: request.client_mac,
                        broadcast: true,
                    }));
                }

                let lease = self.assign_lease(request.client_mac, requested_ip, now_seconds)?;
                let len = render_reply(
                    self.config,
                    &request,
                    DhcpMessageType::Ack,
                    lease.ip,
                    response,
                )?;
                Ok(Some(DhcpReply {
                    len,
                    message_type: DhcpMessageType::Ack,
                    leased_ip: lease.ip,
                    client_mac: request.client_mac,
                    broadcast: request.broadcast(),
                }))
            }
            DhcpMessageType::Release => {
                self.release(request.client_mac);
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    pub fn release(&mut self, mac: [u8; 6]) -> bool {
        let Some(slot) = self
            .leases
            .iter_mut()
            .find(|slot| slot.is_some_and(|lease| lease.mac == mac))
        else {
            return false;
        };
        *slot = None;
        true
    }

    fn expire_leases(&mut self, now_seconds: u64) {
        for lease in &mut self.leases {
            if lease.is_some_and(|lease| lease.expires_at_seconds <= now_seconds) {
                *lease = None;
            }
        }
    }

    fn lookup_ip(&self, mac: [u8; 6]) -> Option<Ipv4Addr> {
        self.leases
            .iter()
            .flatten()
            .find(|lease| lease.mac == mac)
            .map(|lease| lease.ip)
    }

    fn lease_or_allocate(
        &mut self,
        mac: [u8; 6],
        now_seconds: u64,
    ) -> Result<DhcpLease, DhcpServerError> {
        if let Some(ip) = self.lookup_ip(mac) {
            return self.assign_lease(mac, ip, now_seconds);
        }

        for offset in 0..self.config.lease_count {
            let ip = u32_to_ipv4(ipv4_to_u32(self.config.lease_start) + u32::from(offset));
            if !self.ip_is_leased(ip) {
                return self.assign_lease(mac, ip, now_seconds);
            }
        }

        Err(DhcpServerError::LeasePoolFull)
    }

    fn assign_lease(
        &mut self,
        mac: [u8; 6],
        ip: Ipv4Addr,
        now_seconds: u64,
    ) -> Result<DhcpLease, DhcpServerError> {
        let expires_at_seconds = now_seconds.saturating_add(u64::from(self.config.lease_seconds));
        if let Some(lease) = self
            .leases
            .iter_mut()
            .flatten()
            .find(|lease| lease.mac == mac)
        {
            lease.ip = ip;
            lease.expires_at_seconds = expires_at_seconds;
            return Ok(*lease);
        }

        let slot = self
            .leases
            .iter_mut()
            .find(|lease| lease.is_none())
            .ok_or(DhcpServerError::LeasePoolFull)?;
        let lease = DhcpLease {
            mac,
            ip,
            expires_at_seconds,
        };
        *slot = Some(lease);
        Ok(lease)
    }

    fn ip_in_pool(&self, ip: Ipv4Addr) -> bool {
        let raw = ipv4_to_u32(ip);
        let start = ipv4_to_u32(self.config.lease_start);
        raw >= start && raw < start + u32::from(self.config.lease_count)
    }

    fn ip_is_leased(&self, ip: Ipv4Addr) -> bool {
        self.leases.iter().flatten().any(|lease| lease.ip == ip)
    }

    fn ip_is_leased_to_other(&self, ip: Ipv4Addr, mac: [u8; 6]) -> bool {
        self.leases
            .iter()
            .flatten()
            .any(|lease| lease.ip == ip && lease.mac != mac)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DhcpRequest {
    message_type: DhcpMessageType,
    transaction_id: u32,
    flags: u16,
    client_mac: [u8; 6],
    requested_ip: Option<Ipv4Addr>,
    server_identifier: Option<Ipv4Addr>,
}

impl DhcpRequest {
    const fn broadcast(&self) -> bool {
        self.flags & DHCP_FLAG_BROADCAST != 0
    }

    fn parse(packet: &[u8]) -> Result<Self, DhcpServerError> {
        if packet.len() < DHCP_MIN_LEN
            || packet[0] != 1
            || packet[1] != DHCP_HTYPE_ETHERNET
            || packet[2] != DHCP_HLEN_ETHERNET
            || packet[236..240] != DHCP_MAGIC
        {
            return Err(DhcpServerError::InvalidPacket);
        }

        let mut message_type = None;
        let mut requested_ip = None;
        let mut server_identifier = None;
        let mut offset = DHCP_MIN_LEN;
        while offset < packet.len() {
            match packet[offset] {
                OPT_END => break,
                OPT_PAD => offset += 1,
                kind => {
                    if offset + 2 > packet.len() {
                        return Err(DhcpServerError::InvalidPacket);
                    }
                    let len = packet[offset + 1] as usize;
                    let start = offset + 2;
                    let end = start + len;
                    if end > packet.len() {
                        return Err(DhcpServerError::InvalidPacket);
                    }
                    let data = &packet[start..end];
                    match (kind, data.len()) {
                        (OPT_MESSAGE_TYPE, 1) => {
                            message_type = DhcpMessageType::from_option(data[0]);
                        }
                        (OPT_REQUESTED_IP, 4) => {
                            requested_ip = Some(Ipv4Addr::new(data[0], data[1], data[2], data[3]));
                        }
                        (OPT_SERVER_IDENTIFIER, 4) => {
                            server_identifier =
                                Some(Ipv4Addr::new(data[0], data[1], data[2], data[3]));
                        }
                        _ => {}
                    }
                    offset = end;
                }
            }
        }

        let mut client_mac = [0u8; 6];
        client_mac.copy_from_slice(&packet[28..34]);
        Ok(Self {
            message_type: message_type.ok_or(DhcpServerError::InvalidPacket)?,
            transaction_id: u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]),
            flags: u16::from_be_bytes([packet[10], packet[11]]),
            client_mac,
            requested_ip,
            server_identifier,
        })
    }
}

fn render_reply(
    config: DhcpServerConfig,
    request: &DhcpRequest,
    message_type: DhcpMessageType,
    leased_ip: Ipv4Addr,
    response: &mut [u8],
) -> Result<usize, DhcpServerError> {
    if response.len() < DHCP_MIN_LEN {
        return Err(DhcpServerError::BufferTooSmall);
    }

    response.fill(0);
    response[0] = 2;
    response[1] = DHCP_HTYPE_ETHERNET;
    response[2] = DHCP_HLEN_ETHERNET;
    response[4..8].copy_from_slice(&request.transaction_id.to_be_bytes());
    let flags = if request.flags & DHCP_FLAG_BROADCAST != 0 {
        DHCP_FLAG_BROADCAST
    } else {
        0
    };
    response[10..12].copy_from_slice(&flags.to_be_bytes());
    response[16..20].copy_from_slice(&leased_ip.octets());
    response[20..24].copy_from_slice(&config.server_ip.octets());
    response[28..34].copy_from_slice(&request.client_mac);
    response[236..240].copy_from_slice(&DHCP_MAGIC);

    let mut offset = DHCP_MIN_LEN;
    push_option(
        response,
        &mut offset,
        OPT_MESSAGE_TYPE,
        &[message_type.option_value()],
    )?;
    push_option(
        response,
        &mut offset,
        OPT_SERVER_IDENTIFIER,
        &config.server_ip.octets(),
    )?;

    if message_type != DhcpMessageType::Nak {
        push_option(
            response,
            &mut offset,
            OPT_SUBNET_MASK,
            &config.subnet_mask.octets(),
        )?;
        if let Some(router) = config.router {
            push_option(response, &mut offset, OPT_ROUTER, &router.octets())?;
        }
        if let Some(dns_server) = config.dns_server {
            push_option(
                response,
                &mut offset,
                OPT_DOMAIN_NAME_SERVER,
                &dns_server.octets(),
            )?;
        }
        push_option(
            response,
            &mut offset,
            OPT_IP_LEASE_TIME,
            &config.lease_seconds.to_be_bytes(),
        )?;
        push_option(
            response,
            &mut offset,
            OPT_RENEWAL_TIME,
            &renewal_seconds(config.lease_seconds).to_be_bytes(),
        )?;
        push_option(
            response,
            &mut offset,
            OPT_REBINDING_TIME,
            &rebinding_seconds(config.lease_seconds).to_be_bytes(),
        )?;
    }

    if offset >= response.len() {
        return Err(DhcpServerError::BufferTooSmall);
    }
    response[offset] = OPT_END;
    Ok(offset + 1)
}

fn push_option(
    packet: &mut [u8],
    offset: &mut usize,
    kind: u8,
    data: &[u8],
) -> Result<(), DhcpServerError> {
    if data.len() > u8::MAX as usize || *offset + 2 + data.len() > packet.len() {
        return Err(DhcpServerError::BufferTooSmall);
    }

    packet[*offset] = kind;
    packet[*offset + 1] = data.len() as u8;
    packet[*offset + 2..*offset + 2 + data.len()].copy_from_slice(data);
    *offset += 2 + data.len();
    Ok(())
}

fn ipv4_to_u32(ip: Ipv4Addr) -> u32 {
    u32::from_be_bytes(ip.octets())
}

fn u32_to_ipv4(raw: u32) -> Ipv4Addr {
    let octets = raw.to_be_bytes();
    Ipv4Addr::new(octets[0], octets[1], octets[2], octets[3])
}

fn prefix_len_from_mask(mask: Ipv4Addr) -> Option<u8> {
    let raw = ipv4_to_u32(mask);
    let prefix_len = raw.leading_ones() as u8;
    let expected = if prefix_len == 0 {
        0
    } else {
        u32::MAX << (32 - u32::from(prefix_len))
    };
    (raw == expected).then_some(prefix_len)
}

const fn renewal_seconds(lease_seconds: u32) -> u32 {
    lease_seconds / 2
}

const fn rebinding_seconds(lease_seconds: u32) -> u32 {
    lease_seconds - (lease_seconds / 8)
}
