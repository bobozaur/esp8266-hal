use esp8266_hal::wifi_core::{
    DhcpMessageType, DhcpServer, DhcpServerConfig, DhcpServerError, Ipv4Addr,
};

const DHCP_MIN_LEN: usize = 240;
const OPT_MESSAGE_TYPE: u8 = 53;
const OPT_REQUESTED_IP: u8 = 50;
const OPT_SERVER_IDENTIFIER: u8 = 54;
const OPT_SUBNET_MASK: u8 = 1;
const OPT_ROUTER: u8 = 3;
const OPT_DOMAIN_NAME_SERVER: u8 = 6;
const OPT_IP_LEASE_TIME: u8 = 51;
const OPT_RENEWAL_TIME: u8 = 58;
const OPT_REBINDING_TIME: u8 = 59;
const OPT_END: u8 = 255;

fn request_packet(
    message_type: u8,
    requested_ip: Option<Ipv4Addr>,
    server_id: Option<Ipv4Addr>,
) -> [u8; 320] {
    request_packet_with_flags(message_type, requested_ip, server_id, 0x8000)
}

fn request_packet_with_flags(
    message_type: u8,
    requested_ip: Option<Ipv4Addr>,
    server_id: Option<Ipv4Addr>,
    flags: u16,
) -> [u8; 320] {
    let mut packet = [0u8; 320];
    packet[0] = 1;
    packet[1] = 1;
    packet[2] = 6;
    packet[4..8].copy_from_slice(&0x1234_5678u32.to_be_bytes());
    packet[10..12].copy_from_slice(&flags.to_be_bytes());
    packet[28..34].copy_from_slice(&[2, 4, 6, 8, 10, 12]);
    packet[236..240].copy_from_slice(&0x6382_5363u32.to_be_bytes());

    let mut offset = DHCP_MIN_LEN;
    write_option(&mut packet, &mut offset, OPT_MESSAGE_TYPE, &[message_type]);
    if let Some(ip) = requested_ip {
        write_option(&mut packet, &mut offset, OPT_REQUESTED_IP, &ip.octets());
    }
    if let Some(ip) = server_id {
        write_option(
            &mut packet,
            &mut offset,
            OPT_SERVER_IDENTIFIER,
            &ip.octets(),
        );
    }
    packet[offset] = OPT_END;
    packet
}

fn write_option(packet: &mut [u8], offset: &mut usize, kind: u8, data: &[u8]) {
    packet[*offset] = kind;
    packet[*offset + 1] = data.len() as u8;
    packet[*offset + 2..*offset + 2 + data.len()].copy_from_slice(data);
    *offset += 2 + data.len();
}

fn option_value<'a>(packet: &'a [u8], kind: u8) -> Option<&'a [u8]> {
    let mut offset = DHCP_MIN_LEN;
    while offset < packet.len() {
        match packet[offset] {
            OPT_END => return None,
            0 => offset += 1,
            option => {
                let len = packet[offset + 1] as usize;
                let start = offset + 2;
                let end = start + len;
                if option == kind {
                    return Some(&packet[start..end]);
                }
                offset = end;
            }
        }
    }
    None
}

fn server_config() -> DhcpServerConfig {
    DhcpServerConfig::new(
        Ipv4Addr::new(192, 168, 4, 1),
        Ipv4Addr::new(255, 255, 255, 0),
        Ipv4Addr::new(192, 168, 4, 2),
        4,
    )
    .unwrap()
    .router(Ipv4Addr::new(192, 168, 4, 1))
    .dns_server(Ipv4Addr::new(192, 168, 4, 1))
    .lease_seconds(3600)
}

#[test]
fn dhcp_server_offers_first_lease_for_discover() {
    let mut server = DhcpServer::<4>::new(server_config());
    let request = request_packet(1, None, None);
    let mut response = [0u8; 320];

    let reply = server
        .handle(&request, &mut response)
        .unwrap()
        .expect("discover should produce offer");

    assert_eq!(reply.message_type(), DhcpMessageType::Offer);
    assert_eq!(reply.leased_ip(), Ipv4Addr::new(192, 168, 4, 2));
    assert_eq!(reply.client_mac(), [2, 4, 6, 8, 10, 12]);
    assert!(reply.broadcast());
    assert_eq!(response[0], 2);
    assert_eq!(&response[16..20], &[192, 168, 4, 2]);
    assert_eq!(
        option_value(&response[..reply.len()], OPT_MESSAGE_TYPE),
        Some(&[2][..])
    );
    assert_eq!(
        option_value(&response[..reply.len()], OPT_SERVER_IDENTIFIER),
        Some(&[192, 168, 4, 1][..])
    );
    assert_eq!(
        option_value(&response[..reply.len()], OPT_SUBNET_MASK),
        Some(&[255, 255, 255, 0][..])
    );
    assert_eq!(
        option_value(&response[..reply.len()], OPT_ROUTER),
        Some(&[192, 168, 4, 1][..])
    );
    assert_eq!(
        option_value(&response[..reply.len()], OPT_DOMAIN_NAME_SERVER),
        Some(&[192, 168, 4, 1][..])
    );
    assert_eq!(
        option_value(&response[..reply.len()], OPT_IP_LEASE_TIME),
        Some(&3600u32.to_be_bytes()[..])
    );
}

#[test]
fn dhcp_server_preserves_unicast_reply_preference() {
    let mut server = DhcpServer::<4>::new(server_config());
    let request = request_packet_with_flags(1, None, None, 0);
    let mut response = [0u8; 320];

    let reply = server
        .handle(&request, &mut response)
        .unwrap()
        .expect("discover should produce offer");

    assert_eq!(reply.message_type(), DhcpMessageType::Offer);
    assert_eq!(reply.client_mac(), [2, 4, 6, 8, 10, 12]);
    assert!(!reply.broadcast());
}

#[test]
fn dhcp_server_acks_request_for_its_offer() {
    let mut server = DhcpServer::<4>::new(server_config());
    let request = request_packet(
        3,
        Some(Ipv4Addr::new(192, 168, 4, 2)),
        Some(Ipv4Addr::new(192, 168, 4, 1)),
    );
    let mut response = [0u8; 320];

    let reply = server
        .handle(&request, &mut response)
        .unwrap()
        .expect("request should produce ack");

    assert_eq!(reply.message_type(), DhcpMessageType::Ack);
    assert_eq!(reply.leased_ip(), Ipv4Addr::new(192, 168, 4, 2));
    assert_eq!(
        option_value(&response[..reply.len()], OPT_MESSAGE_TYPE),
        Some(&[5][..])
    );
}

#[test]
fn dhcp_server_ignores_request_for_another_server() {
    let mut server = DhcpServer::<4>::new(server_config());
    let request = request_packet(
        3,
        Some(Ipv4Addr::new(192, 168, 4, 2)),
        Some(Ipv4Addr::new(192, 168, 4, 254)),
    );
    let mut response = [0u8; 320];

    assert_eq!(server.handle(&request, &mut response), Ok(None));
}

#[test]
fn dhcp_server_naks_request_outside_pool() {
    let mut server = DhcpServer::<4>::new(server_config());
    let request = request_packet(
        3,
        Some(Ipv4Addr::new(192, 168, 4, 99)),
        Some(Ipv4Addr::new(192, 168, 4, 1)),
    );
    let mut response = [0u8; 320];

    let reply = server
        .handle(&request, &mut response)
        .unwrap()
        .expect("bad request should produce nak");

    assert_eq!(reply.message_type(), DhcpMessageType::Nak);
    assert_eq!(reply.leased_ip(), Ipv4Addr::UNSPECIFIED);
    assert_eq!(
        option_value(&response[..reply.len()], OPT_MESSAGE_TYPE),
        Some(&[6][..])
    );
}

#[test]
fn dhcp_server_rejects_zero_sized_pool() {
    assert_eq!(
        DhcpServerConfig::new(
            Ipv4Addr::new(192, 168, 4, 1),
            Ipv4Addr::new(255, 255, 255, 0),
            Ipv4Addr::new(192, 168, 4, 2),
            0,
        ),
        Err(DhcpServerError::InvalidPool)
    );
}

#[test]
fn dhcp_server_rejects_pool_overlapping_server() {
    assert_eq!(
        DhcpServerConfig::new(
            Ipv4Addr::new(192, 168, 4, 1),
            Ipv4Addr::new(255, 255, 255, 0),
            Ipv4Addr::new(192, 168, 4, 1),
            4,
        ),
        Err(DhcpServerError::InvalidPool)
    );
}

#[test]
fn dhcp_server_rejects_pool_outside_server_subnet() {
    assert_eq!(
        DhcpServerConfig::new(
            Ipv4Addr::new(192, 168, 4, 1),
            Ipv4Addr::new(255, 255, 255, 0),
            Ipv4Addr::new(192, 168, 5, 2),
            4,
        ),
        Err(DhcpServerError::InvalidPool)
    );
}

#[test]
fn dhcp_server_rejects_pool_containing_broadcast_address() {
    assert_eq!(
        DhcpServerConfig::new(
            Ipv4Addr::new(192, 168, 4, 1),
            Ipv4Addr::new(255, 255, 255, 252),
            Ipv4Addr::new(192, 168, 4, 2),
            2,
        ),
        Err(DhcpServerError::InvalidPool)
    );
}

#[test]
fn dhcp_server_rejects_non_contiguous_subnet_mask() {
    assert_eq!(
        DhcpServerConfig::new(
            Ipv4Addr::new(192, 168, 4, 1),
            Ipv4Addr::new(255, 0, 255, 0),
            Ipv4Addr::new(192, 168, 4, 2),
            4,
        ),
        Err(DhcpServerError::InvalidPool)
    );
}

#[test]
fn dhcp_server_renders_max_lease_without_overflowing_rebind_time() {
    let config = DhcpServerConfig::new(
        Ipv4Addr::new(192, 168, 4, 1),
        Ipv4Addr::new(255, 255, 255, 0),
        Ipv4Addr::new(192, 168, 4, 2),
        4,
    )
    .unwrap()
    .lease_seconds(u32::MAX);
    let mut server = DhcpServer::<4>::new(config);
    let request = request_packet(1, None, None);
    let mut response = [0u8; 320];

    let reply = server.handle(&request, &mut response).unwrap().unwrap();

    assert_eq!(
        option_value(&response[..reply.len()], OPT_RENEWAL_TIME),
        Some(&(u32::MAX / 2).to_be_bytes()[..])
    );
    assert_eq!(
        option_value(&response[..reply.len()], OPT_REBINDING_TIME),
        Some(&(u32::MAX - (u32::MAX / 8)).to_be_bytes()[..])
    );
}
