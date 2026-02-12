//! High-level smoltcp network stack integration.
//!
//! [`WifiStack`] ties together a [`WifiDevice`],
//! a smoltcp [`Interface`], and a [`SocketSet`] with optional DHCP. It drives
//! the network stack via [`poll()`](WifiStack::poll) and provides helpers for
//! common operations like waiting for an IP address.
//!
//! # Example
//!
//! ```rust,no_run
//! use esp8266_wifi::stack::WifiStack;
//! use esp8266_wifi::controller::WifiInterface;
//! use esp8266_wifi::device::WifiDevice;
//! use smoltcp::iface::SocketStorage;
//!
//! let device = WifiDevice::new(WifiInterface::Sta);
//! let mac = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
//! let mut socket_storage: [SocketStorage; 4] = Default::default();
//!
//! let mut stack = WifiStack::new(device, mac, &mut socket_storage);
//! stack.register_rx().expect("register RX callback");
//! ```

use crate::wifi::device::WifiDevice;
use crate::wifi::ip::{Ipv4Addr, Ipv4Info};
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet, SocketStorage};
use smoltcp::socket::dhcpv4;
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpCidr, Ipv4Address};

/// Get the current timestamp as a smoltcp [`Instant`].
///
/// Uses the SDK's `esp_timer_get_time()` which returns microseconds since boot.
pub fn now() -> Instant {
    let us = unsafe { esp8266_wifi_sys::esp_timer_get_time() };
    Instant::from_millis(us / 1000)
}

/// High-level smoltcp network stack.
///
/// Owns the smoltcp [`Interface`] and borrows a [`SocketSet`]. The caller
/// provides all storage (socket buffers, socket set entries) on the stack —
/// no heap allocation is required.
pub struct WifiStack<'a> {
    device: WifiDevice,
    iface: Interface,
    sockets: SocketSet<'a>,
    dhcp_handle: SocketHandle,
}

impl<'a> WifiStack<'a> {
    /// Create a new network stack with DHCP enabled.
    ///
    /// - `device`: The WiFi device (created via `WifiDevice::new`)
    /// - `mac`: The 6-byte MAC address of the WiFi interface
    /// - `socket_storage`: Caller-provided backing storage for the socket set;
    ///   one entry per concurrent socket (including the internal DHCP socket).
    ///   Provide at least 2 entries (1 DHCP + 1 user socket).
    pub fn new(
        mut device: WifiDevice,
        mac: [u8; 6],
        socket_storage: &'a mut [SocketStorage<'a>],
    ) -> Self {
        let config = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
        let mut iface = Interface::new(config, &mut device, now());

        // Start with no IP — DHCP will assign one.
        iface.update_ip_addrs(|addrs| {
            let _ = addrs.push(IpCidr::new(Ipv4Address::UNSPECIFIED.into(), 0));
        });

        let mut sockets = SocketSet::new(socket_storage);

        // Add DHCP socket.
        let dhcp_socket = dhcpv4::Socket::new();
        let dhcp_handle = sockets.add(dhcp_socket);

        Self {
            device,
            iface,
            sockets,
            dhcp_handle,
        }
    }

    /// Register the RX callback with the WiFi driver.
    ///
    /// Must be called after `esp_wifi_start()`.
    pub fn register_rx(&self) -> Result<(), crate::wifi::WifiError> {
        self.device.register_rx_callback()
    }

    /// Drive the network stack: process incoming/outgoing packets and DHCP.
    ///
    /// Returns `true` if socket state may have changed (caller can use this
    /// to decide whether to yield or sleep).
    pub fn poll(&mut self) -> bool {
        let result = self.iface.poll(now(), &mut self.device, &mut self.sockets);
        let changed = matches!(result, smoltcp::iface::PollResult::SocketStateChanged);

        // Check for DHCP events.
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

    /// Returns `true` if the interface has a non-zero IP address.
    pub fn has_ip(&self) -> bool {
        self.iface
            .ipv4_addr()
            .is_some_and(|ip| !ip.is_unspecified())
    }

    /// Get the current IPv4 configuration, if any.
    pub fn ip_info(&mut self) -> Option<Ipv4Info> {
        let ip = self.iface.ipv4_addr()?;
        if ip.is_unspecified() {
            return None;
        }
        // Extract prefix length from the CIDR to compute netmask.
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

        // Gateway: check the default IPv4 route.
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

    /// Async wait until an IP address is obtained or timeout.
    ///
    /// Yields to the embassy executor between polls, allowing WiFi blob tasks
    /// to run via the compat scheduler.
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
            // Yield to embassy executor (and compat scheduler via tick ISR).
            embassy_time::Timer::after(embassy_time::Duration::from_millis(10)).await;
        }
    }

    /// Blocking wait until an IP address is obtained or timeout (in
    /// milliseconds).
    ///
    /// Returns `true` if an IP was obtained, `false` on timeout.
    ///
    /// This uses a timer-based busy-wait instead of `vTaskDelay` because the
    /// main thread is not a FreeRTOS task — calling `vTaskDelay` from non-task
    /// context corrupts the compat scheduler state. The compat scheduler's
    /// blob tasks still run via ISR-driven context switches (tick ISR / PendSV).
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
            // Brief spin — compat scheduler blob tasks run via ISR preemption,
            // so we don't need to explicitly yield to them.
            core::hint::spin_loop();
        }
    }

    /// Async work loop: continuously polls the network stack.
    ///
    /// This is useful when you want to drive the smoltcp stack as a background
    /// embassy task. Never returns.
    pub async fn work(&mut self) -> ! {
        loop {
            self.poll();
            embassy_time::Timer::after(embassy_time::Duration::from_millis(10)).await;
        }
    }

    /// Get mutable access to the socket set (e.g. to add TCP sockets).
    pub fn sockets_mut(&mut self) -> &mut SocketSet<'a> {
        &mut self.sockets
    }

    /// Get a reference to the smoltcp interface.
    pub fn iface(&self) -> &Interface {
        &self.iface
    }

    /// Get mutable access to the smoltcp interface.
    pub fn iface_mut(&mut self) -> &mut Interface {
        &mut self.iface
    }

    /// Get mutable access to both the interface and socket set.
    pub fn iface_and_sockets_mut(&mut self) -> (&mut Interface, &mut SocketSet<'a>) {
        (&mut self.iface, &mut self.sockets)
    }
}
