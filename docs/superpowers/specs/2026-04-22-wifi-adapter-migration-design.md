# ESP8266 WiFi Adapter Migration Design

## Status

- Date: 2026-04-22
- Scope: second WiFi redesign subproject
- Depends on: `wifi_core` foundation from `docs/superpowers/specs/2026-04-22-wifi-core-api-design.md`
- Focus: packet ingress, link events, `smoltcp` device integration, and `embassy-net` integration
- Excludes: scan, promiscuous mode, ESP-NOW, WPS, and broader control-plane migration

## Purpose

This document defines the next redesign slice after the new `wifi_core`
foundation. The first subproject established a strong-typestate,
caller-owned, runtime-neutral core API. The remaining unsafe and global-heavy
networking surface still lives in the legacy packet and event path.

This subproject moves the packet and link adapter path onto `wifi_core` so the
canonical networking flow becomes:

`SDK callbacks -> wifi_core resources -> typed link handle -> network adapters`

The goal is to make `wifi_core` the ownership and state center for network
ingress and link state, while reducing the legacy `wifi` packet and event
modules to thin compatibility layers where they still need to exist.

## Current Evidence And Problems

The working `tcp_server.rs` path proves the platform and the current
`embassy-net` integration are viable. However, the current packet and event
surface still has structural problems that block the full redesign:

- `src/wifi/device.rs` owns static RX rings and static wake storage
- `src/wifi/event.rs` owns global event publication and networking link-state
  signaling
- adapter code still treats legacy packet/event modules as the source of truth
- the callback path is split across several global mutation points instead of a
  single audited resource boundary
- the current networking path still relies on leaked or permanently installed
  global state rather than caller-owned resources

These problems are specifically at odds with the already approved foundation:

- strong typestate
- caller-owned long-lived storage
- narrow, auditable unsafe boundaries
- explicit overflow behavior
- Rust-side control of buffering and wakeup semantics

## Goals

- make `wifi_core` the source of truth for RX ingress and link events
- remove global RX rings and global networking wake slots from the adapter path
- move callback-owned mutable state into caller-owned `WifiResources`
- keep the SDK callback ingress allocation-free and non-blocking
- expose a typed borrow-based link handle that higher-level adapters consume
- rewrite `smoltcp` and `embassy-net` integration as thin layers over that
  link handle
- preserve the already verified runtime shape closely enough that
  `tcp_server.rs` remains the proof target

## Non-Goals

- redesigning the full WiFi control plane in this subproject
- preserving the legacy adapter internals
- adding heap allocation to packet ingress
- introducing a runtime requirement into the `wifi_core` API
- migrating all advanced WiFi features at the same time

## Architecture Overview

The architecture for this subproject is a strict extension of the first
subproject’s ownership model:

- `wifi_core` remains the owner of packet and link resource state
- `src/wifi/core/target.rs` remains the only ESP8266-specific ingress point
  for SDK callbacks
- network adapters remain outside the core and consume typed packet/event
  access instead of touching SDK callback state directly

The packet and link data flow becomes:

1. the SDK emits a WiFi event callback or RX callback
2. `src/wifi/core/target.rs` recovers the installed callback context from the
   single audited callback slot
3. the target callback converts raw SDK input into compact Rust-side event or
   RX record values
4. the callback records those values into caller-owned `WifiResources`
5. the callback wakes the relevant registered waiter
6. adapter code drains packets and observes link transitions through typed
   `wifi_core` handles

This architecture intentionally removes the legacy adapter path from the role
of state owner. Legacy modules may remain during migration, but they become
observers or compatibility shims rather than the canonical networking path.

## Safety Model

The callback safety model is the central design rule for this subproject.

### Callback-Owned Mutation

All callback-owned mutation for the networking path must happen through
caller-owned `WifiResources`.

That means:

- the event store in `WifiResources` owns event backlog, subscriber positions,
  and link wait wakeups
- the RX store in `WifiResources` owns packet staging, packet wakeups, and drop
  accounting
- the target callback trampoline performs only minimal unsafe recovery of the
  installed callback context, then delegates to safe helper methods

### Unsafe Boundary

Unsafe code in the callback path must stay tiny and explicit:

- recover the installed context pointer from the backend-local callback slot
- reborrow the pointed-to resource container
- call narrow safe helpers to publish events and enqueue RX records
- release SDK ownership immediately when ingress cannot retain the packet

No other module in the migrated networking path may own a parallel global event
queue, a parallel global RX ring, or callback-private mutable ingress storage.

### Invariants

The following invariants define the safety contract:

- exactly one live ESP8266 target callback context may be installed at a time
- the installed callback context always points into caller-owned
  `WifiResources`
- no callback path mutates networking state outside that installed context
- callback ingress is allocation-free and non-blocking
- packet overflow is explicit and counted
- event overflow is explicit and counted
- dropped RX packets release SDK ownership immediately
- at most one destructive RX consumer exists for a given queue, enforced by
  borrowing and typed handles
- async wake registration is resource-owned rather than backend-private

## Component Design

## `wifi_core::resources`

`WifiResources` remains the root of caller-owned state and grows the remaining
networking stores needed by adapters.

It owns:

- bounded event storage
- event subscriber or waiter wake registration
- bounded RX storage
- RX wake registration
- ingress diagnostics such as queue depth, drop count, and overflow count

This keeps all long-lived networking state explicit in Rust types and borrowed
from the caller.

## `wifi_core::event`

The core event system remains the canonical link/control event path for the
networking adapters.

It is extended to support callback-safe wake registration and explicit overflow
accounting. Its responsibilities are:

- store compact link/control events
- support blocking and async waiting
- support callback-safe wakeup for waiting tasks
- surface explicit overflow accounting

For the migrated networking path, the event system becomes the source of truth
for link-up/link-down observation instead of legacy `src/wifi/event.rs`.

## `wifi_core::packet`

The packet module remains the canonical RX staging layer and grows the
mechanics adapters require for efficient draining.

It owns:

- raw RX record staging
- packet arrival wake registration
- explicit drop accounting
- typed receive-side ownership transfer

The packet queue remains bounded and allocation-free. Overflow must release SDK
ownership immediately and increment visible counters.

## `wifi_core::interface`

This subproject adds a packet-facing borrowed capability handle for the started
station mode.

The recommended shape is `sta_link()`, because it describes the boundary
precisely: link events and packet ingress/egress without implying a full
network stack.

`StaLink` is responsible for exposing exactly the primitives higher-level
adapters need:

- register a link waker
- register an RX waker
- observe current link state
- receive queued packets
- transmit raw frames through the backend
- expose queue and drop diagnostics where needed

This keeps adapter APIs narrow and prevents `device.rs` or `embassy_net.rs`
from rebuilding a second ownership model outside `wifi_core`.

## `wifi_core::target`

The ESP8266 target backend remains the only SDK-facing ingress layer for the
migrated packet and link path.

Its responsibilities become:

- install and clear the single backend-local callback registration slot
- bind and unbind SDK callbacks during driver lifecycle transitions
- translate SDK WiFi events into compact core events
- translate SDK RX callbacks into `RawRxRecord` values
- enqueue those values into `WifiResources`
- wake registered waiters
- release SDK packet ownership on overflow

The target backend must not become a second event or packet owner. Its role is
translation and ingress, not queue ownership.

## Public API Shape

The public API remains driver-centered and borrow-based.

For started station-capable states, the driver grows a single packet/link
capability constructor:

- `driver.sta_link()`

That handle becomes the boundary consumed by adapter code. The exact method set
for this subproject is:

- `register_link_waker(&self, &Waker)`
- `register_rx_waker(&self, &Waker)`
- `link_state(&self) -> LinkState`
- `receive(&mut self, &mut [u8]) -> Option<usize>`
- `transmit(&mut self, &[u8]) -> Result<(), DriverError>`

The important design property is not the precise naming of every helper. It is
that packet and link adapter code consumes one typed, borrowed handle backed by
`wifi_core`, rather than touching SDK callback state or global rings directly.

## Adapter Design

## `src/wifi/device.rs`

`src/wifi/device.rs` is rewritten as a thin `smoltcp::phy::Device` adapter over
`StaLink`.

After migration, it must no longer own:

- static RX rings
- static wake storage
- direct SDK RX callback registration

Its responsibilities become:

- borrow `StaLink`
- adapt RX draining into the `smoltcp::phy::Device` receive path
- adapt raw TX into the `smoltcp::phy::Device` transmit path
- surface device capabilities

This makes `device.rs` a pure adapter instead of a state owner.

## `src/wifi/embassy_net.rs`

`src/wifi/embassy_net.rs` is rewritten as a thin runner and channel bridge over
`StaLink`.

After migration, it must:

- use `wifi_core` link events as the source of truth for link state
- use `StaLink` RX primitives as the source of truth for packet ingress
- use `StaLink` transmit primitives for packet egress

It must no longer depend on legacy event bus state or legacy static RX ring
state for its own correctness.

## `src/wifi/event.rs`

The legacy event module remains only as a compatibility layer for unmigrated
APIs during this subproject.

It is explicitly not the source of truth for the migrated networking adapter
path. If compatibility mirroring is needed, it should be downstream from
`wifi_core` or the target callback translation rather than upstream of the new
core resources.

## Error And Overflow Behavior

Error and overflow semantics must be explicit and Rust-visible.

### RX Overflow

When the RX ring is full:

- the callback must release SDK ownership immediately
- the RX drop counter must increment
- no blocking or allocation may occur

### Event Overflow

When the event queue is full:

- overflow must be visible as explicit accounting in the core resource state
- the implementation must not silently lose state unless the queue policy
  explicitly says to do so
- if compaction or “latest wins” behavior is used for a specific wait path, it
  must be documented and intentional

### Link State

Adapters must not invent their own link truth. They observe link state from the
core event path and react accordingly:

- connected or started events move the observed link state up
- disconnected or stopped events move the observed link state down
- transmit and receive paths must stop pretending the link is usable when the
  driver is down or disconnected

### Lifecycle

Packet/link handles become invalid through borrowing and typestate, not through
best-effort runtime checks after the driver has stopped.

## Diagnostics

The migrated path should expose Rust-side diagnostics for inspection and tests.

The expected diagnostic surface includes:

- queued RX depth
- RX drop count
- event overflow count
- current link state
- last disconnect reason where available

These diagnostics belong in or immediately above `wifi_core`, not in adapter
private globals.

## Migration Plan

This subproject is intentionally staged.

### Phase 1: Extend Core Resources And Link Handle

- add callback-safe wake registration to the core event and packet stores
- add the station packet/link capability handle to `wifi_core`
- add host tests for wake registration, queue draining, and typestate shape

### Phase 2: Switch Target Callback Ingress To Core Resources

- route WiFi events and RX callbacks through `src/wifi/core/target.rs`
- publish core events and RX records into caller-owned resources
- release SDK packet ownership on overflow

### Phase 3: Rewrite `src/wifi/device.rs`

- replace static RX storage and static wake storage with `StaLink`
- keep the adapter thin and `smoltcp`-focused

### Phase 4: Rewrite `src/wifi/embassy_net.rs`

- consume `StaLink` and core link events
- make the runner a bridge instead of an owner of networking state

### Phase 5: Demote Legacy Event Networking Duties

- reduce `src/wifi/event.rs` to compatibility-only responsibilities for
  unmigrated APIs
- remove its role as the source of truth for the networking adapter path

## Verification Strategy

Verification follows the same correctness-first pattern as the core
subproject.

### Host Verification

Add host-native tests for:

- callback-safe event wake behavior
- callback-safe RX wake behavior
- RX overflow and ownership-release behavior
- `StaLink` borrow and typestate behavior
- adapter-level fake-backend behavior where possible

### Xtensa Verification

Run targeted Xtensa `cargo check` commands for:

- the existing `wifi_core` example
- the migrated `tcp_server` path

### Hardware Verification

Use `examples/tcp_server.rs` as the proof target:

- preserve the already validated runtime shape as closely as practical
- verify the migrated path still honors the known-good SDK-style connect flow
- verify the runtime still fits the validated timing budget in `research.md`

## Success Criteria

This subproject is complete when:

- the canonical packet and link path flows through `wifi_core`
- `src/wifi/device.rs` no longer owns static RX or wake state
- `src/wifi/embassy_net.rs` consumes `StaLink` and core link events
- legacy global event and RX machinery are no longer the networking source of
  truth
- host tests cover the new resource and adapter invariants
- the migrated Xtensa examples compile
- hardware validation of `tcp_server.rs` remains successful

## Scope Check

This document intentionally limits the work to the packet and link adapter
surface. Scan, AP control-plane redesign, and advanced features remain later
subprojects. That keeps the implementation plan focused enough to execute and
verify as one migration slice.
