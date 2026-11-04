# Migrating to 0.6

Version 0.6 adds Wi-Fi to the published HAL. The Wi-Fi changes below describe
migration from the experimental development branch, which used public typestates
and backend parameters. The driver now has one ESP8266 controller and borrowed
packet interfaces, with explicit allocator and SDK runtime initialization. Use
the executable
[station example](../examples/wifi_core_connect.rs) and
[shared runtime setup](../examples/support/mod.rs) as complete starting points.

## Initialize the application runtime

The application owns a permanently live allocator implementing
`unsafe trait WifiAllocator: GlobalAlloc + Sync`, including
`free_dram_bytes()`. It must provide byte-addressable DRAM, honor allocation
layouts, return null on exhaustion, and support SDK task preemption and interrupt
contexts, including Wi-Fi NMI allocation. The HAL critical-section implementation
protects shared state from ordinary interrupts and, in Wi-Fi builds, NMI. Keep
allocator operations bounded and never wait for interrupts while protected.
Statistics must report available DRAM without allocating.

Whenever `wifi` is enabled, register an application global allocator even if
Wi-Fi remains unused. `allocator-api2` enables the upstream `alloc` dependency,
which requires that registration at link time. The explicit runtime provider may
share that allocator, as the examples do with `esp_alloc::EspHeap`. The HAL supplies and initializes neither
the global allocator nor its heap. Heap bounds come from `_heap_start` /
`_heap_end` in the [linker scripts](../ld/memory.x). Keep the original application stack excluded
from the heap for the entire boot.

Split the timer peripheral and initialize the runtime once from ordinary task
context with interrupts enabled:

```rust,ignore
use esp8266_hal::{timer::TimerExt, wifi::Runtime};

let (timer1, timer2) = peripherals.TIMER.timers();
let runtime = Runtime::init(timer2, &ALLOCATOR).expect("runtime init");
```

`ALLOCATOR` here is the application's initialized static provider. Runtime
initialization consumes Timer2 even on failure; reset before retrying. The
runtime reserves FRC2, the CPU compare interrupt and the software scheduling
interrupt. It keeps the scheduler and timer infrastructure alive after Wi-Fi
stops or is dropped. `Timer1` remains application-owned.

## Replace the driver chain

| Previous API | 0.6 API |
| --- | --- |
| `WifiResources` and capacity const generics | `InitConfig::queues` and fallible allocation during `Wifi::new` |
| `WifiDriver::new(resources).initialize(config)` | `Wifi::new(runtime, config)` |
| `configure_sta`, `configure_ap`, `configure_mixed` | `controller.set_configuration(&Configuration::...)` |
| Mode/lifecycle typestates and public backend parameters | One `Controller`; invalid operations return errors |
| `enable_sta()` | `controller.enable_station()` for scanning/provisioning without credentials |
| `start()` | `start_blocking(timeout_ms)` or `start_async()` |
| Temporary `driver.sta()` / `driver.ap()` controls | Controller methods |
| `sta_link()`, `ap_link()`, `mixed_link()` | `wifi.split()` and its named packet interfaces |
| Separate connection error types/backend codes | `DriverError`, including SDK codes and disconnect reasons |

For example, after explicit runtime initialization:

```rust,ignore
use esp8266_hal::wifi::{
    ClientConfig, Configuration, InitConfig, Ssid, StationCredential, Wifi,
};

let mut wifi = Wifi::new(runtime, InitConfig::default()).expect("wifi init");
let (mut controller, interfaces) = wifi.split();
let station = ClientConfig::new(
    Ssid::try_from("my-network").expect("ssid"),
    StationCredential::open(),
);
controller.set_configuration(&Configuration::Station(station)).expect("configure");
controller.start_blocking(1_000).expect("start");
controller.connect_blocking(30_000).expect("connect");
// Move interfaces.station into the selected network adapter.
```

Keep `Wifi` alive while its controller or interfaces are borrowed. Long-lived
Embassy tasks can borrow a `Wifi` stored in `StaticCell`, as the network examples
do. `Configuration::AccessPoint` and `Configuration::Mixed` use the same
controller. Stop before reconfiguring.

The defaults reserve four station RX slots, zero AP RX slots, 1514-byte frames,
eight events and zero protocol-specific slots. Set `access_point_rx`,
`esp_now_peers`, `esp_now_rx` or `promiscuous_rx` before construction as needed.
Rust queue capacities remain fixed for that Wi-Fi instance. SDK allocations still
use the shared provider during operation; queue preallocation does not make the
whole native radio allocation-free.

TLS and EAP use that same application allocator directly. The separate RSA
reservation pool and its native hooks are removed. The nine audited native
initialization reservations remain to handle SDK startup paths that do not check
allocation failures.

Native hardware/PP buffers survive deinitialization. Once the first native entry
begins, its RX buffer, RX packet and TX buffer counts are locked for the boot,
even if a later initialization phase fails. A subsequent `Wifi::new` with
different counts returns `InitError::BufferConfigurationLocked`; reset to change
them. Later instances may use a different Rust `QueueConfig`. The
[native allocation notes](native-runtime.md#allocation-and-reinitialization)
explain startup reservation and persistent native storage.

## Choose waiting and cancellation policy

Start, connect, disconnect and scan provide blocking and async waits over shared
operation state. Blocking deadlines are milliseconds. Async operations have no
internal timeout; apply the application's timeout mechanism.

A timeout or dropped future stops waiting and may leave SDK work active. Use
`disconnect()` to request an abort/disconnection, or
`disconnect_blocking(timeout_ms)` / `disconnect_async()` to await completion.
For scans, call `cancel_scan()` explicitly. A new conflicting operation returns
`Busy` while previous work remains active. There is no automatic reconnect loop.

Scanning requires started station mode with no connection or connection attempt:

```rust,ignore
controller.enable_station().expect("station mode");
controller.start_blocking(1_000).expect("start");
let mut records: [esp8266_hal::wifi::AccessPointRecord; 16] =
    core::array::from_fn(|_| Default::default());
let count = controller.scan_blocking(
    &esp8266_hal::wifi::ScanOptions::all(), &mut records, 15_000,
).expect("scan");
// Only records[..count] contain this scan's output.
```

`scan_async(options, records)` waits for SDK completion events. These fragments
show separate station-connect and scan flows; choose configuration before startup.

Protocol sessions are concrete borrowed types returned by
`controller.esp_now()`, `.promiscuous()`, `.wps()` and `.smart_config()`.
Provisioning `next_event()` returns `Result<Option<Event>, DriverError>` so event
overflow is distinguishable from an empty queue.

## Select networking and optional Embassy support

`wifi` enables `rt` and `interrupt`; it no longer enables Embassy or smoltcp.
The opt-in boundary remains because global allocator registration and dormant
SDK storage affect even unused Wi-Fi builds. Remove
`wifi-core` from feature lists. Enable `wifi-smoltcp` for
`wifi::smoltcp::WifiDevice::new(interface)`, or `wifi-embassy-net` for
`wifi::embassy_net::new(state, interface)`. Both adapters accept either station or
AP interfaces; the mixed-mode wrapper and separate AP constructors are removed.
The smaller-MTU Embassy constructor remains `new_with_mtu`.

The library leaves IP addressing, DHCP and sockets to the chosen network stack.
Custom DHCP and static-IP helpers moved to
[example support](../examples/support). SmartConfig acknowledgement policy lives
in its [example](../examples/wifi_smartconfig.rs).

For the supplied executor, enable `embassy` and keep a synchronous
`#[esp8266_hal::main]` entry point. Create `Executor` explicitly and call
`executor.run(runtime, |spawner| { ... })`. Async `#[main]` is no longer accepted.
Controller futures themselves do not require that executor.

## Update other HAL code and builds

Timers and the watchdog accept `core::time::Duration`. The integer `.s()`,
`.ms()`, `.us()` and `.ns()` conveniences return that type. `TIMER.timers()`
returns `(Timer1, Timer2)`; Wi-Fi consumes Timer2. Use `try_start(duration)` when
a timer duration may exceed its hardware range.

Interrupt handlers use the static handler model shown in
[`uart_interrupt`](../examples/uart_interrupt.rs). Raw interrupt enable is now
unsafe and requires ownership of the source and appropriate context; runtime
interrupts remain reserved for Wi-Fi.

The HAL selects `critical-section/restore-state-u64` to preserve complete processor
and NMI state across nested sections. Downstream dependencies should not force a
competing `restore-state-*` feature; use `critical-section` without selecting a
restore-state format.

Flash erase and program operations are now unsafe: the caller must reserve a
writable range within the installed chip, exclude executing code, live mapped
references and calibration data, and meet the documented execution constraints.
The flash example requires an explicit scratch offset. Reads remain safe.

Wi-Fi firmware builds need `-Zbuild-std=core,alloc`. External applications must
set their own target, Xtensa GCC linker and linker-script options; a dependency's
Cargo configuration is not inherited. See the
[external consumer fixture](../tests/fixtures/downstream-wifi-link).

The SDK ABI remains unchanged. The existing native TLS backend now handles a
null connection during failed-initialization cleanup. Optional Camellia and 3DES
TLS suites and mbedTLS Blowfish are disabled to recover memory; AES-based TLS and
WPA's separate DES implementation for MSCHAPv2 remain. All three derived native
archives were rebuilt from the canonical SDK configuration; the closed Espressif
radio archives are unchanged.
Explicit aliases in
[`ld/wifi.x`](../ld/wifi.x) replace blanket duplicate-symbol suppression; see
[the native adaptation notes](native-runtime.md).

The pinned TLS record-content budgets are 768 bytes incoming and 512 bytes
outgoing. MPI values are limited to 256 bytes, bounding RSA moduli at 2048 bits;
EC groups are limited to 256 bits. Certificate chains and cryptographic work
must also fit the shared heap. Use compact certificates and compatible record
sizes; key-size limits alone do not qualify a certificate or RSA profile. See
[TLS memory limits](native-runtime.md#tls-memory-limits).

The EAP example preserves SSID and credential text, including leading and
trailing whitespace. Its numeric and boolean configuration trims ASCII
whitespace only; it does not normalize credential values.

Section sizes exclude runtime heap and stacks; see the
[cost and evidence notes](native-runtime.md#cost-and-current-evidence). The idle
stack overflow and NMI exclusion defects are corrected. Integrated validation
covers the available radio, network and peripheral fixtures; it does not establish
first-attempt association reliability. WPS PBC/PIN and SmartConfig provisioning,
DHCP and acknowledgement pass against an external fixture. PEAP/MSCHAPv2
authentication and incorrect-password rejection also pass. Compact P-256 client-certificate EAP-TLS
also passes, including the four-way handshake. Examples keep bounded retry policy in
application code.

Before publishing HAL 0.6, publish the currently unpublished dependency versions:
PAC `esp8266` 0.7, `esp8266-hal-proc-macros` 0.2 and `esp8266-wifi-sys` 0.1. Local
package/link checks do not establish registry readiness. Follow
[hardware validation](../plan.md) separately; older research does not qualify the
migrated implementation.
