# Possible upstream contributions

## smoltcp: learn an Ethernet IPv4 neighbor from safe inbound unicast traffic

**Problem:** `smoltcp` 0.12 refreshes an IPv4 neighbor entry from inbound Ethernet IPv4 traffic only when the neighbor already exists. On tiny WiFi targets, a peer can already have our MAC address while our freshly booted stack has not yet seen that peer's ARP request. The first response then has to wait for a separate ARP exchange even though the inbound Ethernet frame already carries the sender IP/MAC binding.

**Local workaround:** this crate now uses published `smoltcp` and injects a tiny synthetic ARP reply into the RX path after a validated unicast IPv4 frame. That exercises upstream's existing ARP cache path without patching `smoltcp`.

**Why `reset_expiry_if_existing` is insufficient:** it preserves an existing cache entry but intentionally refuses to create the first one. That is conservative, but it means the first inbound TCP response can be queued behind ARP resolution even when the Ethernet source MAC and IPv4 source address have already been received in the same validated frame. The proposed change mirrors the narrow, safe subset of lwIP-style IP/MAC trust while keeping same-subnet and unicast gates.

**Patch target:** the diff below is against `smoltcp 0.12.0`, which is the published version currently resolved by this workspace. Re-check the surrounding `iface::interface` internals before opening an upstream PR against a newer smoltcp revision.

**Upstream improvement:** teach `smoltcp` to fill the neighbor cache from an inbound IPv4 packet only when all of these are true:

- the medium is Ethernet;
- the destination IPv4 address is unicast and accepted by the interface;
- the source IPv4 address is in the same configured network;
- the source hardware address is unicast.

```diff
diff --git a/src/iface/interface/ipv4.rs b/src/iface/interface/ipv4.rs
@@
         #[cfg(feature = "medium-ethernet")]
         if self.is_unicast_v4(ipv4_repr.dst_addr) {
-            self.neighbor_cache.reset_expiry_if_existing(
-                IpAddress::Ipv4(ipv4_repr.src_addr),
-                source_hardware_addr,
-                self.now,
-            );
+            let source_protocol_addr = IpAddress::Ipv4(ipv4_repr.src_addr);
+
+            if matches!(self.caps.medium, Medium::Ethernet)
+                && self.in_same_network(&source_protocol_addr)
+                && source_hardware_addr.is_unicast()
+            {
+                self.neighbor_cache
+                    .fill(source_protocol_addr, source_hardware_addr, self.now);
+            } else {
+                self.neighbor_cache.reset_expiry_if_existing(
+                    source_protocol_addr,
+                    source_hardware_addr,
+                    self.now,
+                );
+            }
         }
```

**Regression test:**

```diff
diff --git a/src/iface/interface/tests/ipv4.rs b/src/iface/interface/tests/ipv4.rs
@@
+#[rstest]
+#[case(Medium::Ethernet)]
+#[cfg(feature = "medium-ethernet")]
+fn test_unicast_ipv4_learns_neighbor_from_ethernet_source(#[case] medium: Medium) {
+    let (mut iface, mut sockets, _device) = setup(medium);
+
+    let local_ip_addr = Ipv4Address::new(192, 168, 0, 116);
+    let remote_ip_addr = Ipv4Address::new(192, 168, 0, 221);
+    let remote_hw_addr = EthernetAddress([0x08, 0x5b, 0xd6, 0xcb, 0xe8, 0x19]);
+
+    iface.update_ip_addrs(|addrs| {
+        addrs.iter_mut().next().map(|addr| {
+            *addr = IpCidr::Ipv4(Ipv4Cidr::new(local_ip_addr, 24));
+        });
+    });
+
+    let repr = Ipv4Repr {
+        src_addr: remote_ip_addr,
+        dst_addr: local_ip_addr,
+        next_header: IpProtocol::Unknown(0x0c),
+        payload_len: 0,
+        hop_limit: 64,
+    };
+
+    let mut bytes = vec![0u8; repr.buffer_len()];
+    repr.emit(
+        &mut Ipv4Packet::new_unchecked(&mut bytes),
+        &ChecksumCapabilities::default(),
+    );
+    let frame = Ipv4Packet::new_unchecked(&bytes[..]);
+
+    let _ = iface.inner.process_ipv4(
+        &mut sockets,
+        PacketMeta::default(),
+        HardwareAddress::Ethernet(remote_hw_addr),
+        &frame,
+        &mut iface.fragments,
+    );
+
+    assert_eq!(
+        iface.inner.lookup_hardware_addr(
+            MockTxToken,
+            &IpAddress::Ipv4(remote_ip_addr),
+            &mut iface.fragmenter,
+        ),
+        Ok((HardwareAddress::Ethernet(remote_hw_addr), MockTxToken))
+    );
+}
```
