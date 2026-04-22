# WiFi Adapter Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task in the current tree. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate the ESP8266 packet ingress and link-state networking path onto `wifi_core` so `src/wifi/device.rs`, `src/wifi/stack.rs`, `src/wifi/embassy_net.rs`, and `examples/tcp_server.rs` consume a typed `StaLink` backed by caller-owned `WifiResources` instead of legacy global RX/event state.

**Architecture:** Extend `wifi_core` with callback-safe waker slots, explicit link-state tracking, real RX release tokens, and a borrowed `StaLink` capability. Route SDK WiFi events and RX callbacks through `src/wifi/core/target.rs`, then rewrite the `smoltcp` and `embassy-net` adapters as thin layers over that core surface while keeping legacy `src/wifi/event.rs` as a compatibility island for unmigrated APIs.

**Tech Stack:** Rust `no_std`, `heapless`, `portable-atomic`, `critical-section`, `smoltcp`, `embassy-net-driver-channel`, existing `esp8266-wifi-sys` FFI, host tests via `cargo test`, Xtensa checks via `cargo check`.

---

## File Structure

### New Files

- `src/wifi/core/waker.rs` — callback-safe, critical-section-protected waker slot shared by the core event and packet stores.
- `tests/wifi_core_link.rs` — host-native tests for callback-safe waking, link-state transitions, RX overflow release, and public diagnostics.

### Modified Files

- `Cargo.toml` — add `critical-section` to the `wifi-core` feature so callback-safe wake slots compile on host and target.
- `src/wifi/core/mod.rs` — export the new `StaLink`, `LinkState`, `WifiEventKind`, and helper modules.
- `src/wifi/core/error.rs` — add a retryable TX error for SDK queue saturation.
- `src/wifi/core/event.rs` — track current link state, last disconnect reason, explicit overflow counts, callback-safe link wake registration, and payload-bearing disconnect events.
- `src/wifi/core/packet.rs` — give `ReleaseToken` a real release function, add callback-safe RX wake registration, and surface copy-and-release helpers.
- `src/wifi/core/resources.rs` — add public enqueue/dequeue/publish/diagnostic/waker methods that adapters and target callbacks can consume.
- `src/wifi/core/backend.rs` — remove backend-owned async wake registration and add `transmit` plus station MAC accessors.
- `src/wifi/core/interface.rs` — add `StaLink` and update `StaHandle` async waits to use resource-owned wake registration.
- `src/wifi/core/driver.rs` — add `sta_link()` on started STA-capable drivers.
- `src/wifi/core/target.rs` — route SDK WiFi events and RX callbacks into `WifiResources`, bind/unbind callbacks during lifecycle transitions, and implement TX/MAC accessors.
- `src/wifi/device.rs` — rewrite as a thin `smoltcp::phy::Device` adapter over `StaLink`.
- `src/wifi/stack.rs` — stop asking callers for a separate MAC address and consume the rewritten `WifiDevice`.
- `src/wifi/embassy_net.rs` — rewrite the runner over `StaLink` and core link state instead of legacy `event::EVENT_BUS`.
- `src/wifi/mod.rs` — update docs and re-exports for the rewritten adapter construction flow.
- `examples/tcp_server.rs` — migrate the validated embassy TCP path onto `wifi_core` + `StaLink`.
- `research.md` — record that packet/link networking now flows through `wifi_core`.
- `.github/workflows/ci.yaml` — add the new host test target and keep the Xtensa checks on `wifi_core_connect` and `tcp_server`.

### Test Locations

- `tests/wifi_core_link.rs`
- `src/wifi/core/interface.rs`
- `src/wifi/core/event.rs`
- `src/wifi/core/packet.rs`

### Design Rules For Implementers

- `wifi_core` remains host-testable under `--no-default-features --features wifi-core`.
- `wifi_core` must not depend on `esp8266-wifi-sys`; target-only release logic must enter through function pointers or target modules.
- The only global callback state allowed in the migrated path is the existing backend-local callback slot in `src/wifi/core/target.rs`.
- RX overflow must release packet ownership immediately and increment a visible drop counter.
- Event overflow must increment a visible overflow counter and never silently drop state except where explicitly documented.
- Async waits must use resource-owned wakers, not backend-private wake registration.
- The embassy runner and `smoltcp` device must read link truth from `wifi_core`, not from `src/wifi/event.rs`.

### Task 1: Add Callback-Safe Wake Slots And Public Resource Diagnostics

**Files:**
- Modify: `Cargo.toml`
- Create: `src/wifi/core/waker.rs`
- Modify: `src/wifi/core/mod.rs`
- Modify: `src/wifi/core/error.rs`
- Modify: `src/wifi/core/event.rs`
- Modify: `src/wifi/core/packet.rs`
- Modify: `src/wifi/core/resources.rs`
- Create: `tests/wifi_core_link.rs`

- [ ] **Step 1: Write the failing host test**

Create `tests/wifi_core_link.rs`:

```rust
use core::ffi::c_void;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::task::{RawWaker, RawWakerVTable, Waker};

use esp8266_hal::wifi_core::{DriverError, LinkState, RawRxRecord, WifiEvent, WifiResources};

static LINK_WAKES: AtomicUsize = AtomicUsize::new(0);
static RX_WAKES: AtomicUsize = AtomicUsize::new(0);
static RELEASES: AtomicUsize = AtomicUsize::new(0);

unsafe fn clone_counter(data: *const ()) -> RawWaker {
    RawWaker::new(data, &VTABLE)
}

unsafe fn wake_counter(data: *const ()) {
    let counter = &*(data as *const AtomicUsize);
    counter.fetch_add(1, Ordering::SeqCst);
}

unsafe fn wake_counter_by_ref(data: *const ()) {
    wake_counter(data);
}

unsafe fn drop_counter(_: *const ()) {}

static VTABLE: RawWakerVTable =
    RawWakerVTable::new(clone_counter, wake_counter, wake_counter_by_ref, drop_counter);

fn counter_waker(counter: &'static AtomicUsize) -> Waker {
    let raw = RawWaker::new(counter as *const AtomicUsize as *const (), &VTABLE);
    unsafe { Waker::from_raw(raw) }
}

unsafe fn count_release(_: *mut c_void, _: *mut c_void) {
    RELEASES.fetch_add(1, Ordering::SeqCst);
}

fn fake_record(len: u16) -> RawRxRecord {
    unsafe { RawRxRecord::from_raw(core::ptr::null_mut(), len, core::ptr::null_mut(), Some(count_release)) }
}

#[test]
fn publishing_link_events_updates_state_and_wakes_waiters() {
    LINK_WAKES.store(0, Ordering::SeqCst);

    let mut resources = WifiResources::<4, 2, 1>::new();
    let waker = counter_waker(&LINK_WAKES);
    resources.register_link_waker(&waker);

    assert_eq!(resources.link_state(), LinkState::Down);
    assert_eq!(resources.last_disconnect_reason(), None);

    resources.publish_event(WifiEvent::StaConnected).unwrap();
    assert_eq!(resources.link_state(), LinkState::Up);
    assert_eq!(LINK_WAKES.load(Ordering::SeqCst), 1);

    resources
        .publish_event(WifiEvent::StaDisconnected { reason: Some(7) })
        .unwrap();
    assert_eq!(resources.link_state(), LinkState::Down);
    assert_eq!(resources.last_disconnect_reason(), Some(7));
    assert_eq!(LINK_WAKES.load(Ordering::SeqCst), 2);
}

#[test]
fn rx_enqueue_wakes_waiters_and_overflow_releases_immediately() {
    RX_WAKES.store(0, Ordering::SeqCst);
    RELEASES.store(0, Ordering::SeqCst);

    let mut resources = WifiResources::<4, 1, 1>::new();
    let waker = counter_waker(&RX_WAKES);
    resources.register_rx_waker(&waker);

    assert!(resources.enqueue_rx(fake_record(32)).is_ok());
    assert_eq!(resources.queued_rx(), 1);
    assert_eq!(RX_WAKES.load(Ordering::SeqCst), 1);

    assert_eq!(
        resources.enqueue_rx(fake_record(64)),
        Err(DriverError::PacketOverflow)
    );
    assert_eq!(resources.rx_drops(), 1);
    assert_eq!(RELEASES.load(Ordering::SeqCst), 1);
}

#[test]
fn event_overflow_is_counted() {
    let mut resources = WifiResources::<1, 1, 1>::new();
    let _cursor = resources.claim_event_cursor().unwrap();

    resources.publish_event(WifiEvent::StaStarted).unwrap();
    assert_eq!(
        resources.publish_event(WifiEvent::StaConnected),
        Err(DriverError::EventOverflow)
    );
    assert_eq!(resources.event_overflows(), 1);
}
```

- [ ] **Step 2: Run the test and verify it fails**

Run:

```bash
cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --test wifi_core_link -q
```

Expected: FAIL with unresolved imports or missing methods such as `LinkState`, `register_link_waker`, `enqueue_rx`, `claim_event_cursor`, `publish_event`, or the new `RawRxRecord::from_raw` signature.

- [ ] **Step 3: Implement callback-safe wake slots, event state, packet release, and resource accessors**

Update `Cargo.toml` so `wifi-core` includes `critical-section`:

```toml
wifi-core = [
  "dep:portable-atomic",
  "dep:heapless",
  "dep:critical-section",
]
```

Create `src/wifi/core/waker.rs`:

```rust
use core::cell::UnsafeCell;
use core::task::Waker;

pub(crate) struct CallbackWaker {
    slot: UnsafeCell<Option<Waker>>,
}

unsafe impl Send for CallbackWaker {}
unsafe impl Sync for CallbackWaker {}

impl CallbackWaker {
    pub const fn new() -> Self {
        Self {
            slot: UnsafeCell::new(None),
        }
    }

    pub fn register(&self, waker: &Waker) {
        critical_section::with(|_| {
            let slot = unsafe { &mut *self.slot.get() };
            match slot {
                Some(existing) if existing.will_wake(waker) => {}
                _ => *slot = Some(waker.clone()),
            }
        });
    }

    pub fn wake(&self) {
        let waker = critical_section::with(|_| unsafe { (*self.slot.get()).take() });
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}
```

Update `src/wifi/core/error.rs`:

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DriverError {
    Timeout,
    EventOverflow,
    PacketOverflow,
    WouldBlock,
    BackendRejected,
}
```

Update `src/wifi/core/event.rs` so the event store owns current link state, disconnect reason, overflow accounting, and a link-state waker:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkState {
    Down,
    Up,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WifiEvent {
    DriverReady,
    StaStarted,
    StaStopped,
    StaConnected,
    StaDisconnected { reason: Option<u8> },
    ApStarted,
    ApStopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WifiEventKind {
    DriverReady,
    StaStarted,
    StaStopped,
    StaConnected,
    StaDisconnected,
    ApStarted,
    ApStopped,
}

pub struct EventBroker<const EVENTS: usize, const SUBS: usize> {
    events: Vec<WifiEvent, EVENTS>,
    claimed: [bool; SUBS],
    positions: [u64; SUBS],
    first_sequence: u64,
    next_sequence: u64,
    link_state: LinkState,
    last_disconnect_reason: Option<u8>,
    overflow_count: u32,
    link_waker: CallbackWaker,
}
```

Add methods in `src/wifi/core/event.rs`:

```rust
impl WifiEvent {
    pub const fn kind(self) -> WifiEventKind {
        match self {
            Self::DriverReady => WifiEventKind::DriverReady,
            Self::StaStarted => WifiEventKind::StaStarted,
            Self::StaStopped => WifiEventKind::StaStopped,
            Self::StaConnected => WifiEventKind::StaConnected,
            Self::StaDisconnected { .. } => WifiEventKind::StaDisconnected,
            Self::ApStarted => WifiEventKind::ApStarted,
            Self::ApStopped => WifiEventKind::ApStopped,
        }
    }
}

impl<const EVENTS: usize, const SUBS: usize> EventBroker<EVENTS, SUBS> {
    pub fn register_link_waker(&self, waker: &Waker) {
        self.link_waker.register(waker);
    }

    pub const fn link_state(&self) -> LinkState {
        self.link_state
    }

    pub const fn last_disconnect_reason(&self) -> Option<u8> {
        self.last_disconnect_reason
    }

    pub const fn overflow_count(&self) -> u32 {
        self.overflow_count
    }
}
```

Inside `publish`, update state and count overflow explicitly:

```rust
pub fn publish(&mut self, event: WifiEvent) -> Result<(), DriverError> {
    match event {
        WifiEvent::StaConnected | WifiEvent::ApStarted => self.link_state = LinkState::Up,
        WifiEvent::StaDisconnected { reason } => {
            self.link_state = LinkState::Down;
            self.last_disconnect_reason = reason;
        }
        WifiEvent::StaStopped | WifiEvent::ApStopped => self.link_state = LinkState::Down,
        _ => {}
    }

    if !self.has_claimed_cursors() {
        self.link_waker.wake();
        self.clear();
        return Ok(());
    }

    self.compact();
    if self.events.push(event).is_err() {
        self.overflow_count = self.overflow_count.wrapping_add(1);
        self.link_waker.wake();
        return Err(DriverError::EventOverflow);
    }

    self.next_sequence += 1;
    self.link_waker.wake();
    Ok(())
}
```

Update `src/wifi/core/packet.rs` so `ReleaseToken` stores a real release function and `RxRing` owns an RX waker:

```rust
pub type ReleaseFn = unsafe fn(*mut c_void, *mut c_void);

#[derive(Clone, Copy)]
pub struct ReleaseToken {
    buffer: *mut c_void,
    eb: *mut c_void,
    release_fn: Option<ReleaseFn>,
}

impl ReleaseToken {
    pub const unsafe fn from_raw(
        buffer: *mut c_void,
        eb: *mut c_void,
        release_fn: Option<ReleaseFn>,
    ) -> Self {
        Self { buffer, eb, release_fn }
    }

    pub fn release(self) {
        if let Some(release_fn) = self.release_fn {
            unsafe { release_fn(self.buffer, self.eb) };
        }
    }
}

pub struct RxRing<const N: usize> {
    entries: [Option<RawRxRecord>; N],
    head: usize,
    tail: usize,
    len: usize,
    drops: u32,
    waker: CallbackWaker,
}
```

Add RX wake registration and copy-and-release helpers in `src/wifi/core/packet.rs`:

```rust
impl RawRxRecord {
    pub const unsafe fn from_raw(
        buffer: *mut c_void,
        len: u16,
        eb: *mut c_void,
        release_fn: Option<ReleaseFn>,
    ) -> Self {
        Self {
            buffer,
            len,
            release: ReleaseToken::from_raw(buffer, eb, release_fn),
        }
    }

    pub fn copy_into(self, dst: &mut [u8]) -> usize {
        let len = self.len as usize;
        let copy_len = len.min(dst.len());
        if copy_len > 0 && !self.buffer.is_null() {
            unsafe {
                core::ptr::copy_nonoverlapping(self.buffer as *const u8, dst.as_mut_ptr(), copy_len);
            }
        }
        self.release.release();
        copy_len
    }
}

impl<const N: usize> RxRing<N> {
    pub fn register_waker(&self, waker: &Waker) {
        self.waker.register(waker);
    }

    pub fn try_push(&mut self, record: RawRxRecord) -> Result<(), DriverError> {
        if N == 0 || self.len == N {
            self.drops = self.drops.wrapping_add(1);
            return Err(DriverError::PacketOverflow);
        }
        self.entries[self.head] = Some(record);
        self.head = (self.head + 1) % N;
        self.len += 1;
        self.waker.wake();
        Ok(())
    }
}
```

Update `src/wifi/core/resources.rs` so the new public API is the single access point for callback ingress and adapter diagnostics:

```rust
impl<const EVENTS: usize, const RX: usize, const SUBS: usize> WifiResources<EVENTS, RX, SUBS> {
    pub fn claim_event_cursor(&mut self) -> Result<EventCursor, DriverError> {
        self.events.claim_cursor()
    }

    pub fn publish_event(&mut self, event: WifiEvent) -> Result<(), DriverError> {
        self.events.publish(event)
    }

    pub fn register_link_waker(&self, waker: &Waker) {
        self.events.register_link_waker(waker);
    }

    pub fn link_state(&self) -> LinkState {
        self.events.link_state()
    }

    pub fn last_disconnect_reason(&self) -> Option<u8> {
        self.events.last_disconnect_reason()
    }

    pub fn event_overflows(&self) -> u32 {
        self.events.overflow_count()
    }

    pub fn enqueue_rx(&mut self, record: RawRxRecord) -> Result<(), DriverError> {
        match self.rx.try_push(record) {
            Ok(()) => Ok(()),
            Err(error) => {
                record.release.release();
                Err(error)
            }
        }
    }

    pub fn register_rx_waker(&self, waker: &Waker) {
        self.rx.register_waker(waker);
    }

    pub fn dequeue_rx(&mut self) -> Option<RawRxRecord> {
        self.rx.pop()
    }
}
```

Update `src/wifi/core/mod.rs` to wire everything together:

```rust
mod waker;

pub use event::{EventCursor, LinkState, WifiEvent, WifiEventKind};
```

- [ ] **Step 4: Run the host tests and verify they pass**

Run:

```bash
cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --test wifi_core_link -q
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml src/wifi/core/waker.rs src/wifi/core/mod.rs src/wifi/core/error.rs src/wifi/core/event.rs src/wifi/core/packet.rs src/wifi/core/resources.rs tests/wifi_core_link.rs
git commit -m "feat: add wifi core link wake and packet diagnostics"
```

### Task 2: Add `StaLink`, Core TX Hooks, And Resource-Owned Async Waiting

**Files:**
- Modify: `src/wifi/core/backend.rs`
- Modify: `src/wifi/core/interface.rs`
- Modify: `src/wifi/core/driver.rs`
- Modify: `src/wifi/core/event.rs`
- Modify: `src/wifi/core/mod.rs`

- [ ] **Step 1: Add the failing `StaLink` unit test**

Extend `src/wifi/core/interface.rs` with:

```rust
#[cfg(test)]
mod link_tests {
    use super::*;
    use crate::wifi_core::{
        ClientConfig, Credential, InitConfig, LinkState, RawMode, RawRxRecord, Ssid, WifiBackend,
        WifiDriver, WifiEvent, WifiResources,
    };

    #[derive(Default)]
    struct FakeBackend {
        transmitted: heapless::Vec<usize, 4>,
        mac: [u8; 6],
    }

    impl WifiBackend for FakeBackend {
        fn initialize(&mut self, _config: &InitConfig) -> Result<(), crate::wifi_core::InitError> {
            Ok(())
        }

        fn set_mode(&mut self, _mode: RawMode) -> Result<(), crate::wifi_core::DriverError> {
            Ok(())
        }

        fn start(&mut self) -> Result<(), crate::wifi_core::DriverError> {
            Ok(())
        }

        fn stop(&mut self) -> Result<(), crate::wifi_core::DriverError> {
            Ok(())
        }

        fn connect(&mut self) -> Result<(), crate::wifi_core::ConnectError> {
            Ok(())
        }

        fn disconnect(&mut self) -> Result<(), crate::wifi_core::DisconnectError> {
            Ok(())
        }

        fn transmit(&mut self, frame: &[u8]) -> Result<(), crate::wifi_core::DriverError> {
            self.transmitted.push(frame.len()).unwrap();
            Ok(())
        }

        fn sta_mac_address(&self) -> Result<[u8; 6], crate::wifi_core::DriverError> {
            Ok(self.mac)
        }

        fn now_ms(&self) -> u64 {
            0
        }

        fn spin_once(&mut self) {}
    }

    #[test]
    fn sta_link_reads_core_state_and_transmits_frames() {
        let mut resources = WifiResources::<8, 4, 2>::new();
        resources.publish_event(WifiEvent::StaConnected).unwrap();
        resources.enqueue_rx(unsafe {
            RawRxRecord::from_raw(core::ptr::null_mut(), 24, core::ptr::null_mut(), None)
        }).unwrap();

        let backend = FakeBackend {
            mac: [1, 2, 3, 4, 5, 6],
            ..Default::default()
        };

        let mut driver = WifiDriver::with_backend(&mut resources, backend)
            .initialize(InitConfig::default()).unwrap()
            .configure_sta(ClientConfig::new(
                Ssid::try_from("core-net").unwrap(),
                Credential::Open,
            )).unwrap()
            .start().unwrap();

        let mut link = driver.sta_link().unwrap();
        let mut rx_buf = [0u8; 32];

        assert_eq!(link.link_state(), LinkState::Up);
        assert_eq!(link.mac_address().unwrap(), [1, 2, 3, 4, 5, 6]);
        assert_eq!(link.receive(&mut rx_buf), Some(24));
        link.transmit(&[9, 8, 7]).unwrap();
    }
}
```

- [ ] **Step 2: Run the `interface.rs` tests and verify they fail**

Run:

```bash
cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core interface::link_tests::sta_link_reads_core_state_and_transmits_frames -q
```

Expected: FAIL with missing trait methods such as `transmit`, `sta_mac_address`, or the missing `sta_link()` / `StaLink` API.

- [ ] **Step 3: Implement `StaLink`, backend TX/MAC hooks, and resource-owned async waits**

Update `src/wifi/core/backend.rs`:

```rust
pub trait WifiBackend {
    fn initialize(&mut self, config: &InitConfig) -> Result<(), InitError>;
    fn configure_sta(&mut self, _config: &ClientConfig) -> Result<(), DriverError> { Ok(()) }
    fn configure_ap(&mut self, _config: &AccessPointConfig) -> Result<(), DriverError> { Ok(()) }
    fn configure_mixed(
        &mut self,
        client: &ClientConfig,
        access_point: &AccessPointConfig,
    ) -> Result<(), DriverError> {
        self.configure_sta(client)?;
        self.configure_ap(access_point)
    }
    fn set_mode(&mut self, mode: RawMode) -> Result<(), DriverError>;
    fn start(&mut self) -> Result<(), DriverError>;
    fn stop(&mut self) -> Result<(), DriverError>;
    fn connect(&mut self) -> Result<(), ConnectError>;
    fn disconnect(&mut self) -> Result<(), DisconnectError>;
    fn transmit(&mut self, _frame: &[u8]) -> Result<(), DriverError> {
        Err(DriverError::BackendRejected)
    }
    fn sta_mac_address(&self) -> Result<[u8; 6], DriverError> {
        Err(DriverError::BackendRejected)
    }
    fn connect_blocking(&mut self, _timeout_ms: u32) -> Option<Result<(), DriverError>> {
        None
    }
    fn now_ms(&self) -> u64;
    fn spin_once(&mut self);
}
```

Update `src/wifi/core/event.rs` so async waits use the event store’s own link waker instead of a backend hook:

```rust
pub async fn wait_async(
    &mut self,
    cursor: &mut EventCursor,
    wanted: WifiEventKind,
) -> WifiEvent {
    poll_fn(|cx| {
        self.register_link_waker(cx.waker());

        while let Some(event) = self.try_next(cursor) {
            if event.kind() == wanted {
                return Poll::Ready(event);
            }
        }

        Poll::Pending
    })
    .await
}
```

Add `StaLink` to `src/wifi/core/interface.rs`:

```rust
pub struct StaLink<'a, B, const EVENTS: usize, const RX: usize, const SUBS: usize> {
    pub(crate) backend: &'a mut B,
    pub(crate) resources: &'a mut WifiResources<EVENTS, RX, SUBS>,
}

impl<'a, B: WifiBackend, const EVENTS: usize, const RX: usize, const SUBS: usize>
    StaLink<'a, B, EVENTS, RX, SUBS>
{
    pub fn register_link_waker(&self, waker: &Waker) {
        self.resources.register_link_waker(waker);
    }

    pub fn register_rx_waker(&self, waker: &Waker) {
        self.resources.register_rx_waker(waker);
    }

    pub fn link_state(&self) -> LinkState {
        self.resources.link_state()
    }

    pub fn last_disconnect_reason(&self) -> Option<u8> {
        self.resources.last_disconnect_reason()
    }

    pub fn mac_address(&self) -> Result<[u8; 6], DriverError> {
        self.backend.sta_mac_address()
    }

    pub fn has_rx_data(&self) -> bool {
        self.resources.queued_rx() != 0
    }

    pub fn queued_rx(&self) -> usize {
        self.resources.queued_rx()
    }

    pub fn rx_drops(&self) -> u32 {
        self.resources.rx_drops()
    }

    pub fn receive(&mut self, dst: &mut [u8]) -> Option<usize> {
        self.resources.dequeue_rx().map(|record| record.copy_into(dst))
    }

    pub fn transmit(&mut self, frame: &[u8]) -> Result<(), DriverError> {
        self.backend.transmit(frame)
    }
}
```

Update `StaHandle` in `src/wifi/core/interface.rs`:

```rust
pub async fn connect_async(&mut self) -> Result<StaSession, ConnectError> {
    self.connect()?;
    let _ = self.events.wait_async(&mut self.cursor, WifiEventKind::StaConnected).await;
    Ok(StaSession { connected: true })
}
```

Update `src/wifi/core/driver.rs`:

```rust
pub fn sta_link(&mut self) -> Result<StaLink<'_, B, EVENTS, RX, SUBS>, DriverError> {
    Ok(StaLink {
        backend: &mut self.backend,
        resources: self.resources,
    })
}
```

Update `src/wifi/core/mod.rs`:

```rust
pub use interface::{StaHandle, StaLink, StaSession};
```

- [ ] **Step 4: Re-run the `StaLink` test and the full host core test set**

Run:

```bash
cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core interface::link_tests::sta_link_reads_core_state_and_transmits_frames -q
cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --lib --tests -q
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/wifi/core/backend.rs src/wifi/core/interface.rs src/wifi/core/driver.rs src/wifi/core/event.rs src/wifi/core/mod.rs
git commit -m "feat: add wifi core sta link surface"
```

### Task 3: Route ESP8266 Event And RX Callbacks Into `wifi_core`

**Files:**
- Modify: `src/wifi/core/target.rs`
- Modify: `examples/wifi_core_connect.rs`

- [ ] **Step 1: Write the failing target-surface compile change**

Update `examples/wifi_core_connect.rs` so the example exercises the new link-facing surface after a successful connection:

```rust
    let mut sta = driver.sta().unwrap();
    let _session = sta.connect_blocking(10_000).unwrap();
    drop(sta);

    let link = driver.sta_link().unwrap();
    let _mac = link.mac_address().unwrap();
    let _state = link.link_state();

    loop {
        unsafe { esp8266_hal::wifi::compat::task::vTaskDelay(100) };
    }
```

- [ ] **Step 2: Run the Xtensa example check and verify it fails**

Run:

```bash
source .envrc && cargo check --example wifi_core_connect --target xtensa-esp8266-none-elf -Zbuild-std=core --no-default-features --features interrupt,rt,wifi-core,wifi,wifi-sta
```

Expected: FAIL with missing `Esp8266TargetBackend` implementations for `transmit` or `sta_mac_address`, or with missing callback wiring needed by the new link-facing path.

- [ ] **Step 3: Implement target-side event and RX ingress**

Update `src/wifi/core/target.rs` so the callback context is the only ingress point for target-side packet and link state:

```rust
pub(crate) struct CallbackContext<const EVENTS: usize, const RX: usize, const SUBS: usize> {
    pub resources: *mut WifiResources<EVENTS, RX, SUBS>,
}

unsafe fn release_rx_ownership(buffer: *mut core::ffi::c_void, eb: *mut core::ffi::c_void) {
    if !eb.is_null() {
        esp8266_wifi_sys::esp_wifi_internal_free_rx_buffer(eb);
    } else if !buffer.is_null() {
        crate::wifi::compat::memory::free(buffer);
    }
}

unsafe extern "C" fn wifi_core_event_trampoline<const EVENTS: usize, const RX: usize, const SUBS: usize>(
    _arg: *mut core::ffi::c_void,
    _base: esp8266_wifi_sys::esp_event_base_t,
    event_id: i32,
    event_data: *mut core::ffi::c_void,
) {
    let context = Esp8266TargetBackend::<EVENTS, RX, SUBS>::active_context();
    if context.is_null() {
        return;
    }

    let resources = &mut *(*context).resources;
    let event = match event_id as u32 {
        esp8266_wifi_sys::WIFI_EVENT_STA_START => Some(WifiEvent::StaStarted),
        esp8266_wifi_sys::WIFI_EVENT_STA_STOP => Some(WifiEvent::StaStopped),
        esp8266_wifi_sys::WIFI_EVENT_STA_CONNECTED => Some(WifiEvent::StaConnected),
        esp8266_wifi_sys::WIFI_EVENT_STA_DISCONNECTED => {
            let reason = if event_data.is_null() {
                None
            } else {
                Some(*(event_data as *const u8).add(32 + 1 + 6))
            };
            Some(WifiEvent::StaDisconnected { reason })
        }
        esp8266_wifi_sys::WIFI_EVENT_AP_START => Some(WifiEvent::ApStarted),
        esp8266_wifi_sys::WIFI_EVENT_AP_STOP => Some(WifiEvent::ApStopped),
        _ => None,
    };

    if let Some(event) = event {
        let _ = resources.publish_event(event);
    }
}

unsafe extern "C" fn rx_callback_sta<const EVENTS: usize, const RX: usize, const SUBS: usize>(
    buffer: *mut core::ffi::c_void,
    len: u16,
    eb: *mut core::ffi::c_void,
) -> esp8266_wifi_sys::esp_err_t {
    let context = Esp8266TargetBackend::<EVENTS, RX, SUBS>::active_context();
    if context.is_null() {
        release_rx_ownership(buffer, eb);
        return esp8266_wifi_sys::ESP_OK;
    }

    let resources = &mut *(*context).resources;
    let record = RawRxRecord::from_raw(buffer, len, eb, Some(release_rx_ownership));
    let _ = resources.enqueue_rx(record);
    esp8266_wifi_sys::ESP_OK
}
```

Bind and unbind callbacks during lifecycle transitions in `src/wifi/core/target.rs`:

```rust
fn start(&mut self) -> Result<(), DriverError> {
    self.controller_mut()?.start().map_err(map_wifi_driver_error)?;

    crate::wifi::EspError::check(unsafe {
        esp8266_wifi_sys::esp_event_handler_register(
            esp8266_wifi_sys::WIFI_EVENT,
            -1,
            Some(wifi_core_event_trampoline::<EVENTS, RX, SUBS>),
            core::ptr::null_mut(),
        )
    }).map_err(|_| DriverError::BackendRejected)?;

    crate::wifi::EspError::check(unsafe {
        esp8266_wifi_sys::esp_wifi_internal_reg_rxcb(
            crate::wifi::controller::WifiInterface::Sta.to_raw(),
            Some(rx_callback_sta::<EVENTS, RX, SUBS>),
        )
    }).map_err(|_| DriverError::BackendRejected)?;

    Ok(())
}

fn stop(&mut self) -> Result<(), DriverError> {
    let _ = unsafe {
        esp8266_wifi_sys::esp_wifi_internal_reg_rxcb(
            crate::wifi::controller::WifiInterface::Sta.to_raw(),
            None,
        )
    };

    let _ = unsafe {
        esp8266_wifi_sys::esp_event_handler_unregister(
            esp8266_wifi_sys::WIFI_EVENT,
            -1,
            Some(wifi_core_event_trampoline::<EVENTS, RX, SUBS>),
        )
    };

    self.controller_mut()?.stop().map_err(map_wifi_driver_error)
}
```

Implement TX and MAC accessors in `src/wifi/core/target.rs`:

```rust
fn transmit(&mut self, frame: &[u8]) -> Result<(), DriverError> {
    let ret = unsafe {
        esp8266_wifi_sys::esp_wifi_internal_tx(
            crate::wifi::controller::WifiInterface::Sta.to_raw(),
            frame.as_ptr() as *mut core::ffi::c_void,
            frame.len() as u16,
        )
    };

    match ret {
        0 => Ok(()),
        -1 => Err(DriverError::WouldBlock),
        _ => Err(DriverError::BackendRejected),
    }
}

fn sta_mac_address(&self) -> Result<[u8; 6], DriverError> {
    self.controller()?
        .get_mac(crate::wifi::controller::WifiInterface::Sta)
        .map_err(map_wifi_driver_error)
}
```

- [ ] **Step 4: Re-run the Xtensa example check**

Run:

```bash
source .envrc && cargo check --example wifi_core_connect --target xtensa-esp8266-none-elf -Zbuild-std=core --no-default-features --features interrupt,rt,wifi-core,wifi,wifi-sta
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/wifi/core/target.rs examples/wifi_core_connect.rs
git commit -m "feat: route esp8266 wifi callbacks into wifi core"
```

### Task 4: Rewrite `src/wifi/device.rs` And `src/wifi/stack.rs` Over `StaLink`

**Files:**
- Modify: `src/wifi/device.rs`
- Modify: `src/wifi/stack.rs`
- Modify: `src/wifi/mod.rs`

- [ ] **Step 1: Write the failing adapter construction change**

Rewrite the top of `src/wifi/stack.rs` so the stack constructor no longer takes a separate MAC address:

```rust
pub struct WifiStack<'a, B, const EVENTS: usize, const RX: usize, const SUBS: usize> {
    device: WifiDevice<'a, B, EVENTS, RX, SUBS>,
    iface: Interface,
    sockets: SocketSet<'a>,
    dhcp_handle: SocketHandle,
}

impl<'a, B: WifiBackend, const EVENTS: usize, const RX: usize, const SUBS: usize>
    WifiStack<'a, B, EVENTS, RX, SUBS>
{
    pub fn new(
        mut device: WifiDevice<'a, B, EVENTS, RX, SUBS>,
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
}
```

Run the existing Xtensa embassy example check before implementing the adapter rewrite:

```bash
source .envrc && cargo check --example tcp_server --target xtensa-esp8266-none-elf -Zbuild-std=core --no-default-features --features interrupt,rt,wifi-embassy-net,wifi-sta
```

Expected: FAIL with mismatched `WifiDevice::new` or `WifiStack::new` signatures.

- [ ] **Step 2: Rewrite `src/wifi/device.rs` as a thin `smoltcp` adapter over `StaLink`**

Replace the static-ring/state-owner shape in `src/wifi/device.rs` with:

```rust
pub struct WifiDevice<'d, B, const EVENTS: usize, const RX: usize, const SUBS: usize> {
    link: StaLink<'d, B, EVENTS, RX, SUBS>,
}

impl<'d, B: WifiBackend, const EVENTS: usize, const RX: usize, const SUBS: usize>
    WifiDevice<'d, B, EVENTS, RX, SUBS>
{
    pub fn new(link: StaLink<'d, B, EVENTS, RX, SUBS>) -> Self {
        Self { link }
    }

    pub fn mac_address(&self) -> Result<[u8; 6], DriverError> {
        self.link.mac_address()
    }

    pub fn register_waker(&self, waker: &Waker) {
        self.link.register_rx_waker(waker);
    }

    pub fn has_rx_data(&self) -> bool {
        self.link.has_rx_data()
    }

    pub fn rx_drops(&self) -> u32 {
        self.link.rx_drops()
    }

    pub(crate) fn pop_frame(&mut self, buf: &mut [u8]) -> Option<usize> {
        self.link.receive(buf)
    }
}
```

Keep the `smoltcp::phy::Device` implementation but move all receive/transmit calls through `StaLink`:

```rust
fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
    if !self.link.has_rx_data() {
        return None;
    }

    Some((
        WifiRxToken { device: self },
        WifiTxToken { device: self },
    ))
}

fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
    Some(WifiTxToken { device: self })
}
```

In the TX token, treat `DriverError::WouldBlock` as a temporary backoff and surface other errors as dropped transmissions:

```rust
let result = f(frame);

match self.device.link.transmit(frame) {
    Ok(()) => {}
    Err(DriverError::WouldBlock) => {
        TX_BACKOFF.store(true, Ordering::Relaxed);
    }
    Err(_) => {}
}

result
```

- [ ] **Step 3: Finish the `WifiStack` rewrite and update public docs**

Update `src/wifi/stack.rs`:

```rust
let mut sockets = SocketSet::new(socket_storage);
let dhcp_socket = dhcpv4::Socket::new();
let dhcp_handle = sockets.add(dhcp_socket);

Ok(Self {
    device,
    iface,
    sockets,
    dhcp_handle,
})
```

Update `src/wifi/mod.rs` docs so the public examples stop instructing callers to register RX callbacks manually:

```rust
//! Packet adapters now borrow typed link handles from `esp8266_hal::wifi_core`.
//! `src/wifi/device.rs` and `src/wifi/stack.rs` are adapter layers, not state owners.
```

- [ ] **Step 4: Re-run the Xtensa `tcp_server` check**

Run:

```bash
source .envrc && cargo check --example tcp_server --target xtensa-esp8266-none-elf -Zbuild-std=core --no-default-features --features interrupt,rt,wifi-embassy-net,wifi-sta
```

Expected: FAIL, but now only in `src/wifi/embassy_net.rs` and `examples/tcp_server.rs` because they still construct the old `WifiDevice` / event subscription flow.

- [ ] **Step 5: Commit**

```bash
git add src/wifi/device.rs src/wifi/stack.rs src/wifi/mod.rs
git commit -m "refactor: rewrite wifi device over sta link"
```

### Task 5: Rewrite `src/wifi/embassy_net.rs` And Migrate `examples/tcp_server.rs`

**Files:**
- Modify: `src/wifi/embassy_net.rs`
- Modify: `examples/tcp_server.rs`

- [ ] **Step 1: Write the failing example migration**

Update `examples/tcp_server.rs` so it switches from `WifiController` + `WifiDevice::new(WifiInterface::Sta)` to `wifi_core` typestate + `StaLink`:

```rust
use esp8266_hal::wifi_core::{
    ClientConfig, Credential, Esp8266TargetBackend, InitConfig, Passphrase, Ssid, WifiDriver,
    WifiResources,
};

type WifiStaDriver = WifiDriver<
    'static,
    esp8266_hal::wifi_core::Started<esp8266_hal::wifi_core::StaMode>,
    Esp8266TargetBackend<8, 4, 2>,
    8,
    4,
    2,
>;

static WIFI_RESOURCES: StaticCell<WifiResources<8, 4, 2>> = StaticCell::new();

fn init_wifi() -> WifiStaDriver {
    let resources = WIFI_RESOURCES.init(WifiResources::new());
    let backend = Esp8266TargetBackend::new(resources);
    WifiDriver::with_backend(resources, backend)
        .initialize(InitConfig::default()).expect("wifi init")
        .configure_sta(ClientConfig::new(
            Ssid::try_from(WIFI_SSID.trim()).unwrap(),
            if WIFI_PASS.trim().is_empty() {
                Credential::Open
            } else {
                Credential::Wpa2(Passphrase::wpa2(WIFI_PASS.trim()).unwrap())
            },
        )).expect("configure sta")
        .start().expect("wifi start")
}
```

In `main`, replace the old connect path with:

```rust
    let mut driver = init_wifi();
    {
        let mut sta = driver.sta().unwrap();
        let _session = sta.connect_blocking(10_000).expect("wifi connect");
    }
    run_network(spawner, driver).await
```

Run the Xtensa example check immediately after this change:

```bash
source .envrc && cargo check --example tcp_server --target xtensa-esp8266-none-elf -Zbuild-std=core --no-default-features --features interrupt,rt,wifi-embassy-net,wifi-sta
```

Expected: FAIL with the old `wifi_net::new(state, mac, device)` and legacy runner API no longer matching the rewritten core construction flow.

- [ ] **Step 2: Rewrite `src/wifi/embassy_net.rs` over `StaLink`**

Change the module signature:

```rust
pub fn new<'d, B, const EVENTS: usize, const RX: usize, const SUBS: usize>(
    state: &'d mut NetDriverState,
    mut link: StaLink<'d, B, EVENTS, RX, SUBS>,
) -> Result<(Runner<'d, B, EVENTS, RX, SUBS>, NetDevice<'d>), DriverError>
where
    B: WifiBackend,
{
    let mac = link.mac_address()?;
    let (ch_runner, net_device) =
        ch::new(state, ch::driver::HardwareAddress::Ethernet(mac));

    Ok((Runner { ch_runner, link, last_link: LinkState::Down }, net_device))
}
```

Rewrite the runner shape:

```rust
pub struct Runner<'d, B, const EVENTS: usize, const RX: usize, const SUBS: usize> {
    ch_runner: ch::Runner<'d, MTU>,
    link: StaLink<'d, B, EVENTS, RX, SUBS>,
    last_link: LinkState,
}
```

Replace the legacy event-bus loop with a core-link loop:

```rust
pub async fn run(&mut self) -> ! {
    self.apply_link_state();

    loop {
        self.apply_link_state();
        self.poll_rx();
        self.poll_tx();

        let wait = core::future::poll_fn(|cx| {
            self.link.register_rx_waker(cx.waker());
            self.link.register_link_waker(cx.waker());

            if self.link.has_rx_data() || self.link.link_state() != self.last_link {
                core::task::Poll::Ready(())
            } else {
                core::task::Poll::Pending
            }
        });

        embassy_futures::select::select(
            wait,
            embassy_time::Timer::after(embassy_time::Duration::from_millis(5)),
        )
        .await;
    }
}

fn apply_link_state(&mut self) {
    let current = self.link.link_state();
    if current != self.last_link {
        self.ch_runner.set_link_state(match current {
            LinkState::Up => ch::driver::LinkState::Up,
            LinkState::Down => ch::driver::LinkState::Down,
        });
        self.last_link = current;
    }
}
```

In RX/TX polling, use `self.link.receive(...)` and `self.link.transmit(...)`. Treat `DriverError::WouldBlock` exactly like the legacy `ERR_MEM` path: stop the TX loop and retry later.

- [ ] **Step 3: Finish the `tcp_server` migration**

Update `run_network` in `examples/tcp_server.rs`:

```rust
async fn run_network(spawner: Spawner, mut driver: WifiStaDriver) -> ! {
    let link = driver.sta_link().unwrap();
    let net_state = NET_STATE.init(NetDriverState::new());
    let (driver_runner, net_device) = wifi_net::new(net_state, link).expect("net driver");

    let config = embassy_net::Config::dhcpv4(Default::default());
    let resources = unsafe { heap_leak_stack_resources() };
    let seed = 0x1234_5678_9abc_def0;

    let (stack, net_runner) = embassy_net::new(net_device, config, resources, seed);
    let driver_runner = unsafe { heap_leak_value(driver_runner) };
    let net_runner = unsafe { heap_leak_value(net_runner) };
    spawn_heap_task(spawner, move || driver_runner.run());
    spawn_heap_task(spawner, move || net_runner.run());
    wait_for_dhcp_and_serve(stack).await
}
```

Drop all imports and calls to:

```rust
use esp8266_hal::wifi::controller::{WifiInitConfig, WifiInterface};
use esp8266_hal::wifi::device::{clear_sta_net_diag, WifiDevice};
use esp8266_hal::wifi::event;
use esp8266_hal::wifi::{AuthMode, StaConfig, WifiController, WifiError, WifiMode};
```

Replace them with:

```rust
use esp8266_hal::wifi_core::{
    ClientConfig, Credential, Esp8266TargetBackend, InitConfig, Passphrase, Ssid, WifiDriver,
    WifiResources,
};
```

Keep the validated DHCP wait and TCP echo loop unchanged after the network stack is created.

- [ ] **Step 4: Re-run the Xtensa example check**

Run:

```bash
source .envrc && cargo check --example tcp_server --target xtensa-esp8266-none-elf -Zbuild-std=core --no-default-features --features interrupt,rt,wifi-embassy-net,wifi-sta
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/wifi/embassy_net.rs examples/tcp_server.rs
git commit -m "refactor: migrate embassy net path to wifi core"
```

### Task 6: Update Docs, Record The New Source Of Truth, And Finalize Verification

**Files:**
- Modify: `research.md`
- Modify: `.github/workflows/ci.yaml`

- [ ] **Step 1: Add the failing CI expectation**

Update `.github/workflows/ci.yaml` so the host core test job includes the new link test file:

```yaml
      - name: Run host wifi-core tests
        run: cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --lib --tests
```

Run the host suite before updating `research.md`:

```bash
cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --lib --tests -q
```

Expected: PASS locally; if it fails, stop and fix the actual regression before touching docs.

- [ ] **Step 2: Update `research.md` with the new packet/link source of truth**

Add a new section to `research.md`:

```markdown
## Core API Redesign Track

### Packet/link adapter migration

- The networking source of truth now flows through `src/wifi/core/*`.
- ESP8266 SDK WiFi events and RX callbacks publish into caller-owned
  `WifiResources`.
- `src/wifi/device.rs` and `src/wifi/embassy_net.rs` are adapter layers over
  `StaLink`, not owners of global RX or event state.
- Legacy `src/wifi/event.rs` remains a compatibility island for unmigrated
  APIs, but it is no longer the networking source of truth.
```

- [ ] **Step 3: Run the full final verification commands fresh**

Run:

```bash
cargo test --target x86_64-unknown-linux-gnu --no-default-features --features wifi-core --lib --tests -q
source .envrc && cargo check --example wifi_core_connect --target xtensa-esp8266-none-elf -Zbuild-std=core --no-default-features --features interrupt,rt,wifi-core,wifi,wifi-sta
source .envrc && cargo check --example tcp_server --target xtensa-esp8266-none-elf -Zbuild-std=core --no-default-features --features interrupt,rt,wifi-embassy-net,wifi-sta
```

Expected:

- host `wifi_core` tests: PASS
- Xtensa `wifi_core_connect`: PASS
- Xtensa `tcp_server`: PASS

- [ ] **Step 4: Commit**

```bash
git add research.md .github/workflows/ci.yaml
git commit -m "docs: record wifi core adapter migration"
```

## Spec Coverage Checklist

- Callback-safe wake registration: Task 1
- Explicit link state, disconnect reason, and overflow diagnostics: Tasks 1 and 2
- Borrowed `StaLink` API: Task 2
- ESP8266 event/RX ingress through `src/wifi/core/target.rs`: Task 3
- `smoltcp` adapter migration: Task 4
- `embassy-net` runner migration: Task 5
- `tcp_server.rs` proof path migration: Task 5
- CI and `research.md` updates: Task 6

## Placeholder Scan

- The plan contains no `TODO`, `TBD`, or “implement later” placeholders.
- Every task includes exact file paths, concrete code snippets, verification commands, and expected outcomes.

## Type Consistency Check

- Core packet ingress uses `RawRxRecord::from_raw(..., release_fn)` throughout the plan.
- Link state is always `LinkState`.
- Station packet adapter construction always flows through `driver.sta_link()`.
- Retryable TX saturation is always `DriverError::WouldBlock`.

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-04-22-wifi-adapter-migration.md`. When you're ready, use `superpowers:executing-plans` to implement it task-by-task in the current tree.
