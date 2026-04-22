# WiFi Core API Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task in the current tree. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a new strong-typestate, runtime-neutral, caller-owned WiFi core API under `src/wifi/core/` that is host-testable without the ESP8266 SDK, then add an ESP8266 target backend plus a minimal blocking station-connect example that proves the foundation composes without `smoltcp` or `embassy-net`.

**Architecture:** Keep the new foundation separate from the legacy `wifi` public surface during bring-up by exposing it as `esp8266_hal::wifi_core` behind a new `wifi-core` feature. Make the core pure Rust first: validated config, typestate transitions, resources, event broker, packet ring, and a backend trait. Only after that, add the ESP8266 target backend under the existing `wifi` feature, with one explicit audited callback-registration slot that points into caller-owned resources. Avoid checked-in bindgen snapshots, avoid `static mut` event state, and make the legacy surface a temporary compatibility island rather than the design center.

**Tech Stack:** Rust `no_std`, `heapless`, `portable-atomic`, existing `esp8266-wifi-sys` FFI only for the target backend, existing `src/wifi/compat/*` and `src/wifi/sdk/*` runtime bridge only for the target backend, `proptest` for bounded queue invariants, xtensa cross-checks with `cargo check`.

---

## File Structure

### New Files

- `docs/superpowers/notes/2026-04-22-wifi-core-callback-ownership.md` — explicit note for the FFI callback ownership model.
- `src/wifi/core/mod.rs` — core module docs and re-exports.
- `src/wifi/core/state.rs` — typestate markers, mode markers, and capability traits.
- `src/wifi/core/error.rs` — pure-Rust config, lifecycle, driver, and connect error types.
- `src/wifi/core/config.rs` — validated config newtypes and per-mode config objects.
- `src/wifi/core/resources.rs` — caller-owned storage object and bounded capacities.
- `src/wifi/core/backend.rs` — runtime-neutral backend trait, raw mode enum, and deadline helper.
- `src/wifi/core/driver.rs` — `WifiDriver<S, B, ...>` and typed lifecycle transitions.
- `src/wifi/core/event.rs` — bounded event broker and cursor-based event waits.
- `src/wifi/core/packet.rs` — release tokens, raw RX records, and bounded RX ring.
- `src/wifi/core/target.rs` — ESP8266 target backend and explicit callback-registration slot.
- `src/wifi/core/interface.rs` — `StaHandle`, `StaSession`, and borrowed capability handles.
- `tests/wifi_core_smoke.rs` — import and visibility smoke test.
- `tests/wifi_core_config.rs` — validated config tests.
- `tests/wifi_core_typestate.rs` — mode marker and resource-shape tests.
- `examples/wifi_core_connect.rs` — minimal blocking station-connect example using the new core API.

### Modified Files

- `Cargo.toml` — add `wifi-core` feature, add `proptest`, and add the new example stanza.
- `src/lib.rs` — expose the new `wifi_core` top-level module behind `wifi-core`.
- `research.md` — record the new core track and the explicit callback-registration rule.
- `.github/workflows/ci.yaml` — add host-native core tests and a targeted xtensa check for the new example.

### Test Locations

- `tests/wifi_core_smoke.rs`
- `tests/wifi_core_config.rs`
- `tests/wifi_core_typestate.rs`
- `src/wifi/core/backend.rs`
- `src/wifi/core/driver.rs`
- `src/wifi/core/event.rs`
- `src/wifi/core/packet.rs`
- `src/wifi/core/interface.rs`

### Design Rules For Implementers

- The core foundation must compile under `--features wifi-core` without enabling `esp8266-wifi-sys`.
- Host-native tests for the core must use `wifi-core`, not `wifi`.
- The target backend is the only part of the new foundation that may depend on `esp8266-wifi-sys`, `src/wifi/compat/*`, or `src/wifi/sdk/*`.
- No step may introduce a `static mut` event broker, packet ring, or driver state store.
- If a global raw pointer is needed for SDK callbacks, it must be one backend-local audited slot whose pointee lives inside caller-owned `WifiResources`.
- The legacy public `wifi` APIs are not the design center. They may remain temporarily, but CI for this milestone must validate the new core directly.

### Task 1: Carve Out A Host-Testable `wifi-core` Feature

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/lib.rs`
- Create: `tests/wifi_core_smoke.rs`

- [ ] **Step 1: Write the failing smoke test**

Create `tests/wifi_core_smoke.rs`:

```rust
use esp8266_hal::wifi_core::{Configured, Uninitialized, WifiResources};

#[test]
fn wifi_core_module_is_public() {
    let _ = core::mem::size_of::<WifiResources<8, 4, 2>>();
    let _ = core::mem::size_of::<Configured<()>>();
    let _ = core::mem::size_of::<Uninitialized>();
}
```

- [ ] **Step 2: Run the smoke test and verify it fails**

Run:

```bash
cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --test wifi_core_smoke -q
```

Expected: FAIL with `package 'esp8266-hal' does not have these features: wifi-core`.

- [ ] **Step 3: Add the new `wifi-core` feature and module exposure**

Modify `Cargo.toml` feature and dev-dependency sections so they contain:

```toml
[dev-dependencies]
panic-halt = "1.0"
embassy-net = { version = "=0.5.0", default-features = false, features = [
  "proto-ipv4",
  "medium-ethernet",
  "tcp",
  "udp",
  "dhcpv4",
] }
embedded-io-async = "0.6"
embassy-futures = "0.1"
proptest = "1"

[features]
default = ["interrupt", "rt"]
interrupt = ["paste"]
rt = ["xtensa-lx-rt"]
wifi-core = [
  "dep:portable-atomic",
  "dep:heapless",
]
wifi = [
  "wifi-core",
  "dep:esp8266-wifi-sys",
  "dep:esp-alloc",
  "dep:bitflags",
  "dep:log",
  "dep:smoltcp",
  "dep:embassy-executor",
  "dep:embassy-time",
  "dep:embassy-time-driver",
  "dep:embassy-time-queue-driver",
  "dep:embassy-sync",
  "dep:critical-section",
  "dep:static_cell",
]
wifi-sta = ["wifi"]
wifi-ap = ["wifi"]
wifi-embassy-net = [
  "wifi",
  "dep:embassy-net-driver-channel",
  "dep:embassy-futures",
]
wifi-wps = ["wifi"]
wifi-smartconfig = ["wifi", "esp8266-wifi-sys/smartconfig"]
wifi-promiscuous = ["wifi"]
wifi-esp-now = ["wifi", "esp8266-wifi-sys/esp-now"]
```

Modify `src/lib.rs` to add:

```rust
#[cfg(feature = "wifi-core")]
#[path = "wifi/core/mod.rs"]
pub mod wifi_core;

#[cfg(feature = "wifi")]
#[deny(unsafe_op_in_unsafe_fn)]
pub mod wifi;
```

Keep the existing `#[cfg(feature = "wifi")]` gate on the legacy `wifi` module. Do not make `wifi-core` compile `src/wifi/mod.rs`.

- [ ] **Step 4: Re-run the smoke test**

Run:

```bash
cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --test wifi_core_smoke -q
```

Expected: FAIL with unresolved imports for `esp8266_hal::wifi_core::{Configured, Uninitialized, WifiResources}`. The feature now exists and the build gets past feature resolution.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml src/lib.rs tests/wifi_core_smoke.rs
git commit -m "build: add wifi-core feature boundary"
```

### Task 2: Scaffold The Pure Core Module And Typestate Markers

**Files:**
- Create: `src/wifi/core/mod.rs`
- Create: `src/wifi/core/state.rs`
- Create: `src/wifi/core/resources.rs`
- Modify: `tests/wifi_core_smoke.rs`
- Create: `tests/wifi_core_typestate.rs`

- [ ] **Step 1: Add the failing typestate test**

Create `tests/wifi_core_typestate.rs`:

```rust
use esp8266_hal::wifi_core::{ApMode, HasAp, HasSta, MixedMode, StaMode, WifiResources};

fn assert_has_sta<M: HasSta>() {}
fn assert_has_ap<M: HasAp>() {}

#[test]
fn mode_capabilities_are_encoded_in_traits() {
    assert_has_sta::<StaMode>();
    assert_has_sta::<MixedMode>();
    assert_has_ap::<ApMode>();
    assert_has_ap::<MixedMode>();
}

#[test]
fn resources_report_declared_capacities() {
    let resources = WifiResources::<8, 4, 2>::new();
    assert_eq!(resources.event_capacity(), 8);
    assert_eq!(resources.rx_capacity(), 4);
    assert_eq!(resources.subscriber_capacity(), 2);
}
```

- [ ] **Step 2: Run the smoke and typestate tests and verify they fail**

Run:

```bash
cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --test wifi_core_smoke --test wifi_core_typestate -q
```

Expected: FAIL with unresolved items from `esp8266_hal::wifi_core`.

- [ ] **Step 3: Add the core module shell**

Create `src/wifi/core/mod.rs`:

```rust
//! Strong-typestate, caller-owned WiFi core API.

mod resources;
mod state;

pub use resources::WifiResources;
pub use state::{
    ApMode, Configured, HasAp, HasSta, Initialized, MixedMode, StaMode, Started, Uninitialized,
};
```

Create `src/wifi/core/state.rs`:

```rust
use core::marker::PhantomData;

pub struct Uninitialized;
pub struct Initialized;
pub struct Configured<M>(pub(crate) PhantomData<M>);
pub struct Started<M>(pub(crate) PhantomData<M>);

pub struct StaMode;
pub struct ApMode;
pub struct MixedMode;

pub trait HasSta {}
pub trait HasAp {}

impl HasSta for StaMode {}
impl HasSta for MixedMode {}
impl HasAp for ApMode {}
impl HasAp for MixedMode {}
```

Create `src/wifi/core/resources.rs`:

```rust
pub struct WifiResources<const EVENTS: usize, const RX: usize, const SUBS: usize> {
    _priv: (),
}

impl<const EVENTS: usize, const RX: usize, const SUBS: usize> WifiResources<EVENTS, RX, SUBS> {
    pub const fn new() -> Self {
        Self { _priv: () }
    }

    pub const fn event_capacity(&self) -> usize {
        EVENTS
    }

    pub const fn rx_capacity(&self) -> usize {
        RX
    }

    pub const fn subscriber_capacity(&self) -> usize {
        SUBS
    }
}
```

Update `tests/wifi_core_smoke.rs` to import only items that exist now:

```rust
use esp8266_hal::wifi_core::{Configured, Uninitialized, WifiResources};

#[test]
fn wifi_core_module_is_public() {
    let _ = core::mem::size_of::<WifiResources<8, 4, 2>>();
    let _ = core::mem::size_of::<Configured<()>>();
    let _ = core::mem::size_of::<Uninitialized>();
}
```

- [ ] **Step 4: Re-run the smoke and typestate tests**

Run:

```bash
cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --test wifi_core_smoke --test wifi_core_typestate -q
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/wifi/core/mod.rs src/wifi/core/state.rs src/wifi/core/resources.rs tests/wifi_core_smoke.rs tests/wifi_core_typestate.rs
git commit -m "feat: scaffold wifi core typestate module"
```

### Task 3: Add Validated Config Types And Core Error Types

**Files:**
- Create: `src/wifi/core/error.rs`
- Create: `src/wifi/core/config.rs`
- Modify: `src/wifi/core/mod.rs`
- Create: `tests/wifi_core_config.rs`

- [ ] **Step 1: Write the failing config tests**

Create `tests/wifi_core_config.rs`:

```rust
use esp8266_hal::wifi_core::{
    AccessPointConfig, Channel, ClientConfig, ConfigError, Credential, InitConfig, Passphrase,
    Ssid,
};

#[test]
fn ssid_rejects_more_than_32_bytes() {
    assert!(matches!(
        Ssid::try_from("123456789012345678901234567890123"),
        Err(ConfigError::SsidTooLong)
    ));
}

#[test]
fn wpa2_passphrase_rejects_short_values() {
    assert!(matches!(
        Passphrase::wpa2("short"),
        Err(ConfigError::PassphraseTooShort)
    ));
}

#[test]
fn channel_rejects_zero() {
    assert!(matches!(Channel::try_from(0u8), Err(ConfigError::InvalidChannel(0))));
}

#[test]
fn open_client_config_preserves_ssid() {
    let ssid = Ssid::try_from("core-net").unwrap();
    let client = ClientConfig::new(ssid.clone(), Credential::Open);
    assert_eq!(client.ssid(), &ssid);
}

#[test]
fn open_ap_config_preserves_channel() {
    let ap = AccessPointConfig::open(
        Ssid::try_from("core-ap").unwrap(),
        Channel::try_from(6u8).unwrap(),
    );
    assert_eq!(ap.channel().get(), 6);
}

#[test]
fn init_config_defaults_match_current_buffer_counts() {
    let init = InitConfig::default();
    assert_eq!(init.rx_buf_num, 16);
    assert_eq!(init.rx_pkt_num, 7);
    assert_eq!(init.tx_buf_num, 6);
}
```

- [ ] **Step 2: Run the config tests and verify they fail**

Run:

```bash
cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --test wifi_core_config -q
```

Expected: FAIL with unresolved imports for the new config and error types.

- [ ] **Step 3: Implement config and error types**

Create `src/wifi/core/error.rs`:

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigError {
    SsidTooLong,
    PassphraseTooShort,
    PassphraseTooLong,
    InvalidChannel(u8),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InitError {
    AlreadyInitialized,
    BackendUnavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DriverError {
    Timeout,
    EventOverflow,
    PacketOverflow,
    BackendRejected,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectError {
    Timeout,
    Disconnected,
    BackendRejected,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisconnectError {
    BackendRejected,
}
```

Create `src/wifi/core/config.rs`:

```rust
use heapless::Vec;

use super::error::ConfigError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ssid(Vec<u8, 32>);

impl TryFrom<&str> for Ssid {
    type Error = ConfigError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if value.len() > 32 {
            return Err(ConfigError::SsidTooLong);
        }
        let mut bytes = Vec::new();
        bytes.extend_from_slice(value.as_bytes()).unwrap();
        Ok(Self(bytes))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Passphrase(Vec<u8, 64>);

impl Passphrase {
    pub fn wpa2(value: &str) -> Result<Self, ConfigError> {
        if value.len() < 8 {
            return Err(ConfigError::PassphraseTooShort);
        }
        if value.len() > 64 {
            return Err(ConfigError::PassphraseTooLong);
        }
        let mut bytes = Vec::new();
        bytes.extend_from_slice(value.as_bytes()).unwrap();
        Ok(Self(bytes))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Channel(u8);

impl Channel {
    pub const fn get(self) -> u8 {
        self.0
    }
}

impl TryFrom<u8> for Channel {
    type Error = ConfigError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1..=14 => Ok(Self(value)),
            other => Err(ConfigError::InvalidChannel(other)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Credential {
    Open,
    Wpa2(Passphrase),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientConfig {
    ssid: Ssid,
    credential: Credential,
}

impl ClientConfig {
    pub fn new(ssid: Ssid, credential: Credential) -> Self {
        Self { ssid, credential }
    }

    pub fn ssid(&self) -> &Ssid {
        &self.ssid
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessPointConfig {
    ssid: Ssid,
    channel: Channel,
    credential: Credential,
}

impl AccessPointConfig {
    pub fn open(ssid: Ssid, channel: Channel) -> Self {
        Self {
            ssid,
            channel,
            credential: Credential::Open,
        }
    }

    pub fn channel(&self) -> Channel {
        self.channel
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MixedConfig {
    client: ClientConfig,
    ap: AccessPointConfig,
}

impl MixedConfig {
    pub fn new(client: ClientConfig, ap: AccessPointConfig) -> Self {
        Self { client, ap }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InitConfig {
    pub rx_buf_num: u8,
    pub rx_pkt_num: u8,
    pub tx_buf_num: u8,
}

impl Default for InitConfig {
    fn default() -> Self {
        Self {
            rx_buf_num: 16,
            rx_pkt_num: 7,
            tx_buf_num: 6,
        }
    }
}
```

Modify `src/wifi/core/mod.rs`:

```rust
mod config;
mod error;
mod resources;
mod state;

pub use config::{AccessPointConfig, Channel, ClientConfig, Credential, InitConfig, MixedConfig, Passphrase, Ssid};
pub use error::{ConfigError, ConnectError, DisconnectError, DriverError, InitError};
pub use resources::WifiResources;
pub use state::{
    ApMode, Configured, HasAp, HasSta, Initialized, MixedMode, StaMode, Started, Uninitialized,
};
```

- [ ] **Step 4: Re-run the config tests**

Run:

```bash
cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --test wifi_core_config -q
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/wifi/core/error.rs src/wifi/core/config.rs src/wifi/core/mod.rs tests/wifi_core_config.rs
git commit -m "feat: add validated wifi core config types"
```

### Task 4: Lock Down The Callback Ownership Model And Backend Trait

**Files:**
- Create: `docs/superpowers/notes/2026-04-22-wifi-core-callback-ownership.md`
- Create: `src/wifi/core/backend.rs`
- Modify: `src/wifi/core/mod.rs`
- Test: `src/wifi/core/backend.rs`

- [ ] **Step 1: Write the callback ownership note**

Create `docs/superpowers/notes/2026-04-22-wifi-core-callback-ownership.md`:

```markdown
# WiFi Core Callback Ownership

## Rule

The ESP8266 target backend may use one backend-local global raw pointer slot to
bridge SDK callbacks into Rust. The pointee itself must live inside
caller-owned `WifiResources`.

## Model

- `WifiResources` owns `CallbackContext`
- the target backend installs `CallbackContext` into one `AtomicPtr<()>`
- installation is guarded by compare-and-exchange and returns a
  `CallbackRegistration` token
- the registration token is stored in the target backend
- dropping the registration token clears the global slot
- host-native tests never touch this slot because they use `FakeBackend`

## Soundness Conditions

- only one live target backend may own the callback slot
- the raw pointer always points to storage owned by the caller
- callback trampolines immediately recover `CallbackContext` and then delegate
  to safe broker and packet methods
- no event broker or packet ring may be global
```

- [ ] **Step 2: Add the failing backend unit tests**

Create `src/wifi/core/backend.rs` with only this test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadline_reports_timeout_at_or_after_limit() {
        let deadline = Deadline::new(1_000, 250);
        assert!(!deadline.is_expired(1_249));
        assert!(deadline.is_expired(1_250));
        assert!(deadline.is_expired(1_251));
    }

    #[test]
    fn raw_mode_constructors_match_expected_variants() {
        assert_eq!(RawMode::for_sta(), RawMode::Sta);
        assert_eq!(RawMode::for_ap(), RawMode::Ap);
        assert_eq!(RawMode::for_mixed(), RawMode::ApSta);
    }
}
```

- [ ] **Step 3: Run the backend unit tests and verify they fail**

Run:

```bash
cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --lib
```

Expected: FAIL because `Deadline` and `RawMode` do not exist yet.

- [ ] **Step 4: Implement the pure backend abstraction**

Replace `src/wifi/core/backend.rs` with:

```rust
use core::task::Waker;

use super::{ConnectError, DisconnectError, DriverError, InitConfig, InitError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RawMode {
    Sta,
    Ap,
    ApSta,
}

impl RawMode {
    pub const fn for_sta() -> Self {
        Self::Sta
    }

    pub const fn for_ap() -> Self {
        Self::Ap
    }

    pub const fn for_mixed() -> Self {
        Self::ApSta
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Deadline {
    start_ms: u64,
    timeout_ms: u32,
}

impl Deadline {
    pub const fn new(start_ms: u64, timeout_ms: u32) -> Self {
        Self { start_ms, timeout_ms }
    }

    pub fn is_expired(self, now_ms: u64) -> bool {
        now_ms.saturating_sub(self.start_ms) >= self.timeout_ms as u64
    }
}

pub trait WifiBackend {
    fn initialize(&mut self, config: &InitConfig) -> Result<(), InitError>;
    fn set_mode(&mut self, mode: RawMode) -> Result<(), DriverError>;
    fn start(&mut self) -> Result<(), DriverError>;
    fn stop(&mut self) -> Result<(), DriverError>;
    fn connect(&mut self) -> Result<(), ConnectError>;
    fn disconnect(&mut self) -> Result<(), DisconnectError>;
    fn now_ms(&self) -> u64;
    fn spin_once(&mut self);
    fn register_waker(&self, _waker: &Waker) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadline_reports_timeout_at_or_after_limit() {
        let deadline = Deadline::new(1_000, 250);
        assert!(!deadline.is_expired(1_249));
        assert!(deadline.is_expired(1_250));
        assert!(deadline.is_expired(1_251));
    }

    #[test]
    fn raw_mode_constructors_match_expected_variants() {
        assert_eq!(RawMode::for_sta(), RawMode::Sta);
        assert_eq!(RawMode::for_ap(), RawMode::Ap);
        assert_eq!(RawMode::for_mixed(), RawMode::ApSta);
    }
}
```

Modify `src/wifi/core/mod.rs`:

```rust
mod backend;
mod config;
mod error;
mod resources;
mod state;

pub use backend::{Deadline, RawMode, WifiBackend};
pub use config::{AccessPointConfig, Channel, ClientConfig, Credential, InitConfig, MixedConfig, Passphrase, Ssid};
pub use error::{ConfigError, ConnectError, DisconnectError, DriverError, InitError};
pub use resources::WifiResources;
pub use state::{
    ApMode, Configured, HasAp, HasSta, Initialized, MixedMode, StaMode, Started, Uninitialized,
};
```

- [ ] **Step 5: Re-run the backend tests**

Run:

```bash
cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --lib
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add docs/superpowers/notes/2026-04-22-wifi-core-callback-ownership.md src/wifi/core/backend.rs src/wifi/core/mod.rs
git commit -m "docs: define wifi core callback ownership model"
```

### Task 5: Implement The Pure Typestate Driver Lifecycle

**Files:**
- Create: `src/wifi/core/driver.rs`
- Modify: `src/wifi/core/mod.rs`
- Test: `src/wifi/core/driver.rs`

- [ ] **Step 1: Add the failing lifecycle unit test**

Create `src/wifi/core/driver.rs` with only this test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::wifi_core::{ClientConfig, Credential, InitConfig, Ssid, WifiBackend, WifiResources};

    #[derive(Default)]
    struct FakeBackend {
        started: bool,
    }

    impl WifiBackend for FakeBackend {
        fn initialize(&mut self, _config: &InitConfig) -> Result<(), crate::wifi_core::InitError> { Ok(()) }
        fn set_mode(&mut self, _mode: crate::wifi_core::RawMode) -> Result<(), crate::wifi_core::DriverError> { Ok(()) }
        fn start(&mut self) -> Result<(), crate::wifi_core::DriverError> { self.started = true; Ok(()) }
        fn stop(&mut self) -> Result<(), crate::wifi_core::DriverError> { self.started = false; Ok(()) }
        fn connect(&mut self) -> Result<(), crate::wifi_core::ConnectError> { Ok(()) }
        fn disconnect(&mut self) -> Result<(), crate::wifi_core::DisconnectError> { Ok(()) }
        fn now_ms(&self) -> u64 { 0 }
        fn spin_once(&mut self) {}
    }

    #[test]
    fn lifecycle_moves_between_explicit_driver_states() {
        let mut resources = WifiResources::<8, 4, 2>::new();
        let driver = WifiDriver::with_backend(&mut resources, FakeBackend::default());
        let driver = driver.initialize(InitConfig::default()).unwrap();
        let driver = driver
            .configure_sta(ClientConfig::new(Ssid::try_from("core-net").unwrap(), Credential::Open))
            .unwrap();
        let driver = driver.start().unwrap();
        let _driver = driver.stop().unwrap();
    }
}
```

- [ ] **Step 2: Run the lifecycle test and verify it fails**

Run:

```bash
cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --lib lifecycle_moves_between_explicit_driver_states -- --exact
```

Expected: FAIL because `WifiDriver`, `with_backend`, `initialize`, `configure_sta`, `start`, and `stop` do not exist yet.

- [ ] **Step 3: Implement `WifiDriver` with borrowed resources**

Replace `src/wifi/core/driver.rs` with:

```rust
use core::marker::PhantomData;

use super::{
    ApMode, ClientConfig, ConfigError, Configured, DisconnectError, DriverError, InitConfig,
    InitError, Initialized, MixedConfig, MixedMode, RawMode, Started, StaMode, Uninitialized,
    WifiBackend, WifiResources,
};

pub struct WifiDriver<'d, S, B, const EVENTS: usize, const RX: usize, const SUBS: usize> {
    backend: B,
    resources: &'d mut WifiResources<EVENTS, RX, SUBS>,
    _state: PhantomData<S>,
}

impl<'d, B, const EVENTS: usize, const RX: usize, const SUBS: usize>
    WifiDriver<'d, Uninitialized, B, EVENTS, RX, SUBS>
{
    pub fn with_backend(
        resources: &'d mut WifiResources<EVENTS, RX, SUBS>,
        backend: B,
    ) -> Self {
        Self {
            backend,
            resources,
            _state: PhantomData,
        }
    }
}

impl<'d, B: WifiBackend, const EVENTS: usize, const RX: usize, const SUBS: usize>
    WifiDriver<'d, Uninitialized, B, EVENTS, RX, SUBS>
{
    pub fn initialize(mut self, config: InitConfig) -> Result<WifiDriver<'d, Initialized, B, EVENTS, RX, SUBS>, InitError> {
        self.backend.initialize(&config)?;
        Ok(WifiDriver {
            backend: self.backend,
            resources: self.resources,
            _state: PhantomData,
        })
    }
}

impl<'d, B: WifiBackend, const EVENTS: usize, const RX: usize, const SUBS: usize>
    WifiDriver<'d, Initialized, B, EVENTS, RX, SUBS>
{
    pub fn configure_sta(
        self,
        _config: ClientConfig,
    ) -> Result<WifiDriver<'d, Configured<StaMode>, B, EVENTS, RX, SUBS>, ConfigError> {
        Ok(WifiDriver {
            backend: self.backend,
            resources: self.resources,
            _state: PhantomData,
        })
    }

    pub fn configure_ap(
        self,
        _config: super::AccessPointConfig,
    ) -> Result<WifiDriver<'d, Configured<ApMode>, B, EVENTS, RX, SUBS>, ConfigError> {
        Ok(WifiDriver {
            backend: self.backend,
            resources: self.resources,
            _state: PhantomData,
        })
    }

    pub fn configure_mixed(
        self,
        _config: MixedConfig,
    ) -> Result<WifiDriver<'d, Configured<MixedMode>, B, EVENTS, RX, SUBS>, ConfigError> {
        Ok(WifiDriver {
            backend: self.backend,
            resources: self.resources,
            _state: PhantomData,
        })
    }
}

impl<'d, B: WifiBackend, const EVENTS: usize, const RX: usize, const SUBS: usize>
    WifiDriver<'d, Configured<StaMode>, B, EVENTS, RX, SUBS>
{
    pub fn start(mut self) -> Result<WifiDriver<'d, Started<StaMode>, B, EVENTS, RX, SUBS>, DriverError> {
        self.backend.set_mode(RawMode::for_sta())?;
        self.backend.start()?;
        Ok(WifiDriver {
            backend: self.backend,
            resources: self.resources,
            _state: PhantomData,
        })
    }
}

impl<'d, B: WifiBackend, const EVENTS: usize, const RX: usize, const SUBS: usize>
    WifiDriver<'d, Configured<ApMode>, B, EVENTS, RX, SUBS>
{
    pub fn start(mut self) -> Result<WifiDriver<'d, Started<ApMode>, B, EVENTS, RX, SUBS>, DriverError> {
        self.backend.set_mode(RawMode::for_ap())?;
        self.backend.start()?;
        Ok(WifiDriver {
            backend: self.backend,
            resources: self.resources,
            _state: PhantomData,
        })
    }
}

impl<'d, B: WifiBackend, const EVENTS: usize, const RX: usize, const SUBS: usize>
    WifiDriver<'d, Configured<MixedMode>, B, EVENTS, RX, SUBS>
{
    pub fn start(mut self) -> Result<WifiDriver<'d, Started<MixedMode>, B, EVENTS, RX, SUBS>, DriverError> {
        self.backend.set_mode(RawMode::for_mixed())?;
        self.backend.start()?;
        Ok(WifiDriver {
            backend: self.backend,
            resources: self.resources,
            _state: PhantomData,
        })
    }
}

impl<'d, M, B: WifiBackend, const EVENTS: usize, const RX: usize, const SUBS: usize>
    WifiDriver<'d, Started<M>, B, EVENTS, RX, SUBS>
{
    pub fn stop(mut self) -> Result<WifiDriver<'d, Initialized, B, EVENTS, RX, SUBS>, DriverError> {
        self.backend.stop()?;
        Ok(WifiDriver {
            backend: self.backend,
            resources: self.resources,
            _state: PhantomData,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wifi_core::{ClientConfig, Credential, InitConfig, Ssid, WifiBackend, WifiResources};

    #[derive(Default)]
    struct FakeBackend {
        started: bool,
    }

    impl WifiBackend for FakeBackend {
        fn initialize(&mut self, _config: &InitConfig) -> Result<(), crate::wifi_core::InitError> { Ok(()) }
        fn set_mode(&mut self, _mode: crate::wifi_core::RawMode) -> Result<(), crate::wifi_core::DriverError> { Ok(()) }
        fn start(&mut self) -> Result<(), crate::wifi_core::DriverError> { self.started = true; Ok(()) }
        fn stop(&mut self) -> Result<(), crate::wifi_core::DriverError> { self.started = false; Ok(()) }
        fn connect(&mut self) -> Result<(), crate::wifi_core::ConnectError> { Ok(()) }
        fn disconnect(&mut self) -> Result<(), crate::wifi_core::DisconnectError> { Ok(()) }
        fn now_ms(&self) -> u64 { 0 }
        fn spin_once(&mut self) {}
    }

    #[test]
    fn lifecycle_moves_between_explicit_driver_states() {
        let mut resources = WifiResources::<8, 4, 2>::new();
        let driver = WifiDriver::with_backend(&mut resources, FakeBackend::default());
        let driver = driver.initialize(InitConfig::default()).unwrap();
        let driver = driver
            .configure_sta(ClientConfig::new(Ssid::try_from("core-net").unwrap(), Credential::Open))
            .unwrap();
        let driver = driver.start().unwrap();
        let _driver = driver.stop().unwrap();
    }
}
```

- [ ] **Step 4: Export the driver**

Modify `src/wifi/core/mod.rs`:

```rust
mod backend;
mod config;
mod driver;
mod error;
mod resources;
mod state;

pub use backend::{Deadline, RawMode, WifiBackend};
pub use config::{AccessPointConfig, Channel, ClientConfig, Credential, InitConfig, MixedConfig, Passphrase, Ssid};
pub use driver::WifiDriver;
pub use error::{ConfigError, ConnectError, DisconnectError, DriverError, InitError};
pub use resources::WifiResources;
pub use state::{
    ApMode, Configured, HasAp, HasSta, Initialized, MixedMode, StaMode, Started, Uninitialized,
};
```

- [ ] **Step 5: Re-run the lifecycle test**

Run:

```bash
cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --lib lifecycle_moves_between_explicit_driver_states -- --exact
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/wifi/core/driver.rs src/wifi/core/mod.rs
git commit -m "feat: add pure wifi core typestate driver"
```

### Task 6: Add A Cursor-Based Event Broker In Caller-Owned Resources

**Files:**
- Create: `src/wifi/core/event.rs`
- Modify: `src/wifi/core/resources.rs`
- Modify: `src/wifi/core/mod.rs`
- Test: `src/wifi/core/event.rs`

- [ ] **Step 1: Add the failing event tests**

Create `src/wifi/core/event.rs` with only this test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_observes_events_in_publish_order() {
        let mut broker = EventBroker::<8, 2>::new();
        let mut cursor = broker.claim_cursor().unwrap();

        broker.publish(WifiEvent::StaStarted).unwrap();
        broker.publish(WifiEvent::StaConnected).unwrap();

        assert_eq!(broker.try_next(&mut cursor), Some(WifiEvent::StaStarted));
        assert_eq!(broker.try_next(&mut cursor), Some(WifiEvent::StaConnected));
        assert_eq!(broker.try_next(&mut cursor), None);
    }

    #[test]
    fn publish_reports_overflow_when_capacity_is_exhausted() {
        let mut broker = EventBroker::<1, 1>::new();
        broker.publish(WifiEvent::StaStarted).unwrap();
        assert_eq!(broker.publish(WifiEvent::StaConnected), Err(DriverError::EventOverflow));
    }
}
```

- [ ] **Step 2: Run the event tests and verify they fail**

Run:

```bash
cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --lib
```

Expected: FAIL because `EventBroker`, `EventCursor`, and `WifiEvent` do not exist yet.

- [ ] **Step 3: Implement the broker with explicit cursors**

Replace `src/wifi/core/event.rs` with:

```rust
use core::future::poll_fn;
use core::task::Poll;

use heapless::Vec;

use super::{Deadline, DriverError, WifiBackend};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WifiEvent {
    DriverReady,
    StaStarted,
    StaStopped,
    StaConnected,
    StaDisconnected,
    ApStarted,
    ApStopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventCursor {
    slot: usize,
    next_index: usize,
}

pub struct EventBroker<const EVENTS: usize, const SUBS: usize> {
    events: Vec<WifiEvent, EVENTS>,
    claimed: [bool; SUBS],
    positions: [usize; SUBS],
}

impl<const EVENTS: usize, const SUBS: usize> EventBroker<EVENTS, SUBS> {
    pub const fn new() -> Self {
        Self {
            events: Vec::new(),
            claimed: [false; SUBS],
            positions: [0; SUBS],
        }
    }

    pub fn claim_cursor(&mut self) -> Result<EventCursor, DriverError> {
        for index in 0..SUBS {
            if !self.claimed[index] {
                self.claimed[index] = true;
                self.positions[index] = 0;
                return Ok(EventCursor {
                    slot: index,
                    next_index: 0,
                });
            }
        }
        Err(DriverError::EventOverflow)
    }

    pub fn release_cursor(&mut self, cursor: EventCursor) {
        self.claimed[cursor.slot] = false;
        self.positions[cursor.slot] = 0;
        self.compact();
    }

    pub fn publish(&mut self, event: WifiEvent) -> Result<(), DriverError> {
        self.events
            .push(event)
            .map_err(|_| DriverError::EventOverflow)
    }

    pub fn try_next(&mut self, cursor: &mut EventCursor) -> Option<WifiEvent> {
        let event = self.events.get(cursor.next_index).copied()?;
        cursor.next_index += 1;
        self.positions[cursor.slot] = cursor.next_index;
        self.compact();
        Some(event)
    }

    pub fn wait_blocking<B: WifiBackend>(
        &mut self,
        cursor: &mut EventCursor,
        backend: &mut B,
        wanted: WifiEvent,
        timeout_ms: u32,
    ) -> Result<WifiEvent, DriverError> {
        let deadline = Deadline::new(backend.now_ms(), timeout_ms);
        loop {
            if let Some(event) = self.try_next(cursor) {
                if event == wanted {
                    return Ok(event);
                }
            }
            if deadline.is_expired(backend.now_ms()) {
                return Err(DriverError::Timeout);
            }
            backend.spin_once();
        }
    }

    pub async fn wait_async<B: WifiBackend>(
        &mut self,
        cursor: &mut EventCursor,
        backend: &B,
        wanted: WifiEvent,
    ) -> WifiEvent {
        poll_fn(|cx| {
            backend.register_waker(cx.waker());
            match self.try_next(cursor) {
                Some(event) if event == wanted => Poll::Ready(event),
                _ => Poll::Pending,
            }
        })
        .await
    }

    fn compact(&mut self) {
        let mut min = self.events.len();
        let mut any_claimed = false;
        for index in 0..SUBS {
            if self.claimed[index] {
                any_claimed = true;
                min = min.min(self.positions[index]);
            }
        }

        if !any_claimed || min == 0 {
            return;
        }

        for _ in 0..min {
            self.events.remove(0);
        }

        for index in 0..SUBS {
            if self.claimed[index] {
                self.positions[index] -= min;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_observes_events_in_publish_order() {
        let mut broker = EventBroker::<8, 2>::new();
        let mut cursor = broker.claim_cursor().unwrap();

        broker.publish(WifiEvent::StaStarted).unwrap();
        broker.publish(WifiEvent::StaConnected).unwrap();

        assert_eq!(broker.try_next(&mut cursor), Some(WifiEvent::StaStarted));
        assert_eq!(broker.try_next(&mut cursor), Some(WifiEvent::StaConnected));
        assert_eq!(broker.try_next(&mut cursor), None);
    }

    #[test]
    fn publish_reports_overflow_when_capacity_is_exhausted() {
        let mut broker = EventBroker::<1, 1>::new();
        broker.publish(WifiEvent::StaStarted).unwrap();
        assert_eq!(broker.publish(WifiEvent::StaConnected), Err(DriverError::EventOverflow));
    }
}
```

- [ ] **Step 4: Put the broker inside `WifiResources`**

Replace `src/wifi/core/resources.rs` with:

```rust
use super::event::EventBroker;

pub struct WifiResources<const EVENTS: usize, const RX: usize, const SUBS: usize> {
    pub(crate) events: EventBroker<EVENTS, SUBS>,
}

impl<const EVENTS: usize, const RX: usize, const SUBS: usize> WifiResources<EVENTS, RX, SUBS> {
    pub const fn new() -> Self {
        Self {
            events: EventBroker::new(),
        }
    }

    pub const fn event_capacity(&self) -> usize {
        EVENTS
    }

    pub const fn rx_capacity(&self) -> usize {
        RX
    }

    pub const fn subscriber_capacity(&self) -> usize {
        SUBS
    }
}
```

Modify `src/wifi/core/mod.rs`:

```rust
mod backend;
mod config;
mod driver;
mod error;
mod event;
mod resources;
mod state;

pub use backend::{Deadline, RawMode, WifiBackend};
pub use config::{AccessPointConfig, Channel, ClientConfig, Credential, InitConfig, MixedConfig, Passphrase, Ssid};
pub use driver::WifiDriver;
pub use error::{ConfigError, ConnectError, DisconnectError, DriverError, InitError};
pub use event::{EventBroker, EventCursor, WifiEvent};
pub use resources::WifiResources;
pub use state::{
    ApMode, Configured, HasAp, HasSta, Initialized, MixedMode, StaMode, Started, Uninitialized,
};
```

- [ ] **Step 5: Re-run the event tests**

Run:

```bash
cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --lib
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/wifi/core/event.rs src/wifi/core/resources.rs src/wifi/core/mod.rs
git commit -m "feat: add wifi core event broker"
```

### Task 7: Add Packet Ownership And The RX Ring

**Files:**
- Create: `src/wifi/core/packet.rs`
- Modify: `src/wifi/core/resources.rs`
- Modify: `src/wifi/core/mod.rs`
- Test: `src/wifi/core/packet.rs`

- [ ] **Step 1: Add the failing packet tests**

Create `src/wifi/core/packet.rs` with only this test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn ring_counts_drops_when_full() {
        let mut ring = RxRing::<1>::new();
        assert!(ring.try_push(unsafe { RawRxRecord::fake(32) }).is_ok());
        assert!(ring.try_push(unsafe { RawRxRecord::fake(64) }).is_err());
        assert_eq!(ring.drops(), 1);
    }

    proptest! {
        #[test]
        fn ring_len_never_exceeds_capacity(ops in proptest::collection::vec(0u8..3, 1..32)) {
            let mut ring = RxRing::<4>::new();
            for op in ops {
                match op {
                    0 => { let _ = ring.try_push(unsafe { RawRxRecord::fake(64) }); }
                    _ => { let _ = ring.pop(); }
                }
                prop_assert!(ring.len() <= 4);
            }
        }
    }
}
```

- [ ] **Step 2: Run the packet tests and verify they fail**

Run:

```bash
cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --lib
```

Expected: FAIL because `RawRxRecord` and `RxRing` do not exist yet.

- [ ] **Step 3: Implement packet ownership and the bounded RX ring**

Replace `src/wifi/core/packet.rs` with:

```rust
use core::ffi::c_void;

use super::DriverError;

#[derive(Clone, Copy)]
pub struct ReleaseToken {
    buffer: *mut c_void,
    eb: *mut c_void,
}

impl ReleaseToken {
    pub const unsafe fn from_raw(buffer: *mut c_void, eb: *mut c_void) -> Self {
        Self { buffer, eb }
    }

    pub unsafe fn fake() -> Self {
        Self::from_raw(core::ptr::null_mut(), core::ptr::null_mut())
    }

    pub fn release(self) {}
}

#[derive(Clone, Copy)]
pub struct RawRxRecord {
    pub buffer: *mut c_void,
    pub len: u16,
    pub release: ReleaseToken,
}

impl RawRxRecord {
    pub const unsafe fn from_raw(buffer: *mut c_void, len: u16, eb: *mut c_void) -> Self {
        Self {
            buffer,
            len,
            release: ReleaseToken::from_raw(buffer, eb),
        }
    }

    pub unsafe fn fake(len: u16) -> Self {
        Self {
            buffer: core::ptr::null_mut(),
            len,
            release: ReleaseToken::fake(),
        }
    }
}

pub struct RxRing<const N: usize> {
    entries: [Option<RawRxRecord>; N],
    head: usize,
    tail: usize,
    len: usize,
    drops: u32,
}

impl<const N: usize> RxRing<N> {
    pub const fn new() -> Self {
        Self {
            entries: [const { None }; N],
            head: 0,
            tail: 0,
            len: 0,
            drops: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn drops(&self) -> u32 {
        self.drops
    }

    pub fn try_push(&mut self, record: RawRxRecord) -> Result<(), DriverError> {
        if self.len == N {
            self.drops += 1;
            return Err(DriverError::PacketOverflow);
        }
        self.entries[self.head] = Some(record);
        self.head = (self.head + 1) % N;
        self.len += 1;
        Ok(())
    }

    pub fn pop(&mut self) -> Option<RawRxRecord> {
        if self.len == 0 {
            return None;
        }
        let record = self.entries[self.tail].take();
        self.tail = (self.tail + 1) % N;
        self.len -= 1;
        record
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn ring_counts_drops_when_full() {
        let mut ring = RxRing::<1>::new();
        assert!(ring.try_push(unsafe { RawRxRecord::fake(32) }).is_ok());
        assert!(ring.try_push(unsafe { RawRxRecord::fake(64) }).is_err());
        assert_eq!(ring.drops(), 1);
    }

    proptest! {
        #[test]
        fn ring_len_never_exceeds_capacity(ops in proptest::collection::vec(0u8..3, 1..32)) {
            let mut ring = RxRing::<4>::new();
            for op in ops {
                match op {
                    0 => { let _ = ring.try_push(unsafe { RawRxRecord::fake(64) }); }
                    _ => { let _ = ring.pop(); }
                }
                prop_assert!(ring.len() <= 4);
            }
        }
    }
}
```

- [ ] **Step 4: Put the RX ring inside `WifiResources`**

Replace `src/wifi/core/resources.rs` with:

```rust
use super::event::EventBroker;
use super::packet::RxRing;

pub struct WifiResources<const EVENTS: usize, const RX: usize, const SUBS: usize> {
    pub(crate) events: EventBroker<EVENTS, SUBS>,
    pub(crate) rx: RxRing<RX>,
}

impl<const EVENTS: usize, const RX: usize, const SUBS: usize> WifiResources<EVENTS, RX, SUBS> {
    pub const fn new() -> Self {
        Self {
            events: EventBroker::new(),
            rx: RxRing::new(),
        }
    }

    pub const fn event_capacity(&self) -> usize {
        EVENTS
    }

    pub const fn rx_capacity(&self) -> usize {
        RX
    }

    pub const fn subscriber_capacity(&self) -> usize {
        SUBS
    }
}
```

Modify `src/wifi/core/mod.rs`:

```rust
mod backend;
mod config;
mod driver;
mod error;
mod event;
mod packet;
mod resources;
mod state;

pub use backend::{Deadline, RawMode, WifiBackend};
pub use config::{AccessPointConfig, Channel, ClientConfig, Credential, InitConfig, MixedConfig, Passphrase, Ssid};
pub use driver::WifiDriver;
pub use error::{ConfigError, ConnectError, DisconnectError, DriverError, InitError};
pub use event::{EventBroker, EventCursor, WifiEvent};
pub use packet::{RawRxRecord, ReleaseToken, RxRing};
pub use resources::WifiResources;
pub use state::{
    ApMode, Configured, HasAp, HasSta, Initialized, MixedMode, StaMode, Started, Uninitialized,
};
```

- [ ] **Step 5: Re-run the packet tests**

Run:

```bash
cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --lib
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/wifi/core/packet.rs src/wifi/core/resources.rs src/wifi/core/mod.rs
git commit -m "feat: add wifi core packet ownership model"
```

### Task 8: Add The ESP8266 Target Backend With An Explicit Callback Slot

**Files:**
- Create: `src/wifi/core/target.rs`
- Modify: `src/wifi/core/mod.rs`
- Test: xtensa compile via `cargo check`

- [ ] **Step 1: Add the target backend shell**

Create `src/wifi/core/target.rs`:

```rust
#![cfg(feature = "wifi")]

use core::marker::PhantomData;
use core::ptr;

use portable_atomic::{AtomicPtr, Ordering};

use super::{
    ConnectError, DisconnectError, DriverError, InitConfig, InitError, RawRxRecord, RawMode,
    WifiBackend, WifiEvent, WifiResources,
};

static ACTIVE_CALLBACK_CONTEXT: AtomicPtr<()> = AtomicPtr::new(ptr::null_mut());

pub(crate) struct CallbackContext<const EVENTS: usize, const RX: usize, const SUBS: usize> {
    pub resources: *mut WifiResources<EVENTS, RX, SUBS>,
}

pub(crate) struct CallbackRegistration<const EVENTS: usize, const RX: usize, const SUBS: usize> {
    _marker: PhantomData<CallbackContext<EVENTS, RX, SUBS>>,
}

impl<const EVENTS: usize, const RX: usize, const SUBS: usize> CallbackRegistration<EVENTS, RX, SUBS> {
    pub fn install(context: &mut CallbackContext<EVENTS, RX, SUBS>) -> Result<Self, InitError> {
        ACTIVE_CALLBACK_CONTEXT
            .compare_exchange(
                ptr::null_mut(),
                context as *mut _ as *mut (),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map_err(|_| InitError::AlreadyInitialized)?;
        Ok(Self {
            _marker: PhantomData,
        })
    }
}

impl<const EVENTS: usize, const RX: usize, const SUBS: usize> Drop
    for CallbackRegistration<EVENTS, RX, SUBS>
{
    fn drop(&mut self) {
        ACTIVE_CALLBACK_CONTEXT.store(ptr::null_mut(), Ordering::Release);
    }
}

pub struct Esp8266TargetBackend<const EVENTS: usize, const RX: usize, const SUBS: usize> {
    callback_context: CallbackContext<EVENTS, RX, SUBS>,
    registration: Option<CallbackRegistration<EVENTS, RX, SUBS>>,
}

impl<const EVENTS: usize, const RX: usize, const SUBS: usize> Esp8266TargetBackend<EVENTS, RX, SUBS> {
    pub fn new(resources: &mut WifiResources<EVENTS, RX, SUBS>) -> Self {
        Self {
            callback_context: CallbackContext {
                resources: resources as *mut _,
            },
            registration: None,
        }
    }

    pub unsafe fn active_context() -> *mut CallbackContext<EVENTS, RX, SUBS> {
        ACTIVE_CALLBACK_CONTEXT.load(Ordering::Acquire) as *mut CallbackContext<EVENTS, RX, SUBS>
    }
}

impl<const EVENTS: usize, const RX: usize, const SUBS: usize> WifiBackend
    for Esp8266TargetBackend<EVENTS, RX, SUBS>
{
    fn initialize(&mut self, _config: &InitConfig) -> Result<(), InitError> {
        crate::wifi::init_compat_scheduler();
        self.registration = Some(CallbackRegistration::install(&mut self.callback_context)?);
        Ok(())
    }

    fn set_mode(&mut self, _mode: RawMode) -> Result<(), DriverError> {
        Ok(())
    }

    fn start(&mut self) -> Result<(), DriverError> {
        Ok(())
    }

    fn stop(&mut self) -> Result<(), DriverError> {
        Ok(())
    }

    fn connect(&mut self) -> Result<(), ConnectError> {
        Ok(())
    }

    fn disconnect(&mut self) -> Result<(), DisconnectError> {
        Ok(())
    }

    fn now_ms(&self) -> u64 {
        0
    }

    fn spin_once(&mut self) {
        core::hint::spin_loop();
    }
}

pub(crate) unsafe extern "C" fn publish_sta_connected<const EVENTS: usize, const RX: usize, const SUBS: usize>() {
    let context = ACTIVE_CALLBACK_CONTEXT.load(Ordering::Acquire) as *mut CallbackContext<EVENTS, RX, SUBS>;
    if context.is_null() {
        return;
    }
    let resources = unsafe { &mut *(*context).resources };
    let _ = resources.events.publish(WifiEvent::StaConnected);
}

pub(crate) unsafe extern "C" fn push_rx_record<const EVENTS: usize, const RX: usize, const SUBS: usize>(
    buffer: *mut core::ffi::c_void,
    len: u16,
    eb: *mut core::ffi::c_void,
) {
    let context = ACTIVE_CALLBACK_CONTEXT.load(Ordering::Acquire) as *mut CallbackContext<EVENTS, RX, SUBS>;
    if context.is_null() {
        return;
    }
    let resources = unsafe { &mut *(*context).resources };
    let _ = resources.rx.try_push(unsafe { RawRxRecord::from_raw(buffer, len, eb) });
}
```

- [ ] **Step 2: Export the target backend only when `wifi` is enabled**

Modify `src/wifi/core/mod.rs`:

```rust
mod backend;
mod config;
mod driver;
mod error;
mod event;
mod packet;
mod resources;
mod state;

#[cfg(feature = "wifi")]
mod target;

pub use backend::{Deadline, RawMode, WifiBackend};
pub use config::{AccessPointConfig, Channel, ClientConfig, Credential, InitConfig, MixedConfig, Passphrase, Ssid};
pub use driver::WifiDriver;
pub use error::{ConfigError, ConnectError, DisconnectError, DriverError, InitError};
pub use event::{EventBroker, EventCursor, WifiEvent};
pub use packet::{RawRxRecord, ReleaseToken, RxRing};
pub use resources::WifiResources;
pub use state::{
    ApMode, Configured, HasAp, HasSta, Initialized, MixedMode, StaMode, Started, Uninitialized,
};

#[cfg(feature = "wifi")]
pub use target::Esp8266TargetBackend;
```

- [ ] **Step 3: Add an xtensa compile-only check**

Run:

```bash
cargo check --target xtensa-esp8266-none-elf -Zbuild-std=core --no-default-features --features wifi-core,wifi,wifi-sta --lib
```

Expected: PASS, or fail only on target-backend implementation details inside `src/wifi/core/target.rs`. No host-native steps should be touched here.

- [ ] **Step 4: Commit**

```bash
git add src/wifi/core/target.rs src/wifi/core/mod.rs
git commit -m "feat: add esp8266 wifi core target backend shell"
```

### Task 9: Add `StaHandle`, Blocking Connect, The Example, Docs, And CI

**Files:**
- Create: `src/wifi/core/interface.rs`
- Modify: `src/wifi/core/driver.rs`
- Modify: `src/wifi/core/mod.rs`
- Create: `examples/wifi_core_connect.rs`
- Modify: `Cargo.toml`
- Modify: `research.md`
- Modify: `.github/workflows/ci.yaml`
- Test: `src/wifi/core/interface.rs`

- [ ] **Step 1: Add the failing station-handle test**

Create `src/wifi/core/interface.rs` with only this test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::wifi_core::{
        ClientConfig, Credential, InitConfig, Ssid, WifiBackend, WifiDriver, WifiResources,
    };

    #[derive(Default)]
    struct FakeBackend {
        connect_called: bool,
    }

    impl WifiBackend for FakeBackend {
        fn initialize(&mut self, _config: &InitConfig) -> Result<(), crate::wifi_core::InitError> { Ok(()) }
        fn set_mode(&mut self, _mode: crate::wifi_core::RawMode) -> Result<(), crate::wifi_core::DriverError> { Ok(()) }
        fn start(&mut self) -> Result<(), crate::wifi_core::DriverError> { Ok(()) }
        fn stop(&mut self) -> Result<(), crate::wifi_core::DriverError> { Ok(()) }
        fn connect(&mut self) -> Result<(), crate::wifi_core::ConnectError> { self.connect_called = true; Ok(()) }
        fn disconnect(&mut self) -> Result<(), crate::wifi_core::DisconnectError> { Ok(()) }
        fn now_ms(&self) -> u64 { 0 }
        fn spin_once(&mut self) {}
    }

    #[test]
    fn sta_handle_invokes_backend_connect() {
        let mut resources = WifiResources::<8, 4, 2>::new();
        let driver = WifiDriver::with_backend(&mut resources, FakeBackend::default());
        let driver = driver.initialize(InitConfig::default()).unwrap();
        let mut driver = driver
            .configure_sta(ClientConfig::new(Ssid::try_from("core-net").unwrap(), Credential::Open))
            .unwrap()
            .start()
            .unwrap();

        let mut sta = driver.sta().unwrap();
        sta.connect().unwrap();
    }
}
```

- [ ] **Step 2: Run the station-handle test and verify it fails**

Run:

```bash
cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --lib sta_handle_invokes_backend_connect -- --exact
```

Expected: FAIL because `StaHandle` and `WifiDriver::sta()` do not exist yet.

- [ ] **Step 3: Implement borrowed station handles without global event state**

Create `src/wifi/core/interface.rs`:

```rust
use super::{ConnectError, DisconnectError, DriverError, EventBroker, EventCursor, WifiBackend, WifiEvent};

pub struct StaSession {
    connected: bool,
}

impl StaSession {
    pub const fn is_connected(&self) -> bool {
        self.connected
    }
}

pub struct StaHandle<'a, B, const EVENTS: usize, const SUBS: usize> {
    pub(crate) backend: &'a mut B,
    pub(crate) events: &'a mut EventBroker<EVENTS, SUBS>,
    pub(crate) cursor: EventCursor,
}

impl<'a, B: WifiBackend, const EVENTS: usize, const SUBS: usize> StaHandle<'a, B, EVENTS, SUBS> {
    pub fn connect(&mut self) -> Result<(), ConnectError> {
        self.backend.connect()
    }

    pub fn connect_blocking(&mut self, timeout_ms: u32) -> Result<StaSession, DriverError> {
        self.connect().map_err(|_| DriverError::BackendRejected)?;
        let _ = self
            .events
            .wait_blocking(&mut self.cursor, self.backend, WifiEvent::StaConnected, timeout_ms)?;
        Ok(StaSession { connected: true })
    }

    pub async fn connect_async(&mut self) -> Result<StaSession, ConnectError> {
        self.connect()?;
        let _ = self
            .events
            .wait_async(&mut self.cursor, self.backend, WifiEvent::StaConnected)
            .await;
        Ok(StaSession { connected: true })
    }

    pub fn disconnect(&mut self) -> Result<(), DisconnectError> {
        self.backend.disconnect()
    }
}

impl<B, const EVENTS: usize, const SUBS: usize> Drop for StaHandle<'_, B, EVENTS, SUBS> {
    fn drop(&mut self) {
        self.events.release_cursor(self.cursor);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wifi_core::{
        ClientConfig, Credential, InitConfig, RawMode, Ssid, WifiBackend, WifiDriver, WifiResources,
    };

    #[derive(Default)]
    struct FakeBackend {
        connect_called: bool,
    }

    impl WifiBackend for FakeBackend {
        fn initialize(&mut self, _config: &InitConfig) -> Result<(), crate::wifi_core::InitError> { Ok(()) }
        fn set_mode(&mut self, _mode: RawMode) -> Result<(), crate::wifi_core::DriverError> { Ok(()) }
        fn start(&mut self) -> Result<(), crate::wifi_core::DriverError> { Ok(()) }
        fn stop(&mut self) -> Result<(), crate::wifi_core::DriverError> { Ok(()) }
        fn connect(&mut self) -> Result<(), crate::wifi_core::ConnectError> { self.connect_called = true; Ok(()) }
        fn disconnect(&mut self) -> Result<(), crate::wifi_core::DisconnectError> { Ok(()) }
        fn now_ms(&self) -> u64 { 0 }
        fn spin_once(&mut self) {}
    }

    #[test]
    fn sta_handle_invokes_backend_connect() {
        let mut resources = WifiResources::<8, 4, 2>::new();
        let driver = WifiDriver::with_backend(&mut resources, FakeBackend::default());
        let driver = driver.initialize(InitConfig::default()).unwrap();
        let mut driver = driver
            .configure_sta(ClientConfig::new(Ssid::try_from("core-net").unwrap(), Credential::Open))
            .unwrap()
            .start()
            .unwrap();

        let mut sta = driver.sta().unwrap();
        sta.connect().unwrap();
    }
}
```

Extend `src/wifi/core/driver.rs` with:

```rust
use super::interface::StaHandle;

impl<'d, B: WifiBackend, const EVENTS: usize, const RX: usize, const SUBS: usize>
    WifiDriver<'d, Started<StaMode>, B, EVENTS, RX, SUBS>
{
    pub fn sta(&mut self) -> Result<StaHandle<'_, B, EVENTS, SUBS>, DriverError> {
        let cursor = self.resources.events.claim_cursor()?;
        Ok(StaHandle {
            backend: &mut self.backend,
            events: &mut self.resources.events,
            cursor,
        })
    }
}

impl<'d, B, const EVENTS: usize, const RX: usize, const SUBS: usize>
    WifiDriver<'d, Started<StaMode>, B, EVENTS, RX, SUBS>
{
    pub fn resources(&self) -> &WifiResources<EVENTS, RX, SUBS> {
        self.resources
    }
}
```

Modify `src/wifi/core/mod.rs`:

```rust
mod backend;
mod config;
mod driver;
mod error;
mod event;
mod interface;
mod packet;
mod resources;
mod state;

#[cfg(feature = "wifi")]
mod target;

pub use backend::{Deadline, RawMode, WifiBackend};
pub use config::{AccessPointConfig, Channel, ClientConfig, Credential, InitConfig, MixedConfig, Passphrase, Ssid};
pub use driver::WifiDriver;
pub use error::{ConfigError, ConnectError, DisconnectError, DriverError, InitError};
pub use event::{EventBroker, EventCursor, WifiEvent};
pub use interface::{StaHandle, StaSession};
pub use packet::{RawRxRecord, ReleaseToken, RxRing};
pub use resources::WifiResources;
pub use state::{
    ApMode, Configured, HasAp, HasSta, Initialized, MixedMode, StaMode, Started, Uninitialized,
};

#[cfg(feature = "wifi")]
pub use target::Esp8266TargetBackend;
```

- [ ] **Step 4: Add the new example stanza**

Append to `Cargo.toml`:

```toml
[[example]]
name = "wifi_core_connect"
required-features = ["wifi-core", "wifi", "wifi-sta"]
```

- [ ] **Step 5: Add the minimal blocking connect example**

Create `examples/wifi_core_connect.rs`:

```rust
#![no_std]
#![no_main]

use esp8266_hal::println;
use esp8266_hal::wifi_core::{
    ClientConfig, Credential, Esp8266TargetBackend, InitConfig, Ssid, WifiDriver, WifiResources,
};
use static_cell::StaticCell;

const WIFI_SSID: &str = include_str!("ssid");

static RESOURCES: StaticCell<WifiResources<8, 4, 2>> = StaticCell::new();

#[esp8266_hal::main]
fn main() {
    let resources = RESOURCES.init(WifiResources::new());
    let backend = Esp8266TargetBackend::new(resources);
    let driver = WifiDriver::with_backend(resources, backend);
    let driver = driver.initialize(InitConfig::default()).expect("init");
    let config = ClientConfig::new(
        Ssid::try_from(WIFI_SSID.trim()).expect("ssid"),
        Credential::Open,
    );
    let mut driver = driver.configure_sta(config).expect("configure").start().expect("start");
    let mut sta = driver.sta().expect("sta");
    let _session = sta.connect_blocking(5_000).expect("connect");
    println!("wifi core connected");
    loop {
        core::hint::spin_loop();
    }
}
```

- [ ] **Step 6: Update `research.md`**

Append this section to `research.md`:

```markdown
## Core API Redesign Track

- Date started: 2026-04-22
- Public bring-up surface: `esp8266_hal::wifi_core`
- Host-native verification rule: core tests run with `--features wifi-core` and do not build `esp8266-wifi-sys`
- Target callback rule: one backend-local callback pointer slot may exist, but the pointee must live inside caller-owned `WifiResources`
- Legacy WiFi APIs remain temporary and are not the design center for this milestone
```

- [ ] **Step 7: Update CI**

Replace `.github/workflows/ci.yaml` with:

```yaml
on:
  pull_request:
    branches:
      - main
  push:
  workflow_dispatch:

name: CI

env:
  CARGO_TERM_COLOR: always

jobs:
  test-host-core:
    name: Host core tests
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: Swatinem/rust-cache@v2
      - run: cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --lib --tests

  check-wifi-core-example:
    name: Check wifi core example
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: esp-rs/xtensa-toolchain@v1.1
        with:
          default: true
          ldproxy: false
      - uses: Swatinem/rust-cache@v2
      - run: cargo check --example wifi_core_connect --target xtensa-esp8266-none-elf -Zbuild-std=core --no-default-features --features wifi-core,wifi,wifi-sta

  check-legacy-examples:
    name: Check legacy examples
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: esp-rs/xtensa-toolchain@v1.1
        with:
          default: true
          ldproxy: false
      - uses: Swatinem/rust-cache@v2
      - run: cargo check --examples --target=xtensa-esp8266-none-elf -Zbuild-std=core
```

- [ ] **Step 8: Run the final verification commands**

Run:

```bash
cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --lib --tests
cargo check --example wifi_core_connect --target xtensa-esp8266-none-elf -Zbuild-std=core --no-default-features --features wifi-core,wifi,wifi-sta
```

Expected:

- host-native core tests PASS without building `esp8266-wifi-sys`
- xtensa example check PASS with the target backend enabled

- [ ] **Step 9: Commit**

```bash
git add src/wifi/core/interface.rs src/wifi/core/driver.rs src/wifi/core/mod.rs Cargo.toml examples/wifi_core_connect.rs research.md .github/workflows/ci.yaml
git commit -m "feat: add wifi core station handle and example"
```

## Self-Review Checklist

- Spec coverage:
  - strong typestate: Tasks 2 and 5
  - caller-owned storage: Tasks 2, 6, and 7
  - runtime-neutral core and host testability: Tasks 1, 4, and 5
  - bounded events and packets: Tasks 6 and 7
  - explicit callback ownership model: Task 4 and Task 8
  - target backend only after pure core exists: Task order enforces this
  - no `static mut` broker/ring: all handle and broker tasks borrow from `WifiResources`
  - minimal blocking station example: Task 9
- Review feedback integration:
  - removed checked-in `bindings.rs` snapshot approach
  - removed `static mut EventBroker` approach
  - reordered lifecycle before event/packet integration
  - added callback-ownership note before target backend work
  - kept legacy validation explicit and temporary instead of making it the design center
- Placeholder scan:
  - no `TODO`
  - no `TBD`
  - no “implement later”
- Type consistency:
  - `WifiResources`, `WifiDriver`, `InitConfig`, `ClientConfig`, `AccessPointConfig`, `MixedConfig`, `StaMode`, `ApMode`, `MixedMode`, `EventBroker`, `EventCursor`, `StaHandle`, and `StaSession` are named consistently across tasks

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-04-22-wifi-core-api-redesign.md`. When you're ready, use `superpowers:executing-plans` to implement it task-by-task in the current tree.
