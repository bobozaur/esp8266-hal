#[path = "../examples/support/dhcp.rs"]
#[allow(dead_code)] // Other application helpers are outside these protocol tests.
mod dhcp;
#[path = "../examples/support/dhcp_wire.rs"]
mod dhcp_wire;
use core::net::Ipv4Addr;
use dhcp::{DhcpMessageType, DhcpServer, DhcpServerConfig, DhcpServerError};
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

fn option_value(packet: &[u8], kind: u8) -> Option<&[u8]> {
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
        .handle(0, &request, &mut response)
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
        .handle(0, &request, &mut response)
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
        .handle(0, &request, &mut response)
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

    assert_eq!(server.handle(0, &request, &mut response), Ok(None));
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
        .handle(0, &request, &mut response)
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

    let reply = server.handle(0, &request, &mut response).unwrap().unwrap();

    assert_eq!(
        option_value(&response[..reply.len()], OPT_RENEWAL_TIME),
        Some(&(u32::MAX / 2).to_be_bytes()[..])
    );
    assert_eq!(
        option_value(&response[..reply.len()], OPT_REBINDING_TIME),
        Some(&(u32::MAX - (u32::MAX / 8)).to_be_bytes()[..])
    );
}

#[test]
fn dhcp_server_reuses_expired_lease_slots() {
    let mut server = DhcpServer::<1>::new(server_config());
    let first = request_packet(1, None, None);
    let mut second = request_packet(1, None, None);
    second[28..34].copy_from_slice(&[3, 4, 5, 6, 7, 8]);
    let mut response = [0u8; 320];

    let first_reply = server.handle(10, &first, &mut response).unwrap().unwrap();
    assert_eq!(first_reply.leased_ip(), Ipv4Addr::new(192, 168, 4, 2));
    assert_eq!(server.leases().next().unwrap().expires_at_seconds(), 3610);
    assert_eq!(
        server.handle(3609, &second, &mut response),
        Err(DhcpServerError::LeasePoolFull)
    );

    let second_reply = server
        .handle(3610, &second, &mut response)
        .unwrap()
        .unwrap();
    assert_eq!(second_reply.leased_ip(), Ipv4Addr::new(192, 168, 4, 2));
    assert_eq!(second_reply.client_mac(), [3, 4, 5, 6, 7, 8]);
}

#[test]
fn dhcp_release_frees_lease_immediately() {
    let mut server = DhcpServer::<1>::new(server_config());
    let first = request_packet(1, None, None);
    let release = request_packet(7, None, None);
    let mut second = request_packet(1, None, None);
    second[28..34].copy_from_slice(&[3, 4, 5, 6, 7, 8]);
    let mut response = [0u8; 320];

    server.handle(0, &first, &mut response).unwrap().unwrap();
    assert_eq!(server.handle(1, &release, &mut response), Ok(None));
    assert_eq!(server.leases().count(), 0);

    let reply = server.handle(2, &second, &mut response).unwrap().unwrap();
    assert_eq!(reply.leased_ip(), Ipv4Addr::new(192, 168, 4, 2));
}

fn request_frame(payload: &[u8]) -> Vec<u8> {
    let ip_len = 20 + 8 + payload.len();
    let mut frame = vec![0; 14 + ip_len];
    frame[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
    frame[14] = 0x45;
    frame[16..18].copy_from_slice(&(ip_len as u16).to_be_bytes());
    frame[23] = 17;
    frame[34..36].copy_from_slice(&68u16.to_be_bytes());
    frame[36..38].copy_from_slice(&67u16.to_be_bytes());
    frame[38..40].copy_from_slice(&((8 + payload.len()) as u16).to_be_bytes());
    frame[42..].copy_from_slice(payload);
    fill_ipv4_checksum(&mut frame);
    frame
}

fn fill_ipv4_checksum(frame: &mut [u8]) {
    smoltcp::wire::Ipv4Packet::new_unchecked(&mut frame[14..]).fill_checksum();
}

#[test]
fn dhcp_frame_parser_respects_packet_lengths_and_ports() {
    let request = request_packet(1, None, None);
    let mut frame = request_frame(&request);
    frame.extend_from_slice(&[0xa5; 32]);
    assert_eq!(dhcp_wire::payload(&frame), Some(request.as_slice()));
    for length in 0..42 + request.len() {
        assert!(dhcp_wire::payload(&frame[..length]).is_none());
    }
    let mut malformed = frame.clone();
    malformed[12..14].copy_from_slice(&0x0806u16.to_be_bytes());
    assert!(dhcp_wire::payload(&malformed).is_none());
    malformed = frame.clone();
    malformed[23] = 6;
    fill_ipv4_checksum(&mut malformed);
    assert!(dhcp_wire::payload(&malformed).is_none());
    malformed = frame.clone();
    malformed[36..38].copy_from_slice(&68u16.to_be_bytes());
    assert!(dhcp_wire::payload(&malformed).is_none());
    malformed = frame.clone();
    malformed[38..40].copy_from_slice(&7u16.to_be_bytes());
    assert!(dhcp_wire::payload(&malformed).is_none());
    malformed = frame;
    malformed[16..18].copy_from_slice(&19u16.to_be_bytes());
    assert!(dhcp_wire::payload(&malformed).is_none());
}

#[test]
fn dhcp_frame_parser_accepts_ipv4_options_and_ignores_udp_padding() {
    let request = request_packet(1, None, None);
    let mut frame = request_frame(&request);
    drop(frame.splice(34..34, [1, 1, 1, 1]));
    frame[14] = 0x46;
    let ip_len = (frame.len() - 14) as u16;
    frame[16..18].copy_from_slice(&ip_len.to_be_bytes());
    fill_ipv4_checksum(&mut frame);
    assert_eq!(dhcp_wire::payload(&frame), Some(request.as_slice()));

    // The IP packet can contain padding after its declared UDP datagram.
    frame.extend_from_slice(&[0xa5; 8]);
    frame[16..18].copy_from_slice(&(ip_len + 8).to_be_bytes());
    fill_ipv4_checksum(&mut frame);
    assert_eq!(dhcp_wire::payload(&frame), Some(request.as_slice()));
}

#[test]
fn dhcp_frame_parser_rejects_invalid_ip_headers_and_fragments() {
    let request = request_packet(1, None, None);
    let frame = request_frame(&request);
    let mut malformed = frame.clone();
    malformed[14] = 0x65;
    fill_ipv4_checksum(&mut malformed);
    assert!(dhcp_wire::payload(&malformed).is_none());

    malformed = frame.clone();
    malformed[22] ^= 1; // Change TTL without updating the IPv4 checksum.
    assert!(dhcp_wire::payload(&malformed).is_none());

    for flags_and_offset in [0x2000u16, 1] {
        malformed = frame.clone();
        malformed[20..22].copy_from_slice(&flags_and_offset.to_be_bytes());
        fill_ipv4_checksum(&mut malformed);
        assert!(dhcp_wire::payload(&malformed).is_none());
    }

    // Keep a complete UDP datagram behind an illegally short IPv4 header.
    // Its IP checksum is valid, so rejecting it requires the minimum IHL check.
    malformed = frame;
    drop(malformed.drain(26..34));
    malformed[14] = 0x43;
    let ip_len = (malformed.len() - 14) as u16;
    malformed[16..18].copy_from_slice(&ip_len.to_be_bytes());
    fill_ipv4_checksum(&mut malformed);
    assert!(dhcp_wire::payload(&malformed).is_none());
}

#[test]
fn dhcp_frame_parser_checks_nonzero_udp_checksums_but_accepts_omission() {
    use smoltcp::wire::{Ipv4Packet, UdpPacket};
    let request = request_packet(1, None, None);
    let mut frame = request_frame(&request);
    assert_eq!(dhcp_wire::payload(&frame), Some(request.as_slice()));

    let mut ip = Ipv4Packet::new_unchecked(&mut frame[14..]);
    let src_addr = ip.src_addr().into();
    let dst_addr = ip.dst_addr().into();
    UdpPacket::new_unchecked(ip.payload_mut()).fill_checksum(&src_addr, &dst_addr);
    assert_eq!(dhcp_wire::payload(&frame), Some(request.as_slice()));

    frame[46] ^= 1; // A changed DHCP transaction ID is still a valid request.
    assert!(dhcp_wire::payload(&frame).is_none());
    frame[40..42].fill(0); // IPv4 permits an explicitly omitted UDP checksum.
    assert_eq!(dhcp_wire::payload(&frame), Some(&frame[42..]));
}

#[test]
fn dhcp_reply_frames_preserve_payload_routing_and_checksums() {
    use smoltcp::wire::{EthernetAddress, EthernetFrame, EthernetProtocol, Ipv4Packet, UdpPacket};

    for flags in [0, 0x8000] {
        let mut server = DhcpServer::<4>::new(server_config());
        let request = request_packet_with_flags(1, None, None, flags);
        let mut frame = [0xa5; 384];
        let reply = server
            .handle(0, &request, &mut frame[dhcp_wire::REPLY_HEADERS..])
            .unwrap()
            .unwrap();
        let original_payload =
            frame[dhcp_wire::REPLY_HEADERS..dhcp_wire::REPLY_HEADERS + reply.len()].to_vec();
        let server_mac = [0x02, 0x10, 0x20, 0x30, 0x40, 0x50];
        let frame_len =
            dhcp_wire::emit_reply(&mut frame, Ipv4Addr::new(192, 168, 4, 1), server_mac, reply);
        assert_eq!(frame_len, dhcp_wire::REPLY_HEADERS + reply.len());
        let ethernet = EthernetFrame::new_checked(&frame[..frame_len]).unwrap();
        assert_eq!(ethernet.src_addr(), EthernetAddress(server_mac));
        assert_eq!(ethernet.ethertype(), EthernetProtocol::Ipv4);
        assert_eq!(
            ethernet.dst_addr(),
            if flags == 0 {
                EthernetAddress(reply.client_mac())
            } else {
                EthernetAddress::BROADCAST
            }
        );
        let ipv4 = Ipv4Packet::new_checked(ethernet.payload()).unwrap();
        assert_eq!(ipv4.src_addr().octets(), [192, 168, 4, 1]);
        assert_eq!(
            ipv4.dst_addr().octets(),
            if flags == 0 {
                [192, 168, 4, 2]
            } else {
                [255; 4]
            }
        );
        assert_eq!(ipv4.hop_limit(), 64);
        assert!(ipv4.verify_checksum());
        let udp = UdpPacket::new_checked(ipv4.payload()).unwrap();
        assert_eq!(udp.src_port(), 67);
        assert_eq!(udp.dst_port(), 68);
        assert_ne!(udp.checksum(), 0);
        assert!(udp.verify_checksum(&ipv4.src_addr().into(), &ipv4.dst_addr().into()));
        assert_eq!(udp.payload(), original_payload);
        assert!(frame[frame_len..].iter().all(|&byte| byte == 0));
    }
}
