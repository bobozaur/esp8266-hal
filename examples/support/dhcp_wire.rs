//! Ethernet/IPv4/UDP framing shared by the raw AP DHCP examples.

use core::net::Ipv4Addr;
use smoltcp::phy::ChecksumCapabilities;
use smoltcp::wire::{
    EthernetAddress, EthernetFrame, EthernetProtocol, IpAddress, IpProtocol,
    Ipv4Address as WireIpv4Address, Ipv4Packet, Ipv4Repr, UdpPacket, UdpRepr, ETHERNET_HEADER_LEN,
    IPV4_HEADER_LEN, UDP_HEADER_LEN,
};

use super::dhcp::DhcpReply;

pub const REPLY_HEADERS: usize = ETHERNET_HEADER_LEN + IPV4_HEADER_LEN + UDP_HEADER_LEN;

pub fn payload(frame: &[u8]) -> Option<&[u8]> {
    let ethernet = EthernetFrame::new_checked(frame).ok()?;
    if ethernet.ethertype() != EthernetProtocol::Ipv4 {
        return None;
    }
    let ipv4 = Ipv4Packet::new_checked(ethernet.payload()).ok()?;
    // These examples have no fragment reassembly, regardless of which smoltcp
    // features an application enables. IPv4 always needs its complete header.
    if usize::from(ipv4.header_len()) < IPV4_HEADER_LEN
        || ipv4.more_frags()
        || ipv4.frag_offset() != 0
    {
        return None;
    }
    let checksum_caps = ChecksumCapabilities::default();
    let ip_repr = Ipv4Repr::parse(&ipv4, &checksum_caps).ok()?;
    if ip_repr.next_header != IpProtocol::Udp {
        return None;
    }
    let udp = UdpPacket::new_checked(ipv4.payload()).ok()?;
    let udp_repr = UdpRepr::parse(
        &udp,
        &IpAddress::Ipv4(ip_repr.src_addr),
        &IpAddress::Ipv4(ip_repr.dst_addr),
        &checksum_caps,
    )
    .ok()?;
    if udp_repr.src_port == 68 && udp_repr.dst_port == 67 {
        Some(udp.payload())
    } else {
        None
    }
}

/// Add headers to a DHCP payload already written after [`REPLY_HEADERS`].
/// The caller retains the frame buffer, including while retrying transmission.
pub fn emit_reply(
    frame: &mut [u8],
    server_ip: Ipv4Addr,
    server_mac: [u8; 6],
    reply: DhcpReply,
) -> usize {
    let src_ip = WireIpv4Address::from(server_ip.octets());
    let broadcast = reply.broadcast() || reply.leased_ip().is_unspecified();
    let dst_ip = if broadcast {
        WireIpv4Address::BROADCAST
    } else {
        WireIpv4Address::from(reply.leased_ip().octets())
    };
    let checksum_caps = ChecksumCapabilities::default();
    let frame_len = REPLY_HEADERS + reply.len();

    let mut ethernet = EthernetFrame::new_unchecked(&mut frame[..frame_len]);
    ethernet.set_dst_addr(if broadcast {
        EthernetAddress::BROADCAST
    } else {
        EthernetAddress(reply.client_mac())
    });
    ethernet.set_src_addr(EthernetAddress(server_mac));
    ethernet.set_ethertype(EthernetProtocol::Ipv4);

    let mut ipv4 = Ipv4Packet::new_unchecked(ethernet.payload_mut());
    Ipv4Repr {
        src_addr: src_ip,
        dst_addr: dst_ip,
        next_header: IpProtocol::Udp,
        payload_len: UDP_HEADER_LEN + reply.len(),
        hop_limit: 64,
    }
    .emit(&mut ipv4, &checksum_caps);
    UdpRepr {
        src_port: 67,
        dst_port: 68,
    }
    .emit(
        &mut UdpPacket::new_unchecked(ipv4.payload_mut()),
        &IpAddress::Ipv4(src_ip),
        &IpAddress::Ipv4(dst_ip),
        reply.len(),
        |_| {},
        &checksum_caps,
    );
    frame_len
}
