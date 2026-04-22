# ESP8266 WiFi Core API Redesign

## Status

- Date: 2026-04-22
- Scope: first subproject of the full WiFi driver redesign
- Focus: runtime/core API only
- Excludes: `smoltcp`, `embassy-net`, and other higher-level network adapters

## Purpose

This document defines the redesign of the ESP8266 WiFi driver foundation. The
goal is not to preserve the current API. The goal is to build the strongest
possible core driver for this target: correctness-first, Rust-first,
caller-owned, runtime-neutral, and explicit about every important invariant.

This foundation is intended to support the rest of the WiFi surface in later
subprojects:

- station plus network stack adapters
- access point and scan control plane
- advanced features such as promiscuous mode, ESP-NOW, and WPS

## Current Evidence And Problems

The current driver proves the platform is viable: the `tcp_server.rs` example
works and the latest documented validation in `research.md` records stable
hardware runs on the station plus `embassy-net` path.

However, the current foundation has structural issues that make further work
harder than necessary:

- the WiFi tree is large and concentrated in a few difficult modules
- the `compat/*` layer contains a large amount of shared mutable runtime state
- public lifecycle correctness depends too heavily on runtime checks
- the packet and event paths rely on many cross-module safety assumptions
- examples currently use heap-leak construction patterns to satisfy `'static`
  requirements
- the core design does not clearly separate runtime concerns, driver concerns,
  event concerns, packet concerns, and adapter concerns

The redesign in this document treats those issues as root-cause problems, not
surface cleanup targets.

## Goals

- make illegal lifecycle transitions unrepresentable where practical
- make long-lived memory caller-owned and explicit
- make the core runtime-neutral
- make packet ownership explicit and auditable
- shrink the unsafe surface into a few narrow, documented boundaries
- keep bounded memory and overflow behavior visible in Rust types
- support both blocking and async use without choosing an executor in the core
- provide clean seams for later `smoltcp` and `embassy-net` adapters

## Non-Goals

- preserving the current public API
- preserving API similarity with ESP32 crates
- including network-stack adapters in this subproject
- solving every advanced WiFi feature in the same implementation phase
- keeping example-local memory hacks or driver-global mutable singletons

## Core Design Decisions

- The design is strong typestate-first.
- The core is runtime-neutral.
- Long-lived driver memory is caller-owned.
- Network-stack integration lives outside the core.
- The implementation is a clean-slate redesign of the foundation, not a public
  façade over the current internals.

## Architecture Overview

The new core is built around a single rule: lifecycle phases that change driver
invariants become distinct Rust types.

Instead of a single controller value that can be partly initialized, partly
configured, or partly started, the driver moves through explicit phases such as:

- `WifiDriver<Uninitialized>`
- `WifiDriver<Initialized>`
- `WifiDriver<Configured<M>>`
- `WifiDriver<Started<M>>`

where `M` is a mode marker such as `StaMode`, `ApMode`, or `MixedMode`.

Operations only exist on states where they are valid. For example:

- initialization is only possible from `Uninitialized`
- configuration is only possible from `Initialized`
- start is only possible from `Configured<M>`
- station connect operations only exist for started modes that implement a
  station capability marker
- access-point station management only exists for started modes that implement
  an AP capability marker
- packet I/O handles are only obtainable from started states

This moves the core API from ad hoc runtime policing to compile-time
enforcement of the main lifecycle invariants.

## Components

The core redesign is split into a set of focused subsystems.

### `wifi::runtime`

Owns the SDK and blob execution contract:

- compat scheduler
- ISR handoff
- timer integration
- task, queue, and semaphore compatibility
- any required ROM or SDK-side execution glue

This module is mostly internal. Its public role is to supply typed runtime
services to the driver rather than exposing raw global machinery.

### `wifi::driver`

Owns lifecycle, typestate transitions, and construction of capability handles.

Responsibilities:

- borrow caller-owned resources
- bind runtime services
- initialize and deinitialize the blob
- apply validated configuration
- start and stop interfaces
- create typed capability handles
- enforce destruction ordering through borrowing and typestate

### `wifi::config`

Owns validated input data and mode configuration.

This module is pure Rust logic. It converts user inputs into validated types
before the SDK is touched.

### `wifi::event`

Owns:

- raw SDK event parsing
- driver-internal state updates triggered by events
- bounded publication of user-facing events
- blocking and async waiting over those events

This module must clearly separate internal state machine effects from public
delivery semantics.

### `wifi::packet`

Owns RX and TX memory semantics:

- SDK callback packet intake
- bounded RX staging
- typed ownership handoff
- release semantics
- packet metadata and views

This module is the main home for the redesign of packet safety.

### `wifi::interface`

Owns user-facing capability handles such as:

- `StaHandle<'d>`
- `ApHandle<'d>`
- `MixedHandle<'d>` if useful

These are borrowing views into the driver state rather than independently
allocated controller objects.

### Adapter Layer

The core does not include network adapters, but it must leave a clean seam for
future modules that bridge:

- `smoltcp`
- `embassy-net`
- any future packet-driven stack integration

That seam should live at typed packet I/O and link-event boundaries, not at
today's ad hoc runner structure.

## Typestate Model

The typestate model is intentionally strong at the lifecycle boundary and
intentionally lighter at externally driven link progress.

### Top-Level Driver States

- `WifiDriver<Uninitialized>`
  - resources exist
  - runtime is not bound
  - blob is not initialized

- `WifiDriver<Initialized>`
  - runtime is bound
  - blob is initialized
  - core callbacks and hooks are installed
  - no operational WiFi mode is committed yet

- `WifiDriver<Configured<M>>`
  - validated mode and mode-specific configuration are applied
  - `M` is one of the mode markers

- `WifiDriver<Started<M>>`
  - WiFi hardware and firmware are started for mode `M`
  - interface capability handles can be borrowed or derived

### Capability Markers

Mode capability traits express which operations are legal:

- `HasSta`
- `HasAp`

Implementations:

- `StaMode: HasSta`
- `ApMode: HasAp`
- `MixedMode: HasSta + HasAp`

This makes STA-only and AP-only APIs exist only where valid.

### Connection And Link Progress

The design does not create a top-level driver type for every transient link
phase. Instead:

- coarse invariant-changing phases use typestate
- externally driven progress uses typed runtime state and typed results

Examples:

- `Started<StaMode>::connect_begin(...)`
- `Started<StaMode>::connect_blocking(...)`
- `Started<StaMode>::connect_async(...)`
- `Started<StaMode>::connection_state() -> StaConnectionState`
- `Started<StaMode>::wait_for_connect_*() -> Result<StaSession<'d>, ConnectError>`

If a connected session handle is useful, it should represent a stronger local
guarantee such as "the station is connected at the moment this handle was
created", while still acknowledging that future disconnect events can occur.

## Object Model

The core foundation revolves around a driver value and a caller-owned resources
value:

- `WifiResources`
- `WifiDriver<S>`

Construction shape:

1. caller creates `WifiResources`
2. caller creates `WifiDriver<Uninitialized>`
3. caller initializes it into `WifiDriver<Initialized>`
4. caller applies mode configuration to obtain `WifiDriver<Configured<M>>`
5. caller starts the driver to obtain `WifiDriver<Started<M>>`
6. caller derives capability handles from the started driver

Key rule:

- handles borrow the driver and therefore cannot outlive the state that made
  them sound

This prevents stale-handle use after stop or deinit through the normal borrow
checker rather than through convention.

## Memory And Ownership

All long-lived memory required by the core driver must be provided by the
caller.

### `WifiResources`

`WifiResources` owns all long-lived storage needed by the core foundation. Its
exact fields may change during implementation, but conceptually it contains:

- event queue storage
- RX metadata rings
- TX bookkeeping
- callback and wake slots
- timer bookkeeping
- runtime bookkeeping
- fixed-capacity compat tables
- bounded scratch space required by the driver contract

The driver borrows `&'d mut WifiResources` for its entire lifetime.

### Memory Policy

- no hidden heap allocation in the core API
- no example-side memory leaks to make APIs work
- no public API that requires callers to understand private SDK ownership rules
- all bounded capacities visible through Rust configuration or resource types

### Handle Ownership

All public handles are borrowing views into driver-owned state:

- `StaHandle<'d>`
- `ApHandle<'d>`
- session handles
- event subscriptions
- packet-capable interface views

They are not heap objects and are not independent lifetime roots.

## Packet Ownership Model

Packet ownership must be modeled explicitly rather than by loosely coupled
pointer conventions.

### RX Path

The RX callback receives raw SDK values:

- packet buffer pointer
- length
- SDK ownership token or equivalent release value

The callback is allowed to do only bounded, minimal work:

- convert raw callback inputs into a compact typed record
- push that record into a bounded staging structure
- signal a waiter or wake source

Task-level code later converts that staged record into a safe packet object or
safe frame view.

### Release Semantics

Packet release must be encoded in Rust ownership:

- exactly one owner is responsible for the final SDK handoff
- dropping or explicitly releasing the packet object performs the correct
  cleanup exactly once
- double-release and forgotten release are prevented by construction

### Overflow Behavior

If bounded RX staging is full:

- the drop is explicit
- the drop is counted
- the drop is diagnosable
- internal state remains sound

The same applies to any bounded event queue or TX staging facility.

## Runtime Neutrality

The core must support blocking and async use without choosing an executor.

### Required API Styles

The core should expose:

- immediate state queries
- bounded blocking waits
- async waits implemented as `Future`

Typical operation pairs:

- `start()` and `start_async()`
- `stop()` and `stop_async()`
- `connect_*()` variants on station-capable started drivers
- blocking and async event waits

Internally, these can be implemented as nonblocking transitions plus wait
primitives, but the public API should remain direct and ergonomic.

### No Executor Assumption

The core:

- must not depend on Embassy-specific channels or timers as architectural
  primitives
- may expose its own wake-registration and wait infrastructure
- may later be wrapped by Embassy adapters with little or no extra logic

## Event Model

Events have three distinct phases:

1. raw SDK event reception
2. internal driver state reconciliation
3. user-facing bounded event publication

These phases must not be conflated.

### Event Delivery

The event subsystem should support:

- `try_next_event()`
- `wait_for_event_blocking(...)`
- `wait_for_event_async(...)`
- bounded subscription handles
- waits on one event or a set of events

### Event Semantics

- internal state updates happen before user-facing delivery is considered
- slow consumers do not corrupt internal state
- bounded event overflow is explicit and diagnosable
- event waits observe a well-defined ordering relative to internal state updates

## Configuration And Validation

The driver should parse and validate at the edge, then operate on validated
types internally.

Candidate validated types include:

- `Ssid`
- `Passphrase`
- `Channel`
- `Bssid`
- `CountryCode`
- `ScanConfig`
- `PowerSavePolicy`
- `ProtocolSet`

Validation rules should move into constructors or builders for these types, so
that invalid values do not survive into the driver state machine.

Mode configuration should also be validated as a whole:

- `ClientConfig`
- `AccessPointConfig`
- `MixedConfig`

That includes cross-field validity rules such as:

- AP password and auth consistency
- STA credential and auth consistency
- channel and secondary-channel rules
- mixed-mode compatibility constraints

## Error Model

The redesign should use narrower error domains than the current single broad
controller error surface.

Target layers:

- `ConfigError`
- `InitError`
- `StartError<M>`
- `StopError<M>`
- `ConnectError`
- `DisconnectError`
- `ScanError`
- `DriverError`

Raw SDK errors remain available as underlying detail where useful, but they do
not define the public meaning of the API.

Disconnects and connection failures should preserve typed reasons, including:

- authentication timeout or failure
- no AP found
- beacon timeout
- association failure
- handshake timeout
- unknown or unmapped SDK reason codes

## Unsafe Boundaries

Unsafe code should be concentrated into a few audited modules only.

### `wifi::ffi`

Owns:

- direct SDK, blob, and ROM calls
- raw ABI conversions
- raw C layout handling

### `wifi::runtime::isr`

Owns:

- interrupt entry points
- callback trampolines
- context-switch and wakeup boundaries that cannot be expressed safely

### `wifi::packet::raw`

Owns:

- raw packet pointers
- ownership token handling
- final release operations

### Additional Internal Unsafe Modules

If required by the implementation, runtime task and timer internals may contain
unsafe code, but they must remain private and narrowly scoped.

### Unsafe Documentation Rule

Every unsafe module and unsafe function must document:

- required preconditions
- memory ownership before the operation
- memory ownership after the operation
- allowed calling contexts
- the exact reason the operation is sound

## Testing Strategy

The redesign should maximize host-testable logic and minimize hardware-only
correctness.

### Host-Side Unit Tests

- validated config types
- typestate transition logic that does not require hardware
- event broker behavior
- bounded queue and ring behavior
- packet ownership and release state machines using fake raw inputs
- error mapping from raw SDK values into Rust types

### Property Tests

Use property tests where ordering and bounded-state behavior matter:

- ring push and pop ordering
- no double-release invariants
- bounded overflow behavior
- event publication order
- state transition sequences never reaching illegal states

### Hardware-Gated Integration Tests

- init, start, and stop sequencing
- STA connect and disconnect flows
- AP start, stop, and station attach or detach flows
- scan behavior
- packet RX and TX handoff correctness
- long-running stability or soak tests

### Example Policy

Examples should demonstrate the designed API. They must not hide missing design
pieces behind heap leaks, hidden globals, or special-case setup logic.

## Implementation Constraints For Later Planning

The later implementation plan should assume these constraints:

- the core API must not require `smoltcp` or `embassy-net`
- the core must compile and be usable in blocking code
- async support in core must remain executor-neutral
- examples must stop depending on permanent heap leaks for task and storage
- existing monolithic internals may be split if that improves invariants and
  testability
- diagnostics needed only for bring-up should not remain in the core shipping
  path unless they are cheap, bounded, and structurally useful

## Acceptance Criteria

The core API redesign is successful when all of the following are true:

- invalid lifecycle order is rejected at compile time for the main state
  transitions
- mode-specific operations are available only on compatible mode states
- the core has no hidden long-lived heap allocation requirement
- packet release semantics are encoded in ownership rather than convention
- event and packet queues are bounded and their overflow behavior is explicit
- unsafe code lives inside a small number of documented internal modules
- blocking and async waiting both work without centering the architecture on a
  particular executor
- higher-level stack adapters can be implemented as separate layers over typed
  packet and event primitives

## Future Subprojects

This document intentionally stops at the foundation. The next design and plan
documents should build on it in this order:

1. station plus network-stack adapters
2. access point plus scan control plane
3. advanced features such as promiscuous mode, ESP-NOW, and WPS

## References

- Repository context:
  - `research.md`
  - `src/wifi/controller.rs`
  - `src/wifi/device.rs`
  - `src/wifi/event.rs`
  - `src/wifi/compat/task.rs`
  - `examples/tcp_server.rs`
- Official esp-rs WiFi references consulted during design:
  - `esp-wifi::wifi::new`
    - https://docs.espressif.com/projects/rust/esp-wifi/0.14.0/esp32/esp_wifi/wifi/fn.new.html
  - `esp-wifi::wifi::Configuration`
    - https://docs.espressif.com/projects/rust/esp-wifi/0.14.0/esp32/esp_wifi/wifi/enum.Configuration.html
  - `esp-radio::wifi::WifiController`
    - https://docs.espressif.com/projects/rust/esp-radio/0.16.0/esp32c2/esp_radio/wifi/struct.WifiController.html
  - `esp-wifi` crate overview and tunables
    - https://docs.espressif.com/projects/rust/esp-wifi/0.14.0/esp32/esp_wifi/index.html
