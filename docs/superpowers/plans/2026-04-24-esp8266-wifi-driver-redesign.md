# ESP8266 WiFi Driver Redesign Plan

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

Rewrite the ESP8266 WiFi driver around the best Rust-side design we can support on this hardware.

The rewrite has no backwards-compatibility requirement. Existing public APIs may be removed, renamed, or replaced if doing so improves correctness, robustness, performance, or ownership clarity.

The final driver should support and verify:

- station mode: scan, connect, disconnect, DHCP, static IPv4, TCP client, TCP server;
- soft-AP mode: start, station list, deauth, AP RX/TX, Rust-owned DHCP;
- AP+STA mode: upstream station plus downstream AP client coexistence;
- ESP-NOW: broadcast, unicast, encrypted peer, RX, TX status, bad-key behavior;
- promiscuous mode: management/data/control filtering and frame capture;
- WPS and SmartConfig sessions;
- reproducible host and hardware validation through `xtask`.

## Context and Constraints

- Current baseline commit: `b7c22d2 refactor`.
- DUT: ESP8266EX on `/dev/ttyUSB0`, MAC `f4:cf:a2:d9:ad:6b`.
- Companion: ESP32-PICO-D4 on `/dev/ttyACM0`, MAC `4c:c3:82:82:32:80`.
- Repo-local ESP32 companion tooling is already present under `.tools/esp32/` and `companion/esp32-pico-d4/`.
- `research.md` is the runtime evidence log and should be updated after meaningful validation milestones.
- Current `xtask` host tests pass.
- Current `wifi-core` host test command fails because `src/wifi/core/packet.rs` compares a `Result<OwnedRxLease, _>` in a test while `OwnedRxLease` lacks `Debug`/`PartialEq`.
- The validated `tcp_server.rs` path depends on:
  - typestated `wifi_core`;
  - caller-owned `WifiResources`;
  - split `embassy-net-driver-channel` runner;
  - vendored `smoltcp` targeted local-subnet neighbor learning.
- Network stack and driver runtime must fit the ESP8266 DRAM budget.
- C is acceptable only for unavoidable SDK/blob ABI surfaces. If a component can be correct in Rust, it should be in Rust.

## Recommended Approach

Use a full Rust-first driver redesign with a narrow target backend.

The public driver becomes `wifi_core` plus explicitly typed adapters. The existing `wifi` modules should either become target-integration internals or thin re-exports. The raw SDK stays hidden behind a small backend that translates typed Rust operations into SDK calls and callback ingress.

Why this is recommended:

- It maximizes Rust ownership and validation instead of wrapping the SDK model directly.
- It keeps packet queues, event state, feature sessions, DHCP, and network adapters caller-owned and testable on the host.
- It lets the ESP8266 target backend remain small enough to audit for unsafe and global-state invariants.
- It supports AP+STA and special modes through explicit capabilities instead of stringly runtime mode checks.
- It gives the hardware matrix one coherent API surface to validate.

## Alternatives Considered

### 1. Full Rust-First Rewrite

- Good: strongest ownership model and cleanest public API.
- Good: removes legacy API constraints and duplicated state.
- Good: makes host tests meaningful for most logic.
- Cost: largest implementation pass and highest chance of temporary compile breakage.
- Decision: selected.

### 2. Incremental Cleanup of Current `wifi_core`

- Good: lower risk per change.
- Good: preserves the validated `tcp_server` path throughout.
- Bad: leaves legacy controller concepts and duplicated APIs longer.
- Bad: harder to reach the “best possible driver” target.
- Decision: rejected for this pass.

### 3. SDK-Style Wrapper With Rust Adapters

- Good: fastest path to feature coverage.
- Bad: keeps lifecycle, memory, and mode correctness mostly implicit.
- Bad: conflicts with the Rust-sided goal.
- Decision: rejected.

## Architecture

### Public API

The crate should expose one coherent WiFi API:

- `esp8266_hal::wifi` for the public driver surface;
- or `esp8266_hal::wifi_core` if keeping the current module name is cleaner during the first rewrite.

The final public surface should include:

- `WifiDriver<Uninitialized, B>` lifecycle root;
- `Initialized`, `Configured<M>`, and `Started<M>` typestates;
- `StaMode`, `ApMode`, and `MixedMode` capability markers;
- `StaHandle`, `ApHandle`, `StaLink`, `ApLink`, and a mixed dual-link owner;
- RAII feature sessions for ESP-NOW, promiscuous, WPS, and SmartConfig;
- validated config types at all public boundaries.

Legacy controller-style APIs should not remain public unless they become aliases over the new API with no extra state.

### Target Backend

`Esp8266TargetBackend` owns only the target-specific operations:

- SDK initialization/deinitialization;
- mode/config/start/stop/connect/disconnect calls;
- scan mapping;
- internal TX calls;
- callback registration and de-registration;
- unavoidable active-resource pointer for SDK callbacks;
- feature-specific SDK calls for ESP-NOW, promiscuous, WPS, and SmartConfig.

Unsafe code should live in tiny functions with documented invariants:

- why a callback pointer is valid;
- who owns RX buffers and when they are released;
- why critical sections protect shared state;
- which SDK calls may be invoked from task context versus callback context.

### Runtime State

All Rust-owned state lives in caller-owned resources:

- event broker;
- STA RX queue;
- AP RX queue;
- AP copied RX buffer pool;
- ESP-NOW peer table;
- ESP-NOW RX queue;
- ESP-NOW TX-status queue;
- promiscuous RX queue;
- wakers;
- diagnostics.

The backend may hold a pointer to `WifiResources`, but it must not own the state.

### Data Plane

The STA and AP data paths should share one trait-like internal shape:

- register link/RX wakers;
- report link state;
- report MAC address;
- refresh or attach RX;
- dequeue RX;
- transmit TX;
- report diagnostics.

Adapters should be thin:

- direct `smoltcp::phy::Device` over an owned `StaLink` or `ApLink`;
- `embassy-net-driver-channel` bridge over the same data-link abstraction;
- split state/RX/TX runner retained for `embassy-net`.

AP DHCP remains Rust-owned. Any C lwIP shim should be removed unless it is proven required for AP packet ingress or SDK station association; if required, it must be documented as ABI glue only.

### Feature Sessions

Feature sessions should be explicit, RAII-owned, and mode-constrained:

- ESP-NOW available in station-capable and mixed modes, with peer interface typed as station or AP.
- Promiscuous session available after WiFi start, with bounded capture storage.
- WPS and SmartConfig available only on station-capable modes.
- Session `Drop` should best-effort stop/deinit the feature.
- Fallible `stop()` remains available when callers need error reporting.

### Configuration

All configuration should parse into validated Rust types before touching the SDK:

- SSID length and byte handling;
- passphrase length and credential kind;
- AP channel, max clients, beacon interval;
- scan timing/channel/BSSID options;
- static IPv4 subnet constraints;
- ESP-NOW PMK/LMK and payload limits;
- WPS PIN format;
- SmartConfig timeout/key format;
- init buffer counts with documented DRAM trade-offs.

Mapping to SDK structs should be infallible after validation. Avoid `unwrap()` in target mappings.

### Diagnostics

Diagnostics should be structured and optional:

- counters for event overflows and RX drops;
- callback attach/register counters;
- RX/TX runner counters;
- heap/stack snapshots only where they proved useful;
- scenario artifact manifests in `xtask`;
- concise runtime markers in examples.

Debug-only globals should not be part of the normal hot path unless guarded behind features or narrow APIs.

## File Map

Likely files to modify heavily:

- `src/wifi/core/mod.rs`
- `src/wifi/core/backend.rs`
- `src/wifi/core/config.rs`
- `src/wifi/core/driver.rs`
- `src/wifi/core/error.rs`
- `src/wifi/core/event.rs`
- `src/wifi/core/feature.rs`
- `src/wifi/core/interface.rs`
- `src/wifi/core/packet.rs`
- `src/wifi/core/resources.rs`
- `src/wifi/core/state.rs`
- `src/wifi/core/target.rs`
- `src/wifi/device.rs`
- `src/wifi/embassy_net.rs`
- `src/wifi/mod.rs`
- `src/wifi/controller.rs`
- `src/wifi/ap.rs`
- `src/wifi/sta.rs`
- `src/wifi/scan.rs`
- `src/wifi/esp_now.rs`
- `src/wifi/promiscuous.rs`
- `src/wifi/wps.rs`
- `src/wifi/smartconfig.rs`
- `src/wifi/ip.rs`
- `esp8266-wifi-sys/build.rs`
- `esp8266-wifi-sys/src/lib.rs`
- `esp8266-wifi-sys/ap_netif_bridge.c`
- `examples/tcp_server.rs`
- `examples/tcp_client.rs`
- `examples/sta_static_ip.rs`
- `examples/wifi_ap.rs`
- `examples/wifi_ap_sta.rs`
- `examples/wifi_esp_now_tx.rs`
- `examples/wifi_esp_now_rx.rs`
- `examples/wifi_promisc.rs`
- `examples/wifi_wps.rs`
- `examples/wifi_smartconfig.rs`
- `xtask/src/wifi_hw.rs`
- `xtask/src/esp32_companion.rs`
- `companion/esp32-pico-d4/main/main.c`
- `tests/wifi_core_*.rs`
- `tests/wifi_ip_config.rs`
- `xtask/tests/*.rs`
- `research.md`

Likely files to create:

- `src/wifi/core/link.rs`
- `src/wifi/core/session.rs`
- `src/wifi/core/capability.rs`
- `src/wifi/core/target_map.rs`
- `src/wifi/core/target_callback.rs`
- `src/wifi/core/net.rs`
- `tests/wifi_core_mixed.rs`
- `tests/wifi_core_packets.rs`
- `tests/wifi_core_target_mapping.rs`

Files that should remain mostly raw ABI plumbing:

- `esp8266-wifi-sys/src/lib.rs`
- generated bindings under `OUT_DIR`
- minimal C shims only where Rust cannot satisfy SDK ABI requirements.

## Implementation Tasks

### Task 1: Establish a Passing Baseline

**Why:** The rewrite needs a green host baseline before large API movement.

**Files:**
- Modify: `src/wifi/core/packet.rs`
- Modify: `xtask/src/wifi_hw.rs`
- Modify: `research.md`

**Steps:**
- [ ] Fix the `OwnedRxLease` test failure without adding unnecessary public traits to internal types.
- [ ] Remove or justify the `unused_mut` warnings in `xtask/src/wifi_hw.rs`.
- [ ] Record the clean baseline commands and results in `research.md`.

**Verification:**
- Run: `cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --lib --tests -q`
- Expect: all host `wifi-core` tests pass.
- Run: `cargo test -p xtask --target x86_64-unknown-linux-gnu -q`
- Expect: all `xtask` tests pass without new warnings from touched code.

### Task 2: Freeze the New Public Shape

**Why:** The full rewrite needs a single API center before internals move.

**Files:**
- Modify: `src/wifi/mod.rs`
- Modify: `src/wifi/core/mod.rs`
- Modify: `src/lib.rs`
- Modify: `tests/wifi_core_smoke.rs`
- Modify: `tests/wifi_core_typestate.rs`

**Steps:**
- [ ] Decide whether the final public path is `wifi` or `wifi_core`.
- [ ] Re-export only the new driver types from that public path.
- [ ] Make old controller modules private or mark them target-internal.
- [ ] Add compile-time tests for allowed mode capabilities.
- [ ] Ensure examples import only the new public API.

**Verification:**
- Run: `cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --lib --tests -q`
- Expect: public API smoke and typestate tests pass.
- Run: `source .envrc && cargo check --target xtensa-esp8266-none-elf -Zbuild-std=core --no-default-features --features wifi-core`
- Expect: `wifi-core` compiles without target backend.

### Task 3: Redesign Configuration Types

**Why:** SDK calls should receive already-validated Rust values.

**Files:**
- Modify: `src/wifi/core/config.rs`
- Modify: `src/wifi/core/error.rs`
- Modify: `tests/wifi_core_config.rs`
- Modify: `tests/wifi_ip_config.rs`

**Steps:**
- [ ] Replace ad-hoc fields with validated newtypes for every SDK-constrained value.
- [ ] Add explicit config builders for STA, AP, mixed mode, scan, init, ESP-NOW, WPS, and SmartConfig.
- [ ] Make config-to-target mapping infallible after validation.
- [ ] Avoid UTF-8 assumptions where the SDK accepts bytes.
- [ ] Add edge-case tests for exact length limits and invalid combinations.

**Verification:**
- Run: `cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --test wifi_core_config --test wifi_ip_config -q`
- Expect: config boundary tests pass.

### Task 4: Split Core State From Target Backend

**Why:** Most logic should be host-testable and independent of SDK globals.

**Files:**
- Modify: `src/wifi/core/backend.rs`
- Modify: `src/wifi/core/driver.rs`
- Modify: `src/wifi/core/interface.rs`
- Create: `src/wifi/core/capability.rs`
- Create: `src/wifi/core/session.rs`
- Modify: `tests/wifi_core_typestate.rs`
- Modify: `tests/wifi_core_smoke.rs`

**Steps:**
- [ ] Define narrow backend traits for lifecycle, station, AP, packet TX, scan, and special features.
- [ ] Keep typestate transitions consuming `self` for lifecycle state changes.
- [ ] Keep temporary handles borrowing the driver mutably.
- [ ] Keep data links owning the backend or a typed backend partition.
- [ ] Add tests that disallowed operations are unrepresentable or absent from the API.

**Verification:**
- Run: `cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --lib --tests -q`
- Expect: all host core tests pass.

### Task 5: Rebuild Event and Packet Resources

**Why:** RX, events, and wakers are the correctness heart of the driver.

**Files:**
- Modify: `src/wifi/core/event.rs`
- Modify: `src/wifi/core/packet.rs`
- Modify: `src/wifi/core/resources.rs`
- Modify: `src/wifi/core/waker.rs`
- Modify: `tests/wifi_core_link.rs`
- Modify: `tests/wifi_core_resources.rs`
- Create: `tests/wifi_core_packets.rs`

**Steps:**
- [ ] Make event overflow behavior explicit and tested for slow cursors.
- [ ] Ensure zero-capacity queues are valid and cheap.
- [ ] Separate STA and AP RX queues and diagnostics cleanly.
- [ ] Preserve release-token correctness for SDK-owned RX buffers.
- [ ] Keep AP copied RX slots bounded and configurable.
- [ ] Add property tests for ring order, drop accounting, and release behavior.

**Verification:**
- Run: `cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --test wifi_core_link --test wifi_core_resources --test wifi_core_packets -q`
- Expect: packet/resource tests pass.

### Task 6: Rewrite the ESP8266 Target Backend

**Why:** Unsafe SDK interaction should be isolated, audited, and minimal.

**Files:**
- Modify: `src/wifi/core/target.rs`
- Create: `src/wifi/core/target_map.rs`
- Create: `src/wifi/core/target_callback.rs`
- Modify: `src/wifi/controller.rs`
- Modify: `src/wifi/compat/*` only where required
- Modify: `esp8266-wifi-sys/build.rs`
- Modify: `esp8266-wifi-sys/src/lib.rs`

**Steps:**
- [ ] Move SDK struct mapping into a dedicated module.
- [ ] Move callback trampolines into a dedicated module.
- [ ] Document the active-resource pointer invariant.
- [ ] Make attach/detach idempotent for STA and AP RX callbacks.
- [ ] Make stop/drop release callbacks and SDK feature state in a deterministic order.
- [ ] Remove target globals that duplicate `WifiResources` state unless they are only compatibility shims.
- [ ] Replace `unwrap()` in target mappings with typed validation or error propagation.

**Verification:**
- Run: `source .envrc && cargo check --target xtensa-esp8266-none-elf -Zbuild-std=core --no-default-features --features interrupt,rt,wifi-core,wifi,wifi-sta`
- Expect: target backend compiles for STA.
- Run: `source .envrc && cargo check --target xtensa-esp8266-none-elf -Zbuild-std=core --no-default-features --features interrupt,rt,wifi-ap`
- Expect: target backend compiles for AP.

### Task 7: Rebuild Data-Link Adapters

**Why:** `tcp_server.rs` proved the path works; the rewrite must preserve and generalize it.

**Files:**
- Modify: `src/wifi/device.rs`
- Modify: `src/wifi/embassy_net.rs`
- Create: `src/wifi/core/link.rs`
- Modify: `tests/wifi_core_link.rs`

**Steps:**
- [ ] Define one internal Rust data-link abstraction for STA and AP.
- [ ] Implement direct `smoltcp` device over any data link.
- [ ] Implement `embassy-net` bridge over any data link.
- [ ] Keep split state/RX/TX runner behavior.
- [ ] Preserve TX backpressure handling and RX-buffer waiting without deadlock.
- [ ] Add host tests with fake links for RX, TX, waker, and backpressure paths.

**Verification:**
- Run: `cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --test wifi_core_link -q`
- Expect: fake-link adapter behavior covered by host tests passes.
- Run: `source .envrc && cargo check --example tcp_server --target xtensa-esp8266-none-elf -Zbuild-std=core --no-default-features --features interrupt,rt,wifi-embassy-net,wifi-sta`
- Expect: `tcp_server` compiles.

### Task 8: Rewrite Station Examples and Validation

**Why:** STA is the baseline networking path and must stay robust.

**Files:**
- Modify: `examples/wifi_core_connect.rs`
- Modify: `examples/raw_scan.rs`
- Modify: `examples/wifi_core_dhcp_probe.rs`
- Modify: `examples/sta_static_ip.rs`
- Modify: `examples/tcp_client.rs`
- Modify: `examples/tcp_server.rs`
- Modify: `xtask/src/wifi_hw.rs`
- Modify: `research.md`

**Steps:**
- [ ] Convert all STA examples to the final public API.
- [ ] Keep `connect_blocking()` as WiFi-link-only; DHCP stays in network stack logic.
- [ ] Preserve the validated `tcp_server` buffer and runner behavior unless hardware evidence says otherwise.
- [ ] Add scenario markers that distinguish WiFi, DHCP, TCP connect, TCP payload, and close failures.
- [ ] Run the STA hardware matrix before changing AP or special modes further.

**Verification:**
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw run sta_connect`
- Expect: DUT logs station connected.
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw run sta_scan`
- Expect: DUT logs scan success.
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw run sta_dhcp`
- Expect: DUT logs DHCP config-up.
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw run tcp_server`
- Expect: host connects and receives echo on first attempt.
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw run tcp_client`
- Expect: DUT connects to host TCP server and completes response.

### Task 9: Implement Robust AP and Mixed Mode

**Why:** AP and AP+STA require separate RX/TX ownership and downstream client proof.

**Files:**
- Modify: `src/wifi/core/driver.rs`
- Modify: `src/wifi/core/interface.rs`
- Modify: `src/wifi/core/resources.rs`
- Modify: `src/wifi/device.rs`
- Modify: `src/wifi/embassy_net.rs`
- Modify: `examples/wifi_ap.rs`
- Modify: `examples/wifi_ap_sta.rs`
- Modify: `tests/wifi_core_ap.rs`
- Create: `tests/wifi_core_mixed.rs`

**Steps:**
- [ ] Add a typed mixed dual-link owner if concurrent STA and AP data links are needed.
- [ ] Ensure AP station-list and deauth operate through `ApHandle`.
- [ ] Keep AP DHCP implemented in Rust.
- [ ] Remove C lwIP AP netif glue if AP RX/TX works without it.
- [ ] If C AP glue is required, reduce it to a documented ABI shim and keep DHCP/data handling in Rust.
- [ ] Make AP+STA examples prove both upstream DHCP and downstream association/DHCP.

**Verification:**
- Run: `cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --test wifi_core_ap --test wifi_core_mixed -q`
- Expect: AP and mixed host tests pass.
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw run ap_start`
- Expect: AP starts.
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw run ap_station_list_companion`
- Expect: ESP32 joins, station list is non-empty, and deauth is observed.
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw run ap_dhcp_companion`
- Expect: ESP32 receives a `192.168.4.x` lease from Rust DHCP.
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw run ap_sta_downstream_companion`
- Expect: upstream STA DHCP and downstream AP client behavior coexist.

### Task 10: Rewrite ESP-NOW

**Why:** ESP-NOW needs typed peer management, bounded callbacks, and two-radio proof.

**Files:**
- Modify: `src/wifi/core/feature.rs`
- Modify: `src/wifi/core/interface.rs`
- Modify: `src/wifi/core/resources.rs`
- Modify: `src/wifi/core/target.rs`
- Modify: `examples/wifi_esp_now_tx.rs`
- Modify: `examples/wifi_esp_now_rx.rs`
- Modify: `tests/wifi_core_esp_now.rs`
- Modify: `xtask/src/wifi_hw.rs`
- Modify: `companion/esp32-pico-d4/main/main.c`

**Steps:**
- [ ] Validate peer addresses, interfaces, channels, PMK/LMK, and payload length before SDK calls.
- [ ] Track peer table state in Rust and keep SDK errors authoritative.
- [ ] Keep RX and TX-status queues bounded and drop-counted.
- [ ] Add typed station/AP peer entry points for mixed mode.
- [ ] Make examples support DUT-as-RX and DUT-as-TX roles.
- [ ] Use the ESP32 companion for unicast, encrypted, and bad-key validation.

**Verification:**
- Run: `cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --test wifi_core_esp_now -q`
- Expect: ESP-NOW host tests pass.
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw run esp_now_broadcast`
- Expect: broadcast TX status succeeds.
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw run esp_now_unicast_companion`
- Expect: ESP32-to-DUT or DUT-to-ESP32 unicast payload is observed.
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw run esp_now_encrypted_companion`
- Expect: encrypted payload is observed with matching LMK.
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw run esp_now_bad_key_companion`
- Expect: bad-key payload is not accepted.

### Task 11: Rewrite Promiscuous Capture

**Why:** Promiscuous mode must be bounded, filterable, and safe under callback load.

**Files:**
- Modify: `src/wifi/core/feature.rs`
- Modify: `src/wifi/core/interface.rs`
- Modify: `src/wifi/core/resources.rs`
- Modify: `src/wifi/core/target.rs`
- Modify: `examples/wifi_promisc.rs`
- Modify: `tests/wifi_core_promisc.rs`
- Modify: `xtask/src/wifi_hw.rs`

**Steps:**
- [ ] Validate packet and control-subtype filters in Rust.
- [ ] Capture only a bounded prefix and metadata per frame.
- [ ] Count drops when the capture queue is full.
- [ ] Ensure `Drop` disables promiscuous callbacks.
- [ ] Use companion-generated traffic for data-frame validation.

**Verification:**
- Run: `cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --test wifi_core_promisc -q`
- Expect: promiscuous host tests pass.
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw run promisc_capture`
- Expect: at least one frame is captured.
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw run promisc_filters`
- Expect: management filter scenario succeeds.
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw run promisc_data_companion`
- Expect: data frame capture succeeds with ESP32 traffic.

### Task 12: Rewrite WPS and SmartConfig Sessions

**Why:** Provisioning should be session-owned and event-driven.

**Files:**
- Modify: `src/wifi/core/feature.rs`
- Modify: `src/wifi/core/interface.rs`
- Modify: `src/wifi/core/event.rs`
- Modify: `src/wifi/core/target.rs`
- Modify: `examples/wifi_wps.rs`
- Modify: `examples/wifi_smartconfig.rs`
- Modify: `tests/wifi_core_provisioning.rs`
- Modify: `xtask/src/wifi_hw.rs`

**Steps:**
- [ ] Keep WPS and SmartConfig available only in station-capable states.
- [ ] Make event consumption cursor-owned and independent of other sessions.
- [ ] Convert SmartConfig credentials into `ClientConfig` without lossy assumptions.
- [ ] Ensure stop/deinit happens on explicit stop and on drop.
- [ ] Keep hardware scenarios manual unless reliable external WPS/SmartConfig stimulus is available.

**Verification:**
- Run: `cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --test wifi_core_provisioning -q`
- Expect: provisioning host tests pass.
- Run: `source .envrc && cargo check --example wifi_wps --target xtensa-esp8266-none-elf -Zbuild-std=core --no-default-features --features interrupt,rt,wifi-wps,wifi-sta`
- Expect: WPS example compiles.
- Run: `source .envrc && cargo check --example wifi_smartconfig --target xtensa-esp8266-none-elf -Zbuild-std=core --no-default-features --features interrupt,rt,wifi-smartconfig,wifi-sta`
- Expect: SmartConfig example compiles.

### Task 13: Promote Companion Scenarios

**Why:** The ESP32 companion is required to prove AP and ESP-NOW behavior repeatably.

**Files:**
- Modify: `xtask/src/wifi_hw.rs`
- Modify: `xtask/src/esp32_companion.rs`
- Modify: `xtask/tests/wifi_hw_cli.rs`
- Modify: `xtask/tests/esp32_companion_cli.rs`
- Modify: `companion/esp32-pico-d4/main/main.c`
- Modify: `research.md`

**Steps:**
- [ ] Keep ESP-IDF tooling repo-local.
- [ ] Ensure `doctor`, `build`, `flash`, and `command` are deterministic.
- [ ] Mark companion scenarios as implemented once they are automated and validated.
- [ ] Record DUT and companion MACs, ports, firmware version, and logs in every run artifact.
- [ ] Add retry and timeout policies that distinguish RF flakiness from driver failures.

**Verification:**
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw companion doctor`
- Expect: both boards are identified.
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw companion build`
- Expect: companion firmware builds under `target/esp32-companion/`.
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw companion flash --serial-port /dev/ttyACM0`
- Expect: ESP32 flashes and emits `READY`.
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw companion command PING`
- Expect: response includes `OK PONG`.

### Task 14: Remove Legacy API and Duplicated State

**Why:** No backwards compatibility is required, so stale surfaces should not remain.

**Files:**
- Modify or delete: `src/wifi/controller.rs`
- Modify or delete: `src/wifi/ap.rs`
- Modify or delete: `src/wifi/sta.rs`
- Modify or delete: `src/wifi/scan.rs`
- Modify or delete: `src/wifi/esp_now.rs`
- Modify or delete: `src/wifi/promiscuous.rs`
- Modify or delete: `src/wifi/wps.rs`
- Modify or delete: `src/wifi/smartconfig.rs`
- Modify: `src/wifi/mod.rs`
- Modify: examples and tests that import legacy APIs

**Steps:**
- [ ] Delete legacy public wrappers that duplicate the new API.
- [ ] Keep only internal target support needed by `Esp8266TargetBackend`.
- [ ] Remove duplicated station/AP atomics if `WifiResources` can be the single truth.
- [ ] Update docs and examples to use only the final API.
- [ ] Keep compatibility shims private and named as shims when unavoidable.

**Verification:**
- Run: `rg "WifiController|StaConfig|ApConfig|scan_blocking|get_connected_stations" examples tests src/wifi`
- Expect: matches are either gone or private target-internal implementation details.
- Run: `cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --lib --tests -q`
- Expect: host tests pass.

### Task 15: Final Matrix and Evidence Update

**Why:** The rewrite is not complete until software and hardware evidence agree.

**Files:**
- Modify: `research.md`
- Modify: `xtask/src/wifi_hw.rs` if matrix membership changes
- Modify: docs or examples as needed

**Steps:**
- [ ] Run host tests for all supported feature sets.
- [ ] Run Xtensa compile checks for representative examples.
- [ ] Run hardware matrix for implemented scenarios.
- [ ] Record failures with exact artifact paths and failure class.
- [ ] Record accepted fixes and rejected experiments in `research.md`.

**Verification:**
- Run: `cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --lib --tests -q`
- Expect: host core tests pass.
- Run: `cargo test -p xtask --target x86_64-unknown-linux-gnu -q`
- Expect: `xtask` tests pass.
- Run: `source .envrc && cargo check --example tcp_server --target xtensa-esp8266-none-elf -Zbuild-std=core --no-default-features --features interrupt,rt,wifi-embassy-net,wifi-sta`
- Expect: `tcp_server` compiles.
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw matrix`
- Expect: all implemented scenarios pass or failures are triaged and recorded.

## Verification

Use layered verification throughout the rewrite.

### Host Tests

Primary command:

```sh
cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --lib --tests -q
```

`xtask` command:

```sh
cargo test -p xtask --target x86_64-unknown-linux-gnu -q
```

### Xtensa Compile Checks

Representative commands:

```sh
source .envrc && cargo check --target xtensa-esp8266-none-elf -Zbuild-std=core --no-default-features --features wifi-core
source .envrc && cargo check --example tcp_server --target xtensa-esp8266-none-elf -Zbuild-std=core --no-default-features --features interrupt,rt,wifi-embassy-net,wifi-sta
source .envrc && cargo check --example wifi_ap --target xtensa-esp8266-none-elf -Zbuild-std=core --no-default-features --features interrupt,rt,wifi-ap
source .envrc && cargo check --example wifi_esp_now_rx --target xtensa-esp8266-none-elf -Zbuild-std=core --no-default-features --features interrupt,rt,wifi-sta,wifi-esp-now
```

### Hardware Runs

Core STA:

```sh
cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw run sta_connect
cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw run sta_dhcp
cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw run tcp_server
```

Companion:

```sh
cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw companion doctor
cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw run ap_dhcp_companion
cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw run esp_now_unicast_companion
cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw run promisc_data_companion
```

Full matrix:

```sh
cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw matrix
```

## Risks and Edge Cases

- SDK callbacks require global ingress. Mitigation: keep a single documented active-resource pointer and release it deterministically.
- RX buffer ownership is easy to break. Mitigation: release-token tests, AP copy-pool tests, and hardware RX-drop counters.
- AP RX may depend on current C lwIP shim behavior. Mitigation: remove it only after AP companion DHCP proves pure Rust works, otherwise shrink and document it.
- AP+STA may require concurrent STA and AP links. Mitigation: add a typed dual-link owner rather than using shared mutable backend aliases.
- ESP8266 DRAM is tight. Mitigation: keep defaults minimal, const-generic queue sizing explicit, and compile-time size assertions where useful.
- Event queues can overflow with slow subscribers. Mitigation: make overflow visible and avoid hiding stale state behind typed lies.
- `embassy-net` deadlocks can return if RX backpressure blocks TX. Mitigation: preserve split runner and concurrent RX-buffer/TX-buffer waiting.
- RF/hardware tests are nondeterministic. Mitigation: run matrix retries, capture artifacts, and classify failures in `research.md`.
- WPS and SmartConfig need external stimulus. Mitigation: keep session logic host-tested and hardware scenarios manual unless reliable automation exists.

## Open Questions

- Should the final public module path be `esp8266_hal::wifi` or `esp8266_hal::wifi_core`?
- Can AP RX/TX be made fully Rust-owned without `esp8266-wifi-sys/ap_netif_bridge.c`?
- Does mixed mode need simultaneous `StaLink` and `ApLink`, or is a staged handoff enough for the target use cases?
- Which companion scenarios should be promoted from manual to implemented in the first hardware pass?
