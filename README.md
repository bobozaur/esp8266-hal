# esp8266-hal

A `no_std` hardware abstraction layer and Wi-Fi driver for the ESP8266.
The 0.6 API provides station, access-point and mixed operation, scanning,
optional network adapters, and protocol sessions. See the
[0.6 migration guide](docs/migration-0.6.md) for breaking changes.

## Build

The repository pins `esp-1.80.0` in
[`rust-toolchain.toml`](rust-toolchain.toml). Install the ESP8266 Xtensa Rust
toolchain and put `xtensa-lx106-elf-gcc` on `PATH`.
[`.cargo/config.toml`](.cargo/config.toml) selects the target, linker and linker
script for this checkout.

Build the synchronous station example with application-selected credentials:

```sh
ESP8266_HAL_WIFI_SSID=my-network \
ESP8266_HAL_WIFI_PASS=my-password \
cargo build --release --example wifi_core_connect \
  --no-default-features --features wifi -Zbuild-std=core,alloc
```

For DHCP and TCP through Embassy:

```sh
cargo build --release --example tcp_server \
  --no-default-features --features wifi-embassy-net -Zbuild-std=core,alloc
```

An external application must provide its own Cargo target/linker configuration;
Cargo does not inherit configuration from a dependency. The
[downstream consumer fixture](tests/fixtures/downstream-wifi-link) exercises that
boundary independently of this checkout's configuration.

## Runtime and ownership

Applications initialize their allocator, call `Runtime::init(timer2, provider)`,
and construct `Wifi::new(runtime, config)`. Enabling `wifi` requires the
application to register a Rust global allocator, even when the radio is unused:
`allocator-api2` enables the upstream `alloc` dependency. The HAL installs no
global allocator. Registration does not initialize the heap or runtime, and
ordinary `#[esp8266_hal::main]` entry does not start the runtime or Wi-Fi.
The [shared example setup](examples/support/mod.rs) implements `WifiAllocator`
using an application-owned `esp_alloc::EspHeap` over linker-defined unused DRAM.
The [linker memory map](ld/memory.x) permanently reserves 8 KiB for the
application stack by default. Applications may override the absolute
`STACK_SIZE` linker symbol; it must be positive, 16-byte aligned and fit within
DRAM without overlapping static data. The [`wifi_eap`](examples/wifi_eap.rs)
example reserves 4 KiB based on measured stack use. A smaller reservation needs
validation against the application's own call and interrupt depth.

`Runtime` initializes once per boot and reserves Timer2/FRC2, the CPU compare
interrupt and the software scheduling interrupt. Timer1 remains available to the
application. Initialization failure consumes Timer2 and requires reset. Runtime
infrastructure survives Wi-Fi stop and drop. Native RX/TX buffer dimensions
remain fixed after first native initialization; changing them requires reset.
Rust queue capacities can change between Wi-Fi instances. See the
[native initialization contract](docs/native-runtime.md#allocation-and-reinitialization).

`Wifi` owns stable callback state and fixed-capacity queues. `wifi.split()` borrows
one `Controller` and named `interfaces.station` / `interfaces.access_point`
packet endpoints. Configure and reconnect through the controller while a network
stack retains its endpoint. Queue capacities are ordinary `InitConfig::queues`
fields; public types carry no backend, lifecycle or queue-capacity parameters.

Blocking and async operations share the driver. Blocking waits take explicit
millisecond deadlines; async callers choose their own timeout mechanism.
Dropping a connection future stops waiting while the SDK attempt may continue.
Use explicit disconnection to abort it. The driver leaves retry and reconnect
policy to the application.

Wi-Fi handles radio control and Ethernet frames. Applications select IP
addressing, DHCP and sockets through their network stack. The
[AP example](examples/wifi_ap.rs) keeps its DHCP server in
[example support](examples/support/dhcp.rs), and the
[SmartConfig example](examples/wifi_smartconfig.rs) sends its ESPTouch
acknowledgement through smoltcp.

## Features

| Feature | Purpose |
| --- | --- |
| `rt`, `interrupt` | Entry/runtime support and interrupt dispatch; enabled by default |
| `wifi` | Wi-Fi controller, packet interfaces and SDK runtime; enables `rt` and `interrupt` |
| `embassy` | Optional executor and time integration; enables `wifi` |
| `wifi-smoltcp` | Caller-polled smoltcp device adapter; enables `wifi` |
| `wifi-embassy-net` | Embassy network channel and runner; enables `embassy` |
| `wifi-esp-now` | ESP-NOW peers, RX and TX status |
| `wifi-smartconfig` | ESPTouch/AirKiss provisioning |
| `wifi-enterprise` | WPA2-Enterprise credentials and supplicant integration |
| `wifi-wps` | WPS PBC/PIN provisioning |
| `wifi-promiscuous` | Packet capture and filters |

Each protocol feature enables `wifi`; ESP-NOW and SmartConfig also select native
archives. The `wifi` boundary keeps the allocator registration requirement and
dormant SDK storage out of applications using only the HAL.
Wi-Fi itself does not require Embassy;
async controller futures can be polled by another suitable executor. Async
`#[main]` entry sugar has been removed: Embassy examples use a synchronous entry
point and explicitly call `Executor::run(runtime, init)`.

The smoltcp adapter selects Ethernet, IPv4 and UDP to satisfy the pinned stack's
minimum feature requirements. Applications select additional socket protocols
on their smoltcp dependency. The Embassy adapter does not enable smoltcp in the HAL.

## Examples and validation

Executable examples live in [`examples`](examples). Start with
[`wifi_core_connect`](examples/wifi_core_connect.rs),
[`raw_scan`](examples/raw_scan.rs), or
[`tcp_server`](examples/tcp_server.rs). The hardware harness lists scenarios and
the fixtures they require:

```sh
cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw list
```

Validation covers repeated lifecycle/scanning on two boards, bounded connection
recovery, AP and mixed-mode DHCP, TCP traffic, plain/encrypted ESP-NOW,
promiscuous capture, flash, timers and UART interrupts. Host tests and target
links cover every retained feature. Current hardware checks pass WPS PBC/PIN,
SmartConfig provisioning with DHCP and acknowledgement, and PEAP with
wrong-password rejection. Current EAP-TLS interoperability checks pass with
conventional P-256 server/client intermediate chains and an RSA-2048 server
leaf with a client intermediate chain, including EAP success and the WPA
four-way handshake. P-256 EAP-TLS, PEAP and wrong-password rejection also pass
with 64-byte EAP fragments. Invalid certificate material is rejected, with
subsequent connections succeeding without rebooting. Stop/start and reconnect
reuse the configured credentials. Intermittent transport failures against one AP
remain unexplained. The native TLS build has
2 KiB incoming and outgoing record-content limits; certificate chains and
cryptographic work must also fit the available heap. See the
[record and memory limits](docs/native-runtime.md#tls-memory-limits). The
[refactor plan](plan.md) records results and qualification limits.

The refactor establishes no general performance or size improvement. Section
measurements and their heap/stack limits are recorded in the
[native/runtime notes](docs/native-runtime.md#cost-and-current-evidence).

## Native SDK and publication

Espressif's closed ESP8266 Wi-Fi/PHY archives supply the radio implementation.
The SDK also requires compatible task, allocation, timer, event and packet ABIs;
the retained Rust adapters implement that boundary. Native WPA/mbedTLS/support
objects include documented adaptations. Exact versions, licenses, hashes and
reproduction inputs remain in the
[vendoring manifest](esp8266-wifi-sys/vendor/README.md).
The closed Espressif radio archives are unchanged; the derived archives were
rebuilt while preserving the native ABI. The
[adaptation audit](docs/native-runtime.md) records the retained Rust boundary,
removed wrappers and three explicit linker aliases; blanket duplicate-symbol
suppression has been removed.

Normal builds use checked-in bindings and archives without an SDK checkout,
bindgen, libclang or C compilation. Final firmware linking uses the Xtensa GCC
driver. ESP-NOW and SmartConfig native archives follow their respective features.

Publication requires the currently unpublished dependency versions
`esp8266` 0.7, `esp8266-hal-proc-macros` 0.2 and `esp8266-wifi-sys` 0.1 to become
available before `esp8266-hal` 0.6. Local path dependencies and successful
workspace builds alone do not establish registry readiness.

## License

The Rust crates are licensed under Apache-2.0 or MIT at your option. Vendored
native code retains its upstream licenses as recorded in the vendoring manifest.
Contributions are dual-licensed under the same terms unless stated otherwise.
