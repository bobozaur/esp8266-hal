# ESP8266 WiFi Full-Functionality Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:executing-plans` to implement this plan task-by-task in the current tree. Track progress by updating the checkbox items in this file.

## Table of Contents
- [Goal](#goal)
- [Context and Constraints](#context-and-constraints)
- [Recommended Approach](#recommended-approach)
- [Alternatives Considered](#alternatives-considered)
- [Architecture](#architecture)
- [File Map](#file-map)
- [Implementation Tasks](#implementation-tasks)
- [Verification](#verification)
- [Risks and Edge Cases](#risks-and-edge-cases)
- [Open Questions](#open-questions)

## Goal

Make the entire WiFi subsystem fully functional, strongly typed, caller-owned, and hardware-validated on real ESP8266 hardware across the full in-tree surface:

- `wifi-sta`
- `wifi-ap`
- `wifi-embassy-net`
- `wifi-wps`
- `wifi-smartconfig`
- `wifi-promiscuous`
- `wifi-esp-now`

The design center is the strong-typestate `wifi_core` API. There is no backwards-compatibility requirement for the legacy `WifiController`-style surface. Anything that can reasonably live in Rust should move into Rust; the SDK/blob boundary should be thin, explicit, and audited.

## Context and Constraints

- The current repository already contains a working `wifi_core` foundation under `src/wifi/core/`.
- The currently hardware-validated path is station mode plus `embassy-net`, with `examples/tcp_server.rs` proving DHCP + inbound TCP echo on real hardware.
- The current tree still exposes older controller-style modules under `src/wifi/`, and those modules are not the design center for this milestone.
- `vendor/smoltcp` is currently patched to restore same-subnet inbound IPv4 neighbor learning for the ESP8266 path. This plan keeps that patch in place unless a verified Rust-side replacement removes the need for it.
- The blob-side WiFi NVS path is not considered production-ready yet; the plan assumes explicit Rust-owned configuration and an opt-in persistence policy instead of blindly enabling blob-managed NVS.
- Hardware validation is required for everything, not just compile checks and host tests.

## Recommended Approach

Use a foundation-first rewrite centered on `wifi_core`, then rebuild the outer WiFi surface as typed feature modules on top of that core.

Why this is the recommended path:

- It fixes ownership, invalid state transitions, callback routing, and packet/event flow at the root instead of hardening old APIs piecemeal.
- It keeps the pure-Rust logic testable on the host while confining ESP8266-specific unsafe code to a narrow backend layer.
- It gives each feature a hardware-gated completion criterion, which fits the requirement that “done” means real boards and real radios.

This plan does **not** aim to preserve legacy controller semantics. The final surface should be the best driver we can write, not the least disruptive migration.

## Alternatives Considered

### 1. Vertical Slice Completion

Finish station first, then AP, then provisioning, then special modes, each on the current surface.

- Good: faster visible progress
- Bad: tends to preserve the wrong abstractions
- Bad: repeats lifecycle/event fixes in each subsystem

### 2. Compatibility-First Parity Pass

Patch the current `src/wifi/*.rs` surface until every feature works.

- Good: smallest immediate diff
- Bad: locks in legacy API shape
- Bad: pushes typestate and ownership correctness to the side
- Bad: makes later cleanup harder

### 3. Recommended: Typestate-First Rebuild

Treat `wifi_core` as the only design center, rebuild features on top, then remove or collapse the legacy surface.

- Good: best long-term correctness
- Good: strongest ownership model
- Good: easiest to reason about and host-test
- Cost: largest refactor up front

## Architecture

### Public Layers

The finished subsystem should have three layers:

1. **Core foundation** in `src/wifi/core/`
   - typestate driver
   - validated configuration
   - caller-owned resources
   - event broker
   - packet ingress/egress ownership
   - runtime-neutral backend trait

2. **ESP8266 backend** in `src/wifi/`
   - audited FFI boundary
   - compat scheduler integration
   - executor/task integration
   - packet and event bridge into `wifi_core`

3. **Typed feature surfaces** in `src/wifi/`
   - station
   - AP
   - AP+STA
   - scan
   - networking adapters
   - WPS
   - SmartConfig
   - ESP-NOW
   - promiscuous mode

### State Graph

The driver lifecycle should be explicit in types rather than implicit in runtime checks:

```text
Uninitialized
  -> Initialized
  -> Configured<StaMode | ApMode | MixedMode>
  -> Started<...>
  -> Active session handles / links / feature sessions
```

Important rules:

- `Started<StaMode>` can create station, scan, and network-link handles.
- `Started<ApMode>` can create AP handles and AP-network services.
- `Started<MixedMode>` can borrow both station and AP capabilities simultaneously.
- WPS and SmartConfig require station capability and should be unavailable in invalid mode combinations.
- Promiscuous capture and ESP-NOW should encode their coexistence rules in Rust types instead of ad hoc runtime checks where possible.

### Rust-First Ownership Model

Rust owns:

- configuration validation
- lifecycle transitions
- event fanout
- packet-ring bookkeeping
- callback registration ownership
- peer bookkeeping for ESP-NOW
- provisioning result parsing
- promiscuous frame metadata parsing
- AP-side helper services that can be implemented outside the blob

The ESP8266 SDK/blob owns only what cannot reasonably move:

- radio/firmware execution
- SDK entry points
- WiFi firmware algorithms internal to the blob

### Runtime Model

The core remains runtime-neutral. Runtime-specific support stays outside the core:

- blocking waits are adapters over the core event broker
- `embassy-net` integration remains outside `wifi_core`
- host tests target the pure core directly
- xtensa compile checks validate the target backend and examples

### Networking Model

Networking should be layered rather than fused into the core:

- `wifi_core` exposes typed station link ownership and packet transfer
- `src/wifi/embassy_net.rs` adapts the station link to `embassy-net`
- AP-side IP services that are not inherently blob-owned should be implemented in Rust
- the vendored `smoltcp` patch remains until a verified replacement or upstream fix exists

Out of scope for this plan:

- NAT/router features
- preserving the exact legacy `WifiController` API
- hiding all unsafe code; the goal is to confine and audit it

## File Map

This plan assumes the existing `src/wifi/core/` tree stays and is extended. The outer `src/wifi/` tree is treated as rewriteable.

### Core Foundation

- `src/wifi/core/mod.rs`
- `src/wifi/core/backend.rs`
- `src/wifi/core/config.rs`
- `src/wifi/core/diag.rs`
- `src/wifi/core/driver.rs`
- `src/wifi/core/error.rs`
- `src/wifi/core/event.rs`
- `src/wifi/core/interface.rs`
- `src/wifi/core/packet.rs`
- `src/wifi/core/resources.rs`
- `src/wifi/core/state.rs`
- `src/wifi/core/target.rs`
- `src/wifi/core/waker.rs`
- `src/wifi/core/locked.rs`

### Outer WiFi Surface

- `src/wifi/mod.rs`
- `src/wifi/config.rs`
- `src/wifi/error.rs`
- `src/wifi/event.rs`
- `src/wifi/scan.rs`
- `src/wifi/sta.rs`
- `src/wifi/ap.rs`
- `src/wifi/device.rs`
- `src/wifi/ip.rs`
- `src/wifi/stack.rs`
- `src/wifi/embassy_net.rs`
- `src/wifi/wps.rs`
- `src/wifi/smartconfig.rs`
- `src/wifi/esp_now.rs`
- `src/wifi/promiscuous.rs`
- `src/wifi/executor.rs`
- `src/wifi/sdk/*`
- `src/wifi/compat/*`

### New Supporting Code Expected

- `xtask/Cargo.toml`
- `xtask/src/main.rs`
- `xtask/src/wifi_hw.rs`
- `tests/wifi_core_*.rs`
- `tests/wifi_*.rs`
- `examples/wifi_ap.rs`
- `examples/wifi_ap_sta.rs`
- `examples/wifi_wps.rs`
- `examples/wifi_smartconfig.rs`
- `examples/wifi_esp_now_tx.rs`
- `examples/wifi_esp_now_rx.rs`
- `examples/wifi_promisc.rs`

### Existing Examples To Keep And Upgrade

- `examples/wifi_core_connect.rs`
- `examples/raw_scan.rs`
- `examples/tcp_client.rs`
- `examples/tcp_server.rs`

### Documentation And Evidence

- `research.md`
- `Cargo.toml`
- `.github/workflows/ci.yaml`
- `vendor/smoltcp/*`

## Implementation Tasks

### Task 1: Build The Hardware Validation Harness

**Why:** Hardware validation is a core requirement for this project, so the test harness has to exist before the feature waves start.

**Files:**
- Create: `xtask/Cargo.toml`
- Create: `xtask/src/main.rs`
- Create: `xtask/src/wifi_hw.rs`
- Modify: `Cargo.toml`
- Modify: `research.md`

**Steps:**
- [x] Add an `xtask` helper for hardware scenario orchestration, UART capture, optional packet capture hooks, and evidence directory layout under `/tmp` or a configurable artifacts root.
- [x] Define named scenarios for `sta_connect`, `sta_dhcp`, `tcp_server`, `tcp_client`, `ap_start`, `ap_station_list`, `ap_dhcp`, `ap_sta`, `wps`, `smartconfig`, `esp_now`, and `promisc`.
- [x] Standardize evidence collection so each scenario records build command, flash command, UART log, and any companion host-side logs.
- [x] Document the hardware prerequisites in `research.md`: AP/router, second ESP8266 for ESP-NOW, client device for AP mode, and phone/router support for SmartConfig and WPS.

**Verification:**
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw list`
- Expect: every required scenario is discoverable from one place
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw doctor`
- Expect: missing host-side dependencies are reported clearly before feature work starts

### Task 2: Finish The Core Typestate And Resource Model

**Why:** The core has to fully encode valid ownership and mode transitions before higher-level features can be rebuilt correctly.

**Files:**
- Modify: `src/wifi/core/mod.rs`
- Modify: `src/wifi/core/backend.rs`
- Modify: `src/wifi/core/config.rs`
- Modify: `src/wifi/core/driver.rs`
- Modify: `src/wifi/core/error.rs`
- Modify: `src/wifi/core/event.rs`
- Modify: `src/wifi/core/interface.rs`
- Modify: `src/wifi/core/packet.rs`
- Modify: `src/wifi/core/resources.rs`
- Modify: `src/wifi/core/state.rs`
- Modify: `src/wifi/core/waker.rs`
- Create/modify: `tests/wifi_core_*.rs`

**Steps:**
- [x] Make mode, lifecycle, and feature capability transitions explicit in the type system, including `StaMode`, `ApMode`, and `MixedMode`.
- [x] Move all reusable validation into Rust newtypes and config objects, including SSID, credentials, channel selection, AP config, mixed-mode config, and provisioning parameter validation.
- [x] Expand caller-owned resources so event slots, RX records, subscribers, peer storage, and other bounded queues are explicit capacities rather than hidden globals.
- [x] Ensure the event broker supports both async and polling/blocking adapters without runtime-specific assumptions.
- [x] Encode cancellation, timeout, and error semantics in core error types instead of letting outer layers guess.

**Verification:**
- Run: `cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --lib --tests -q`
- Expect: pure core host tests pass without enabling `wifi`
- Run: `source .envrc && cargo check --target xtensa-esp8266-none-elf -Zbuild-std=core --no-default-features --features wifi-core`
- Expect: the core still cross-compiles without target backend assumptions

### Task 3: Rebuild The ESP8266 Backend And Compat Boundary

**Why:** The unsafe target boundary is where lifecycle bugs, callback bugs, and hidden global state tend to leak in. This must be tightened before the feature surfaces become trustworthy.

**Files:**
- Modify: `src/wifi/core/target.rs`
- Modify: `src/wifi/executor.rs`
- Modify: `src/wifi/sdk/cache.rs`
- Modify: `src/wifi/sdk/delay.rs`
- Modify: `src/wifi/sdk/mod.rs`
- Modify: `src/wifi/sdk/phy_init.rs`
- Modify: `src/wifi/sdk/system.rs`
- Modify: `src/wifi/sdk/wifi_init.rs`
- Modify: `src/wifi/compat/*`
- Modify: `procmacros/src/lib.rs`
- Modify: `src/wifi/mod.rs`

**Steps:**
- [ ] Route all blob callbacks through one audited backend-owned registration slot that points into caller-owned resources rather than feature-local globals.
- [ ] Eliminate or reduce legacy state atoms where the new core can own the state directly.
- [ ] Finish the compat scheduler, queue, timer, and critical-section glue so blob task scheduling and wakeups are deterministic and reviewable.
- [x] Make the NVS policy explicit: default to Rust-managed runtime config, keep blob-side WiFi NVS disabled until a dedicated validation pass proves it safe.
- [ ] Keep target-only unsafe code local to backend and compat modules; move parsing and bookkeeping out to safe Rust wherever possible.

**Verification:**
- Run: `source .envrc && cargo check --example wifi_core_connect --target xtensa-esp8266-none-elf -Zbuild-std=core --no-default-features --features interrupt,rt,wifi-core,wifi,wifi-sta`
- Expect: target backend compiles cleanly against the new core
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw run sta_connect`
- Expect: board boots, starts WiFi, associates, and cleanly disconnects/restarts across repeated runs

### Task 4: Rebuild Station, Scan, And Reconnect Behavior

**Why:** Station mode is the foundation for the rest of the networked feature set and should remain the strongest path in the tree.

**Files:**
- Modify: `src/wifi/scan.rs`
- Modify: `src/wifi/sta.rs`
- Modify: `src/wifi/config.rs`
- Modify: `src/wifi/error.rs`
- Modify: `src/wifi/event.rs`
- Modify: `examples/wifi_core_connect.rs`
- Modify: `examples/raw_scan.rs`
- Create/modify: `tests/wifi_sta_*.rs`

**Steps:**
- [ ] Replace controller-style station flows with typed station handles and sessions derived from `Started<StaMode>` or `Started<MixedMode>`.
- [x] Expose active scan, directed scan, channel-limited scan, and BSSID-directed scan through validated Rust config types.
- [ ] Implement connect, disconnect, reconnect, and disconnect-reason reporting around typed state transitions instead of global polling.
- [ ] Keep connection progress and final reasons in the unified event model so higher layers do not parse blob state ad hoc.
- [x] Preserve the working caller-owned station path already proven by `wifi_core_connect` and `tcp_server`.

**Verification:**
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw run sta_connect`
- Expect: WPA2 association succeeds on repeated cold boots
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw run sta_scan`
- Expect: active scan returns stable parsed AP info, including directed-scan scenarios
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw run sta_reconnect`
- Expect: reconnect after AP interruption succeeds and reports the correct state transitions

**Progress evidence:**
- `sta_connect` passed after lazy event-handler registration: `/tmp/esp8266-hal-wifi-hw/sta_connect-1776923506`
- `sta_scan` passed through typed `wifi_core` scan options and target conversion: `/tmp/esp8266-hal-wifi-hw/sta_scan-1776923633`
- `sta_dhcp` passed after the same backend change: `/tmp/esp8266-hal-wifi-hw/sta_dhcp-1776923696`

### Task 5: Harden Network Integration And IP Services

**Why:** The driver is only truly useful when the link, packet path, and IP-facing adapters are stable under real traffic.

**Files:**
- Modify: `src/wifi/device.rs`
- Modify: `src/wifi/embassy_net.rs`
- Modify: `src/wifi/ip.rs`
- Modify: `src/wifi/stack.rs`
- Modify: `examples/tcp_client.rs`
- Modify: `examples/tcp_server.rs`
- Modify: `vendor/smoltcp/*`
- Create/modify: `tests/wifi_net_*.rs`

**Steps:**
- [x] Keep the networking adapter outside the core, but make link-state, RX/TX wakeups, and packet ownership match the core state model exactly.
- [x] Preserve the split-runner behavior that fixed the prior deadlock class and keep the shortest correct wait/wake path for packet delivery.
- [x] Support both DHCP and static IPv4 station configuration through typed Rust config and events.
- [x] Add AP-side Rust network services that are needed for a usable AP path and are not inherently blob-owned, starting with DHCP lease service if required by the AP examples.
- [x] Retain the vendored `smoltcp` patch until an upstreamed or Rust-side replacement is verified on hardware.

**Verification:**
- Run: `cargo run -p xtask -- wifi-hw run tcp_server`
- Expect: DHCP completes, inbound TCP connect succeeds on first attempt, echo succeeds, and close is clean
- Run: `cargo run -p xtask -- wifi-hw run tcp_client`
- Expect: outbound TCP connect/send/receive works on hardware
- Run: `cargo run -p xtask -- wifi-hw run sta_static_ip`
- Expect: static IPv4 configuration comes up without DHCP

**Progress evidence:**
- `tcp_server` passed inbound TCP echo after widening the harness timeout to cover WiFi connect + DHCP: `/tmp/esp8266-hal-wifi-hw/tcp_server-1776924031`
- `tcp_client` passed outbound TCP against a host-served local TCP endpoint with typestate WiFi core: `/tmp/esp8266-hal-wifi-hw/tcp_client-1776924383`
- `sta_static_ip` passed static IPv4 config plus outbound TCP to the host endpoint: `/tmp/esp8266-hal-wifi-hw/sta_static_ip-1776924778`
- Added a Rust-owned AP DHCP lease service in `wifi_core`, plus AP-side packet-link support in the `embassy-net` bridge. Host tests cover DHCP OFFER/ACK/NAK behavior and AP RX/TX link routing. `xtask wifi-hw run ap_dhcp` is a manual scenario because it requires a client to join the soft-AP during the UART watch window.

### Task 6: Rebuild AP And AP+STA Mode

**Why:** Full functionality requires access-point support and mixed-mode coexistence, not just station mode.

**Files:**
- Modify: `src/wifi/ap.rs`
- Modify: `src/wifi/config.rs`
- Modify: `src/wifi/event.rs`
- Modify: `src/wifi/ip.rs`
- Create: `examples/wifi_ap.rs`
- Create: `examples/wifi_ap_sta.rs`
- Create/modify: `tests/wifi_ap_*.rs`

**Steps:**
- [x] Replace free functions with typed AP handles and running AP sessions derived from `Started<ApMode>` or `Started<MixedMode>`.
- [x] Encode AP config validation in Rust, including SSID, auth mode, channel, client limit, beacon settings, and mixed-mode constraints.
- [x] Expose connected-station inspection and deauthentication from typed AP state rather than from global mode assumptions.
- [x] Implement AP+STA ownership so both interfaces can be driven at once without hidden shared-state races.
- [x] Ensure AP-side IP behavior is explicit and testable, including any Rust DHCP service introduced in Task 5.

**Verification:**
- Run: `cargo run -p xtask -- wifi-hw run ap_start`
- Expect: soft-AP starts and is ready for client join
- Run: `cargo run -p xtask -- wifi-hw run ap_station_list`
- Expect: connected-station enumeration and targeted deauth work
- Run: `cargo run -p xtask -- wifi-hw run ap_dhcp`
- Expect: phone or laptop joins the soft-AP and receives a Rust-served DHCP lease
- Run: `cargo run -p xtask -- wifi-hw run ap_sta`
- Expect: upstream STA connectivity and downstream AP client connectivity coexist on one board

**Progress evidence:**
- `ap_start` passed soft-AP start plus typed station-list query with zero clients: `/tmp/esp8266-hal-wifi-hw/ap_start-1776925352`
- `ap_sta` passed mixed-mode AP start, STA association, AP station-list query, and upstream DHCP: `/tmp/esp8266-hal-wifi-hw/ap_sta-1776925931`
- `ap_start` passed again after adding AP packet-link and DHCP support to the example build: `/tmp/esp8266-hal-wifi-hw/ap_start-1776936029`
- AP DHCP implementation status:
  - `DhcpServer` and `DhcpServerConfig` are implemented in `wifi_core` with caller-owned lease storage and no blob DHCP dependency.
  - `WifiDriver<Started<M: HasAp>>::ap_link()` attaches the AP RX callback and routes AP TX through `WIFI_IF_AP`.
  - `wifi::embassy_net::new_ap(...)` adapts AP links to `embassy-net`.
  - `xtask wifi-hw run ap_dhcp` is manual and emits `ap-dhcp-ok` after a client DHCP ACK.
- Remaining AP validation gap: non-zero AP station-list/deauth and AP DHCP lease assignment still require a phone/laptop/companion client to join during the manual scenarios.

### Task 7: Rebuild Provisioning As Typed Sessions

**Why:** WPS and SmartConfig are stateful workflows with strict preconditions; typestate and explicit session ownership are a strong fit here.

**Files:**
- Modify: `src/wifi/wps.rs`
- Modify: `src/wifi/smartconfig.rs`
- Modify: `src/wifi/event.rs`
- Modify: `src/wifi/error.rs`
- Create: `examples/wifi_wps.rs`
- Create: `examples/wifi_smartconfig.rs`
- Create/modify: `tests/wifi_provision_*.rs`

**Steps:**
- [x] Replace free-function provisioning entry points with typed sessions that can only be started from valid station-capable driver states.
- [x] Move protocol parameter validation, reserved-data parsing, and result decoding into Rust.
- [x] Ensure only one provisioning workflow can own the driver at a time and that stop/disable cleanup is deterministic on success, failure, or timeout.
- [x] Feed provisioning results into the same typed configuration model used by normal station connect flows.
- [x] Keep provisioning feature-gated, but make the enabled path production-quality rather than an afterthought.

**Verification:**
- Run: `cargo run -p xtask -- wifi-hw run wps`
- Expect: WPS PBC or PIN flow completes on supported router hardware and yields usable credentials
- Run: `cargo run -p xtask -- wifi-hw run smartconfig`
- Expect: SmartConfig receives credentials from a phone app, reports them through typed events, and completes cleanup cleanly

**Progress evidence:**
- Typed WPS and SmartConfig sessions are implemented in `wifi_core` with backend-owned start/stop, event cursor ownership, and Drop cleanup.
- WPS and SmartConfig examples cross-compile for `xtensa-esp8266-none-elf` with their dedicated feature sets.
- `xtask` now marks `wps` and `smartconfig` as manual UART scenarios: runnable when the router/phone provisioning workflow is prepared, but excluded from automatic `matrix` runs so missing external setup is not reported as a driver regression.
- Remaining provisioning validation gap: successful WPS enrollment requires a supported router in WPS mode, and successful SmartConfig requires a compatible phone app actively sending credentials during the UART watch window.

### Task 8: Rebuild ESP-NOW Around Owned Sessions And Queues

**Why:** ESP-NOW currently relies on global callback registration patterns that do not match the ownership model we want for the rest of the driver.

**Files:**
- Modify: `src/wifi/esp_now.rs`
- Modify: `src/wifi/core/resources.rs`
- Modify: `src/wifi/event.rs`
- Modify: `src/wifi/error.rs`
- Create: `examples/wifi_esp_now_tx.rs`
- Create: `examples/wifi_esp_now_rx.rs`
- Create/modify: `tests/wifi_esp_now_*.rs`

**Steps:**
- [x] Replace global function-pointer callback storage with queue-backed delivery into caller-owned resources.
- [x] Model ESP-NOW as a typed session with explicit peer-table ownership, PMK configuration, and bounded send/receive buffering.
- [x] Support unicast, broadcast, encrypted peers, and peer mutation through validated Rust types.
- [x] Encode coexistence rules with STA/AP mode in the public API instead of relying on documentation comments alone.
- [x] Keep the callback trampolines thin and move peer/data parsing into safe Rust.

**Verification:**
- Run: `cargo run -p xtask -- wifi-hw run esp_now_unicast`
- Expect: two boards exchange unicast frames reliably
- Run: `cargo run -p xtask -- wifi-hw run esp_now_broadcast`
- Expect: broadcast delivery works to registered peers
- Run: `cargo run -p xtask -- wifi-hw run esp_now_encrypted`
- Expect: encrypted peers can exchange payloads and bad-key cases fail predictably
- Hardware evidence:
  - `esp_now_broadcast`: `/tmp/esp8266-hal-wifi-hw/esp_now_broadcast-1776932990`
    - UART includes `esp-now channel ok`, `esp-now peer ok`, `esp-now send status=success`, `esp-now send-status-ok`, and `esp-now-tx-ok`.
  - `esp_now_broadcast` after mode-typed peer registration: `/tmp/esp8266-hal-wifi-hw/esp_now_broadcast-1776934857`
    - UART includes `esp-now channel ok`, `esp-now send status=success`, `esp-now send-status-ok`, and `esp-now-tx-ok`.
  - `esp_now_unicast` and `esp_now_encrypted` are now manual UART scenarios instead of pending examples. They use `wifi_esp_now_tx` with `ESP8266_HAL_ESPNOW_PEER=aa:bb:cc:dd:ee:ff`; encrypted mode also sets `ESP8266_HAL_ESPNOW_ENCRYPTED=1`. `wifi_esp_now_rx` also cross-compiles with the same peer/encryption env for the companion-board receive role.
  - Remaining ESP-NOW validation gap: unicast, encrypted exchange, and bad-key failure cases require a second working ESP8266 serial target. `/dev/ttyACM0` did not respond as an ESP8266 during probing, so only one-board broadcast TX/callback smoke is currently hardware-proven.

### Task 9: Rebuild Promiscuous Mode As A Capture Session

**Why:** Promiscuous mode is another callback-heavy surface that benefits from explicit ownership, bounded queues, and typed metadata parsing.

**Files:**
- Modify: `src/wifi/promiscuous.rs`
- Modify: `src/wifi/core/resources.rs`
- Modify: `src/wifi/error.rs`
- Create: `examples/wifi_promisc.rs`
- Create/modify: `tests/wifi_promisc_*.rs`

**Steps:**
- [x] Replace the single global promiscuous callback with an owned capture session backed by caller-owned buffers.
- [x] Parse RX metadata and packet-type information in Rust and expose a typed capture record API.
- [x] Support packet filters through validated Rust flags.
- [x] Add control-subtype filters through validated Rust flags.
- [x] Make enable/disable semantics explicit and deterministic, including interaction with normal WiFi operation.
- [x] Document and enforce queue-overflow behavior instead of silently dropping with no policy.

**Verification:**
- Run: `cargo run -p xtask -- wifi-hw run promisc_capture`
- Expect: management, data, and control frames are captured with valid metadata
- Run: `cargo run -p xtask -- wifi-hw run promisc_filters`
- Expect: packet and control filters change capture output exactly as configured
- Hardware evidence:
  - `promisc_capture`: `/tmp/esp8266-hal-wifi-hw/promisc_capture-1776928818`
    - UART includes `promisc filter all`, `promisc channel 6`, `promisc frame type=management ch=6 len=256`, and `promisc-ok`.
  - `promisc_filters`: `/tmp/esp8266-hal-wifi-hw/promisc_filters-1776928777`
    - UART includes `promisc filter management`, `promisc frame type=management ch=6 len=256`, and `promisc-management-ok`.
  - `promisc_filters` after adding typed control-subtype filter setup: `/tmp/esp8266-hal-wifi-hw/promisc_filters-1776934387`
    - UART includes `promisc ctrl filter all`, `promisc frame type=management ch=6 len=256`, and `promisc-management-ok`.
  - Remaining RF coverage gap: ambient channel-6 traffic validated management-frame capture; data/control frame capture and control-subtype filtering still need a traffic-generator scenario.

### Task 10: Collapse The Legacy Surface, Update Docs, And Lock In CI

**Why:** Once the typed feature surfaces are working, the tree should reflect the new design instead of keeping a legacy-first façade alive indefinitely.

**Files:**
- Modify: `src/wifi/mod.rs`
- Modify or remove: `src/wifi/controller.rs`
- Modify: `src/lib.rs`
- Modify: `Cargo.toml`
- Modify: `.github/workflows/ci.yaml`
- Modify: `research.md`
- Modify: `examples/*`

**Steps:**
- [x] Make the new typed WiFi surface the primary public API and remove or sharply reduce the legacy controller-first façade.
- [x] Ensure every maintained example uses the final ownership model rather than temporary compatibility entry points.
- [x] Add host-native tests, xtensa compile checks, and vendored `smoltcp` regression tests to CI where they fit.
- [x] Record every completed hardware scenario and its outcome in `research.md`, including known constraints and the current best evidence locations.
- [x] Leave the tree in a state where “full functionality” means the hardware matrix is reproducible, not tribal knowledge.

**Progress evidence:**
- The `wifi` module docs now lead with the `wifi_core::WifiDriver` typestate quick start, and `wifi::controller` is documented as legacy ESP8266 backend compatibility plumbing.
- Maintained WiFi examples used by `xtask wifi-hw` are typestate/caller-owned-resource examples; older probe examples remain in-tree for debugging history but are not the target public surface.
- CI now runs host `wifi-core` tests, `xtask` tests, vendored `smoltcp` tests, and Xtensa compile checks for the maintained WiFi examples.
- `CARGO_INCREMENTAL=0` is set in CI after the Xtensa toolchain produced an incremental dep-graph ICE while checking repeated WiFi examples; the same failing example passed with incremental disabled.
- The originally listed `cargo check --all-targets --target xtensa-esp8266-none-elf ...` verification is not usable for this no-std target because `--all-targets` also builds lib tests requiring `std`, `test`, `proptest`, and a panic handler. The verified replacement is the maintained-example Xtensa matrix now encoded in CI.
- Fresh automatic hardware matrix passed:
  - Command: `source .envrc && cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw matrix`
  - Result: `wifi-hw matrix: pass`
  - Evidence root: `/tmp/esp8266-hal-wifi-hw`
  - Latest implemented scenario evidence:
    - `sta_connect`: `/tmp/esp8266-hal-wifi-hw/sta_connect-1776937196`
    - `sta_scan`: `/tmp/esp8266-hal-wifi-hw/sta_scan-1776937208`
    - `sta_dhcp`: `/tmp/esp8266-hal-wifi-hw/sta_dhcp-1776937217`
    - `sta_static_ip`: `/tmp/esp8266-hal-wifi-hw/sta_static_ip-1776937231`
    - `tcp_server`: `/tmp/esp8266-hal-wifi-hw/tcp_server-1776937245`
    - `tcp_client`: `/tmp/esp8266-hal-wifi-hw/tcp_client-1776937260`
    - `ap_start`: `/tmp/esp8266-hal-wifi-hw/ap_start-1776937274`
    - `ap_sta`: `/tmp/esp8266-hal-wifi-hw/ap_sta-1776937283`
    - `esp_now_broadcast`: `/tmp/esp8266-hal-wifi-hw/esp_now_broadcast-1776937303`
    - `promisc_capture`: `/tmp/esp8266-hal-wifi-hw/promisc_capture-1776937313`
    - `promisc_filters`: `/tmp/esp8266-hal-wifi-hw/promisc_filters-1776937323`

**Verification:**
- Run: `cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --lib --tests -q`
- Expect: pure core host tests pass
- Run: `source .envrc && cargo check --all-targets --target xtensa-esp8266-none-elf -Zbuild-std=core --no-default-features --features interrupt,rt,wifi-ap,wifi-sta,wifi-embassy-net,wifi-wps,wifi-smartconfig,wifi-promiscuous,wifi-esp-now`
- Expect: the full feature matrix cross-compiles
- Run: `cargo run -p xtask -- wifi-hw matrix`
- Expect: every required hardware scenario reports pass with preserved evidence paths

## Verification

The final verification bar is split into three layers:

### 1. Host-Native Core Validation

- pure `wifi-core` unit and integration tests
- bounded queue/resource invariants
- typestate and configuration validation tests
- no target backend required

### 2. Xtensa Build Validation

- feature-matrix compile checks
- examples compile for `xtensa-esp8266-none-elf`
- vendored `smoltcp` regression tests stay green on host

### 3. Hardware Validation Matrix

| Area | Required proof |
| --- | --- |
| Station | repeated connect, disconnect, reconnect, scan, DHCP, static IPv4 |
| TCP/IP | inbound TCP server, outbound TCP client, no packet-path deadlock |
| AP | AP start, client join, lease assignment, station enumeration, deauth |
| AP+STA | simultaneous upstream and downstream connectivity |
| WPS | successful enrollment on supported router hardware |
| SmartConfig | successful credential receipt from supported phone app |
| ESP-NOW | unicast, broadcast, encrypted, and failure-path validation on two boards |
| Promiscuous | capture and filter behavior with correct metadata |

Every hardware scenario must leave behind evidence that can be referenced from `research.md`.

## Risks and Edge Cases

- **Callback lifetime bugs:** the blob invokes callbacks asynchronously; ownership must remain valid for the full registration window.
- **Mode coexistence rules:** AP+STA, ESP-NOW, and promiscuous interactions can be hardware-specific and must be encoded deliberately.
- **Queue pressure:** RX rings, event subscribers, peer tables, and promiscuous captures need explicit overflow policy and telemetry.
- **NVS scope creep:** blob-side persistence is easy to turn on and hard to verify; keep it off until separately proven.
- **Vendored `smoltcp` debt:** the local patch is justified today, but it remains a maintenance liability until upstreamed or removed safely.
- **Provisioning prerequisites:** WPS and SmartConfig depend on external router/phone compatibility; the harness must document exact supported setups.
- **Legacy API temptation:** compatibility shortcuts will slow the rewrite unless the new typed surface remains the default path.

## Open Questions

- No blocking design questions remain for implementation.
- The plan assumes the current vendored `smoltcp` patch stays in place until a hardware-validated alternative exists.
- If a later pass wants persistent WiFi configuration in flash, that should be treated as a separate follow-up project after this plan lands.
