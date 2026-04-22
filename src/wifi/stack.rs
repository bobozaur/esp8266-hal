//! High-level `smoltcp` network stack integration over [`wifi_core`](crate::wifi_core).

use crate::wifi::device::WifiDevice;
use crate::wifi::ip::{Ipv4Addr, Ipv4Info};
use crate::wifi_core::{DriverError, HasSta, WifiBackend};
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet, SocketStorage};
use smoltcp::socket::dhcpv4;
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpCidr, Ipv4Address};

/// Get the current timestamp as a smoltcp [`Instant`].
pub fn now() -> Instant {
    let us = unsafe { esp8266_wifi_sys::esp_timer_get_time() };
    Instant::from_millis(us / 1000)
}

/// High-level smoltcp network stack.
pub struct WifiStack<
    'a,
    'd,
    M,
    B: WifiBackend,
    const EVENTS: usize,
    const RX: usize,
    const SUBS: usize,
    const ESP_NOW_PEERS: usize = 0,
    const ESP_NOW_RX: usize = 0,
    const PROMISCUOUS_RX: usize = 0,
> {
    device: WifiDevice<'d, M, B, EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX>,
    iface: Interface,
    sockets: SocketSet<'a>,
    dhcp_handle: SocketHandle,
}

impl<
        'a,
        'd,
        M: HasSta,
        B: WifiBackend,
        const EVENTS: usize,
        const RX: usize,
        const SUBS: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
    > WifiStack<'a, 'd, M, B, EVENTS, RX, SUBS, ESP_NOW_PEERS, ESP_NOW_RX, PROMISCUOUS_RX>
{
    pub fn new(
        mut device: WifiDevice<
            'd,
            M,
            B,
            EVENTS,
            RX,
            SUBS,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
        >,
        socket_storage: &'a mut [SocketStorage<'a>],
    ) -> Result<Self, DriverError> {
        let mac = device.mac_address()?;
        let config = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
        let mut iface = Interface::new(config, &mut device, now());

        iface.update_ip_addrs(|addrs| {
            let _ = addrs.push(IpCidr::new(Ipv4Address::UNSPECIFIED.into(), 0));
        });

        let mut sockets = SocketSet::new(socket_storage);
        let dhcp_socket = dhcpv4::Socket::new();
        let dhcp_handle = sockets.add(dhcp_socket);

        Ok(Self {
            device,
            iface,
            sockets,
            dhcp_handle,
        })
    }

    pub fn poll(&mut self) -> bool {
        let result = self.iface.poll(now(), &mut self.device, &mut self.sockets);
        let changed = matches!(result, smoltcp::iface::PollResult::SocketStateChanged);

        let dhcp = self.sockets.get_mut::<dhcpv4::Socket>(self.dhcp_handle);
        if let Some(dhcpv4::Event::Configured(config)) = dhcp.poll() {
            self.iface.update_ip_addrs(|addrs| {
                if let Some(addr) = addrs.iter_mut().next() {
                    *addr = IpCidr::Ipv4(config.address);
                }
            });
            if let Some(router) = config.router {
                self.iface.routes_mut().add_default_ipv4_route(router).ok();
            }
        }

        changed
    }

    pub fn has_ip(&self) -> bool {
        self.iface
            .ipv4_addr()
            .is_some_and(|ip| !ip.is_unspecified())
    }

    pub fn ip_info(&mut self) -> Option<Ipv4Info> {
        let ip = self.iface.ipv4_addr()?;
        if ip.is_unspecified() {
            return None;
        }

        let prefix_len = self
            .iface
            .ip_addrs()
            .iter()
            .find_map(|cidr| match cidr {
                IpCidr::Ipv4(v4) => Some(v4.prefix_len()),
                #[allow(unreachable_patterns)]
                _ => None,
            })
            .unwrap_or(24);
        let mask = if prefix_len == 0 {
            0u32
        } else {
            u32::MAX << (32 - prefix_len)
        };

        let mut gateway = Ipv4Addr::UNSPECIFIED;
        self.iface.routes_mut().update(|storage| {
            for route in storage.iter() {
                let smoltcp::wire::IpCidr::Ipv4(cidr) = &route.cidr;
                let smoltcp::wire::IpAddress::Ipv4(via) = &route.via_router;
                if cidr.prefix_len() == 0 {
                    gateway = Ipv4Addr::from_smoltcp(*via);
                }
            }
        });

        Some(Ipv4Info {
            ip: Ipv4Addr::from_smoltcp(ip),
            netmask: Ipv4Addr(mask.to_be_bytes()),
            gateway,
        })
    }

    pub async fn wait_for_ip_async(&mut self, timeout_ms: u32) -> bool {
        let deadline =
            embassy_time::Instant::now() + embassy_time::Duration::from_millis(timeout_ms as u64);

        loop {
            self.poll();
            if self.has_ip() {
                return true;
            }
            if embassy_time::Instant::now() >= deadline {
                return false;
            }
            embassy_time::Timer::after(embassy_time::Duration::from_millis(10)).await;
        }
    }

    pub fn wait_for_ip(&mut self, timeout_ms: u32) -> bool {
        let start = now();
        loop {
            self.poll();
            if self.has_ip() {
                return true;
            }
            let elapsed = now() - start;
            if elapsed.total_millis() >= timeout_ms as u64 {
                return false;
            }
            core::hint::spin_loop();
        }
    }

    pub async fn work(&mut self) -> ! {
        loop {
            self.poll();
            embassy_time::Timer::after(embassy_time::Duration::from_millis(10)).await;
        }
    }

    pub fn sockets_mut(&mut self) -> &mut SocketSet<'a> {
        &mut self.sockets
    }

    pub fn iface(&self) -> &Interface {
        &self.iface
    }

    pub fn iface_mut(&mut self) -> &mut Interface {
        &mut self.iface
    }

    pub fn iface_and_sockets_mut(&mut self) -> (&mut Interface, &mut SocketSet<'a>) {
        (&mut self.iface, &mut self.sockets)
    }
}
