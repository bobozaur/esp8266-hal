//! Adapter-local neighbor-learning workaround for published smoltcp.
//!
//! The prior target TCP server regression needed learning from its first inbound
//! IPv4 frame to avoid a first-response ARP timeout. Retain this narrow adapter
//! workaround until the upstream change or target comparison removes that need.
//! This keeps the crate on published smoltcp while preserving the useful part
//! of the removed fork: after a validated unicast IPv4 frame addressed to this
//! interface, queue one synthetic ARP reply so smoltcp learns the peer through
//! its public ARP path. The hint is one-shot, local-MAC gated, and emitted
//! after the triggering frame, so smoltcp may spend one poll resolving the
//! neighbor before it receives the synthetic ARP reply.

#[cfg(feature = "wifi-embassy-net")]
use crate::wifi_core::Interface;

pub(crate) const ETHERNET_ARP_IPV4_FRAME_LEN: usize = 42;

const ETHERNET_HEADER_LEN: usize = 14;
const IPV4_MIN_HEADER_LEN: usize = 20;
const ARP_IPV4_PAYLOAD_LEN: usize = 28;

const ETHERTYPE_IPV4: [u8; 2] = 0x0800u16.to_be_bytes();
const ETHERTYPE_ARP: [u8; 2] = 0x0806u16.to_be_bytes();
const ARP_HTYPE_ETHERNET: [u8; 2] = 1u16.to_be_bytes();
const ARP_OPERATION_REPLY: [u8; 2] = 2u16.to_be_bytes();

#[derive(Clone, Copy)]
pub(crate) struct EthernetNeighborHint {
    local_mac: [u8; 6],
    remote_mac: [u8; 6],
    local_ip: [u8; 4],
    remote_ip: [u8; 4],
}

pub(crate) struct EthernetNeighborHints {
    local_mac: Option<[u8; 6]>,
    pending: Option<EthernetNeighborHint>,
}

impl EthernetNeighborHints {
    pub(crate) fn new(local_mac: Option<[u8; 6]>) -> Self {
        Self {
            local_mac,
            pending: None,
        }
    }

    pub(crate) fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    #[cfg(any(feature = "wifi-smoltcp", test))]
    pub(crate) fn take_pending(&mut self) -> Option<EthernetNeighborHint> {
        self.pending.take()
    }

    pub(crate) fn clear(&mut self) {
        self.pending = None;
    }

    pub(crate) fn observe(&mut self, frame: &[u8]) {
        let Some(local_mac) = self.local_mac else {
            return;
        };

        if self.pending.is_none() {
            self.pending = EthernetNeighborHint::from_ipv4_frame(frame, local_mac);
        }
    }

    #[cfg(feature = "wifi-embassy-net")]
    #[cfg_attr(not(target_arch = "xtensa"), allow(dead_code))]
    pub(crate) fn receive_into(
        &mut self,
        link: &mut Interface<'_>,
        buf: &mut [u8],
    ) -> Result<Option<usize>, crate::wifi_core::DriverError> {
        if let Some(hint) = self.pending.take() {
            if let Some(len) = hint.emit_arp_reply(buf) {
                return Ok(Some(len));
            }
            return Err(crate::wifi_core::DriverError::PacketOverflow);
        }

        let Some(len) = link.receive(buf)? else {
            return Ok(None);
        };
        self.observe(&buf[..len]);
        Ok(Some(len))
    }
}

impl EthernetNeighborHint {
    pub(crate) fn from_ipv4_frame(frame: &[u8], local_mac: [u8; 6]) -> Option<Self> {
        let ethernet = frame.get(..ETHERNET_HEADER_LEN)?;
        let ethertype: [u8; 2] = ethernet[12..14].try_into().ok()?;
        if ethertype != ETHERTYPE_IPV4 {
            return None;
        }

        let destination_mac: [u8; 6] = ethernet[0..6].try_into().ok()?;
        if destination_mac != local_mac {
            return None;
        }

        let remote_mac = ethernet[6..12].try_into().ok()?;
        if !is_unicast_mac(&local_mac) || !is_unicast_mac(&remote_mac) || remote_mac == local_mac {
            return None;
        }

        let ipv4 = frame.get(ETHERNET_HEADER_LEN..)?;
        let fixed_header = ipv4.get(..IPV4_MIN_HEADER_LEN)?;
        let version = fixed_header[0] >> 4;
        let header_len = usize::from(fixed_header[0] & 0x0f) * 4;
        if version != 4 || header_len < IPV4_MIN_HEADER_LEN {
            return None;
        }

        let header = ipv4.get(..header_len)?;
        if ipv4_checksum(header) != 0 {
            return None;
        }

        let total_len = u16::from_be_bytes([fixed_header[2], fixed_header[3]]) as usize;
        if total_len < header_len || ipv4.len() < total_len {
            return None;
        }

        let flags_fragment = u16::from_be_bytes([fixed_header[6], fixed_header[7]]);
        if flags_fragment & 0x3fff != 0 {
            return None;
        }

        let remote_ip = fixed_header[12..16].try_into().ok()?;
        let local_ip = fixed_header[16..20].try_into().ok()?;
        if !is_unicast_ipv4(&remote_ip) || !is_unicast_ipv4(&local_ip) || remote_ip == local_ip {
            return None;
        }

        Some(Self {
            local_mac,
            remote_mac,
            local_ip,
            remote_ip,
        })
    }

    pub(crate) fn emit_arp_reply(self, frame: &mut [u8]) -> Option<usize> {
        let frame = frame.get_mut(..ETHERNET_ARP_IPV4_FRAME_LEN)?;
        frame[..6].copy_from_slice(&self.local_mac);
        frame[6..12].copy_from_slice(&self.remote_mac);
        frame[12..14].copy_from_slice(&ETHERTYPE_ARP);

        let arp = &mut frame[ETHERNET_HEADER_LEN..];
        debug_assert_eq!(arp.len(), ARP_IPV4_PAYLOAD_LEN);
        arp[0..2].copy_from_slice(&ARP_HTYPE_ETHERNET);
        arp[2..4].copy_from_slice(&ETHERTYPE_IPV4);
        arp[4] = 6;
        arp[5] = 4;
        arp[6..8].copy_from_slice(&ARP_OPERATION_REPLY);
        arp[8..14].copy_from_slice(&self.remote_mac);
        arp[14..18].copy_from_slice(&self.remote_ip);
        arp[18..24].copy_from_slice(&self.local_mac);
        arp[24..28].copy_from_slice(&self.local_ip);

        Some(ETHERNET_ARP_IPV4_FRAME_LEN)
    }
}

fn is_unicast_mac(addr: &[u8; 6]) -> bool {
    addr[0] & 0x01 == 0
}

fn is_unicast_ipv4(addr: &[u8; 4]) -> bool {
    *addr != [0, 0, 0, 0] && *addr != [0xff; 4] && addr[0] != 127 && addr[0] & 0xf0 != 0xe0
}

fn ipv4_checksum(bytes: &[u8]) -> u16 {
    let mut sum = 0u32;
    let mut chunks = bytes.chunks_exact(2);
    for chunk in &mut chunks {
        sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
    }
    if let Some(&byte) = chunks.remainder().first() {
        sum += u16::from_be_bytes([byte, 0]) as u32;
    }
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCAL_MAC: [u8; 6] = [0x02, 0x00, 0x00, 0x00, 0x00, 0x10];
    const REMOTE_MAC: [u8; 6] = [0x02, 0x00, 0x00, 0x00, 0x00, 0x20];
    const LOCAL_IP: [u8; 4] = [192, 168, 1, 10];
    const REMOTE_IP: [u8; 4] = [192, 168, 1, 20];

    #[test]
    fn emits_arp_reply_from_unicast_ipv4_frame() {
        let hint = EthernetNeighborHint::from_ipv4_frame(
            &ipv4_frame(REMOTE_MAC, LOCAL_MAC, REMOTE_IP, LOCAL_IP),
            LOCAL_MAC,
        )
        .expect("valid unicast IPv4 frame should produce a neighbor hint");

        let mut frame = [0u8; ETHERNET_ARP_IPV4_FRAME_LEN];
        assert_eq!(
            hint.emit_arp_reply(&mut frame),
            Some(ETHERNET_ARP_IPV4_FRAME_LEN)
        );

        assert_eq!(&frame[0..6], &LOCAL_MAC);
        assert_eq!(&frame[6..12], &REMOTE_MAC);
        assert_eq!(&frame[12..14], &ETHERTYPE_ARP);
        assert_eq!(&frame[14..16], &ARP_HTYPE_ETHERNET);
        assert_eq!(&frame[16..18], &ETHERTYPE_IPV4);
        assert_eq!(frame[18], 6);
        assert_eq!(frame[19], 4);
        assert_eq!(&frame[20..22], &ARP_OPERATION_REPLY);
        assert_eq!(&frame[22..28], &REMOTE_MAC);
        assert_eq!(&frame[28..32], &REMOTE_IP);
        assert_eq!(&frame[32..38], &LOCAL_MAC);
        assert_eq!(&frame[38..42], &LOCAL_IP);
    }

    #[test]
    fn rejects_broadcast_destination_ipv4() {
        assert!(EthernetNeighborHint::from_ipv4_frame(
            &ipv4_frame(REMOTE_MAC, LOCAL_MAC, REMOTE_IP, [255, 255, 255, 255]),
            LOCAL_MAC,
        )
        .is_none());
    }

    #[test]
    fn rejects_bad_ipv4_checksum() {
        let mut frame = ipv4_frame(REMOTE_MAC, LOCAL_MAC, REMOTE_IP, LOCAL_IP);
        frame[ETHERNET_HEADER_LEN + 8] ^= 1;
        assert!(EthernetNeighborHint::from_ipv4_frame(&frame, LOCAL_MAC).is_none());
    }

    #[test]
    fn rejects_frames_not_addressed_to_local_mac() {
        let other_mac = [0x02, 0, 0, 0, 0, 0x30];
        assert!(EthernetNeighborHint::from_ipv4_frame(
            &ipv4_frame(REMOTE_MAC, other_mac, REMOTE_IP, LOCAL_IP),
            LOCAL_MAC,
        )
        .is_none());
    }

    #[test]
    fn rejects_self_ip_or_mac_bindings() {
        assert!(EthernetNeighborHint::from_ipv4_frame(
            &ipv4_frame(REMOTE_MAC, LOCAL_MAC, LOCAL_IP, LOCAL_IP),
            LOCAL_MAC,
        )
        .is_none());
        assert!(EthernetNeighborHint::from_ipv4_frame(
            &ipv4_frame(LOCAL_MAC, LOCAL_MAC, REMOTE_IP, LOCAL_IP),
            LOCAL_MAC,
        )
        .is_none());
    }

    #[test]
    fn rejects_non_ipv4_or_runt_frames() {
        let mut frame = ipv4_frame(REMOTE_MAC, LOCAL_MAC, REMOTE_IP, LOCAL_IP);
        assert!(EthernetNeighborHint::from_ipv4_frame(&frame[..10], LOCAL_MAC).is_none());
        assert!(EthernetNeighborHint::from_ipv4_frame(&frame[..20], LOCAL_MAC).is_none());

        frame[12..14].copy_from_slice(&ETHERTYPE_ARP);
        assert!(EthernetNeighborHint::from_ipv4_frame(&frame, LOCAL_MAC).is_none());
    }

    #[test]
    fn rejects_multicast_mac_and_loopback_ip() {
        let multicast_mac = [0x03, 0, 0, 0, 0, 0x20];
        assert!(EthernetNeighborHint::from_ipv4_frame(
            &ipv4_frame(multicast_mac, LOCAL_MAC, REMOTE_IP, LOCAL_IP),
            LOCAL_MAC,
        )
        .is_none());
        assert!(EthernetNeighborHint::from_ipv4_frame(
            &ipv4_frame(REMOTE_MAC, LOCAL_MAC, [127, 0, 0, 1], LOCAL_IP),
            LOCAL_MAC,
        )
        .is_none());
    }

    #[test]
    fn rejects_fragmented_ipv4_frames() {
        let mut more_fragments = ipv4_frame(REMOTE_MAC, LOCAL_MAC, REMOTE_IP, LOCAL_IP);
        more_fragments[ETHERNET_HEADER_LEN + 6..ETHERNET_HEADER_LEN + 8]
            .copy_from_slice(&0x6000u16.to_be_bytes());
        write_ipv4_checksum(&mut more_fragments, IPV4_MIN_HEADER_LEN);
        assert!(EthernetNeighborHint::from_ipv4_frame(&more_fragments, LOCAL_MAC).is_none());

        let mut fragment_offset = ipv4_frame(REMOTE_MAC, LOCAL_MAC, REMOTE_IP, LOCAL_IP);
        fragment_offset[ETHERNET_HEADER_LEN + 6..ETHERNET_HEADER_LEN + 8]
            .copy_from_slice(&0x4001u16.to_be_bytes());
        write_ipv4_checksum(&mut fragment_offset, IPV4_MIN_HEADER_LEN);
        assert!(EthernetNeighborHint::from_ipv4_frame(&fragment_offset, LOCAL_MAC).is_none());
    }

    #[test]
    fn accepts_ipv4_options() {
        let frame = ipv4_frame_with_header_len(REMOTE_MAC, LOCAL_MAC, REMOTE_IP, LOCAL_IP, 24);
        assert!(EthernetNeighborHint::from_ipv4_frame(&frame, LOCAL_MAC).is_some());
    }

    #[test]
    fn queue_emits_pending_before_observing_next_frame() {
        let mut hints = EthernetNeighborHints::new(Some(LOCAL_MAC));
        hints.observe(&ipv4_frame(REMOTE_MAC, LOCAL_MAC, REMOTE_IP, LOCAL_IP));
        assert!(hints.has_pending());

        let mut frame = [0u8; ETHERNET_ARP_IPV4_FRAME_LEN];
        assert_eq!(
            hints
                .take_pending()
                .and_then(|hint| hint.emit_arp_reply(&mut frame)),
            Some(ETHERNET_ARP_IPV4_FRAME_LEN)
        );
        assert!(!hints.has_pending());
    }

    fn ipv4_frame(
        src_mac: [u8; 6],
        dst_mac: [u8; 6],
        src_ip: [u8; 4],
        dst_ip: [u8; 4],
    ) -> [u8; ETHERNET_HEADER_LEN + IPV4_MIN_HEADER_LEN] {
        let frame = ipv4_frame_with_header_len(src_mac, dst_mac, src_ip, dst_ip, 20);
        let mut fixed = [0u8; ETHERNET_HEADER_LEN + IPV4_MIN_HEADER_LEN];
        fixed.copy_from_slice(&frame[..ETHERNET_HEADER_LEN + IPV4_MIN_HEADER_LEN]);
        fixed
    }

    fn ipv4_frame_with_header_len(
        src_mac: [u8; 6],
        dst_mac: [u8; 6],
        src_ip: [u8; 4],
        dst_ip: [u8; 4],
        header_len: usize,
    ) -> [u8; ETHERNET_HEADER_LEN + 24] {
        let mut frame = [0u8; ETHERNET_HEADER_LEN + 24];
        frame[0..6].copy_from_slice(&dst_mac);
        frame[6..12].copy_from_slice(&src_mac);
        frame[12..14].copy_from_slice(&ETHERTYPE_IPV4);

        let ip = &mut frame[ETHERNET_HEADER_LEN..ETHERNET_HEADER_LEN + header_len];
        ip[0] = 0x40 | ((header_len / 4) as u8);
        ip[2..4].copy_from_slice(&(header_len as u16).to_be_bytes());
        ip[6..8].copy_from_slice(&0x4000u16.to_be_bytes());
        ip[8] = 64;
        ip[9] = 6;
        ip[12..16].copy_from_slice(&src_ip);
        ip[16..20].copy_from_slice(&dst_ip);
        write_ipv4_checksum(&mut frame, header_len);
        frame
    }

    fn write_ipv4_checksum(frame: &mut [u8], header_len: usize) {
        let ip = &mut frame[ETHERNET_HEADER_LEN..ETHERNET_HEADER_LEN + header_len];
        ip[10..12].fill(0);
        let checksum = ipv4_checksum(ip);
        ip[10..12].copy_from_slice(&checksum.to_be_bytes());
    }
}
