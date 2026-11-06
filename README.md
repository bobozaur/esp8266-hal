# esp8266-hal

A `no_std` hardware abstraction layer for ESP8266, with optional Wi-Fi support.
Wi-Fi provides station, access-point and simultaneous station/AP operation,
scanning, ESP-NOW, promiscuous reception, WPS, SmartConfig and WPA2-Enterprise.
Applications choose when to initialize and activate the radio, their network
stack and executor, and their reconnection policy.

## Build

This checkout pins Rust `esp-1.80.0`. Install that Xtensa toolchain and put
`xtensa-lx106-elf-gcc` on `PATH`; the supported GCC version is 8.4.0,
`esp-2020r3-49-gd5524c1`. [`.cargo/config.toml`](.cargo/config.toml) selects the
ESP8266 target and linker scripts. Wi-Fi builds need `core` and `alloc`:

```sh
ESP8266_HAL_WIFI_SSID=my-network ESP8266_HAL_WIFI_PASS=my-password \
  cargo build --release --example wifi_core_connect \
  --no-default-features --features wifi -Zbuild-std=core,alloc

cargo build --release --example tcp_server \
  --no-default-features --features wifi-embassy-net,embassy -Zbuild-std=core,alloc
```

Normal builds use checked-in native archives and bindings. They need no SDK
source checkout, bindgen, libclang, network access or native rebuilding once
Cargo dependencies and the pinned toolchain are installed.

An external application needs its own Cargo configuration because dependencies
cannot supply it:

```toml
[build]
target = "xtensa-esp8266-none-elf"

[target.xtensa-esp8266-none-elf]
linker = "xtensa-lx106-elf-gcc"
rustflags = ["-C", "link-arg=-nostartfiles", "-C", "link-arg=-Tlink.x"]
```

The [downstream fixture](tests/fixtures/downstream-wifi-link) exercises this
boundary with and without each network adapter.

## Runtime and ownership

Start from a synchronous `#[esp8266_hal::entry]` function. Initialize an
application-owned allocator, split `TIMER`, and call
`Runtime::init(timer2, allocator)` before `Wifi::new(runtime, config)`.
[Example support](examples/support/mod.rs) shows the allocator and owned UART
setup. Ordinary entry does not start Wi-Fi or initialize its runtime.

The `wifi` feature requires a registered Rust global allocator even if the
radio remains unused. The HAL supplies neither that allocator nor its heap.
The explicit `WifiAllocator` provider may share the global allocator; it must
satisfy the documented DRAM, lifetime and interrupt/NMI exclusion contracts.

The linker reserves 8 KiB for the application stack and exposes the remaining
heap through `_heap_start` and `_heap_end`. Applications may override the
absolute `STACK_SIZE` linker symbol after validating their own stack use. The
enterprise example reserves 4 KiB to leave room for certificate processing.
Runtime initialization consumes Timer2/FRC2 and reserves the CPU compare and
software scheduling interrupts for the boot. Timer1 remains application-owned.
`Runtime::now()` supplies monotonic `Duration` timestamps for caller-polled stacks.
Runtime initialization failure requires reset; stopping or dropping Wi-Fi does
not tear down the scheduler.

`wifi.split()` borrows one controller and independent station/AP packet
interfaces. The controller handles configuration and connection operations while
a network stack retains its interface. Rust queue capacities are selected in
`InitConfig::queues` and allocated during construction. The native SDK still
allocates during operation. Its RX/TX buffer dimensions become fixed on first
native initialization, including partial initialization; reset to change them.

Blocking and async methods use the same operation state. Blocking waits have
explicit deadlines; async callers choose their timeout mechanism. A timeout or
dropped future stops waiting, and SDK work may continue. Use explicit disconnect
or scan cancellation to abort it. RX packets retain native buffers until released;
held packets make whole-radio stop return `Busy`. See the owning API documentation
for cancellation, session lifetimes and teardown requirements.

## Features and examples

| Feature | Purpose |
| --- | --- |
| `rt`, `interrupt` | Entry/runtime and interrupt dispatch; enabled by default |
| `wifi` | Controller, packet interfaces and native runtime; enables `rt` and `interrupt` |
| `embassy` | Optional sleeping executor and time integration |
| `wifi-smoltcp` | Caller-polled smoltcp device adapter |
| `wifi-embassy-net` | Embassy network channel, runner and time integration; executor remains application-selected |
| `wifi-esp-now` | Plain and encrypted peers, RX and TX completion |
| `wifi-smartconfig` | ESPTouch/AirKiss provisioning |
| `wifi-enterprise` | PEAP and EAP-TLS credentials and supplicant |
| `wifi-wps` | WPS PBC/PIN provisioning |
| `wifi-promiscuous` | Packet reception and filters |

Each optional Wi-Fi feature enables `wifi`. ESP-NOW and SmartConfig also select
their native archives. Ordinary HAL applications can omit `wifi` and its allocator
and native-runtime requirements. Controller futures work with any suitable
executor. The optional Embassy executor starts explicitly through `Executor::run`.

Both `embassy` and `wifi-embassy-net` use embassy-time's generic timer queue,
which accepts any executor's wakers. It defaults to 64 entries; select one
`generic-queue-N` feature (8, 16, 32, 64 or 128) on the application's
`embassy-time` dependency to trade static memory for timer capacity. Size it for
the application's concurrent timer wakers: overflow wakes queued timers early
and can increase CPU use. The examples select eight entries for three tasks.

Examples use owned UART0 on GPIO1/GPIO3 at 115200 baud. Start with
[`raw_scan`](examples/raw_scan.rs),
[`wifi_core_connect`](examples/wifi_core_connect.rs), or
[`tcp_server`](examples/tcp_server.rs). Other examples cover TCP clients, static
IPv4 configuration, AP/mixed mode and each optional protocol. Configure
application credentials and protocol options in the example source as documented
there. Certificates are application data; the enterprise example shows where to
include trusted CA and client certificate/key material.

Wi-Fi supplies Ethernet frames. IP addressing, DHCP and sockets belong to the
selected network stack. AP/mixed examples demonstrate radio control; applications
supply their own AP IP services. TCP examples use Embassy networking. The
SmartConfig example keeps provisioning alive through smoltcp DHCP and the
ESPTouch acknowledgement. The smoltcp adapter borrows caller-owned TX storage
and enables Ethernet, IPv4 and UDP; applications enable additional smoltcp
protocols on their own dependency.

## Native SDK and maintenance

Rust owns initialization, tasks, queues, timers, event delivery, allocation and
packet ownership around unchanged Espressif radio binaries. The sys crate keeps
open C WPA/WPS/EAP and mbedTLS snapshots because a qualified Rust replacement
preserving TLS 1.2 key export, certificate profiles and recoverable allocation
failure has not been established. Newlib supplies native formatting and strings;
the driver supplies its allocator and syscall boundary. There is no SDK support
archive, native IP stack, VFS or maintained SDK rebuild workflow. The
[vendor manifest](esp8266-wifi-sys/vendor/README.md) identifies every remaining
binary, its provenance, licenses and security adaptations.

TLS record content is limited to 2,048 bytes in each direction, RSA moduli to
2,048 bits, and elliptic-curve groups to 256 bits. EAP fragmentation does not
raise the TLS record limit. Certificate chains and cryptographic work must fit
the application's shared heap; arbitrary chains are not supported by these
limits alone. See the enterprise credential API and native configuration.
Supplying a CA enables certificate-chain verification; the bundled backend does
not verify certificate validity dates.

CI checks host contracts, independent Cargo features, ESP8266 links and external
consumer packaging boundaries. Native ownership regression tests live beside
the sys crate. Hardware validation is necessary for radio behavior, scheduling,
stack budgets and interoperability; successful builds alone do not establish it.

The 0.6 API replaces the experimental Wi-Fi driver with explicit runtime setup,
one controller and borrowed packet interfaces. Use standard `#[entry]` and owned
UART facilities instead of the experimental `#[main]` and global printing APIs.
Timers/watchdog use `core::time::Duration`; flash erase/program calls are unsafe
and require application-owned writable ranges that exclude firmware and board
calibration data. Read each operation's safety contract before use.

Before publishing HAL 0.6, publish its required dependency versions:
`esp8266` 0.7, `esp8266-hal-proc-macros` 0.2 and `esp8266-wifi-sys` 0.1.
Local workspace builds do not establish registry availability.

## License

The Rust crates are licensed under Apache-2.0 or MIT at your option. Vendored
native code retains its upstream licenses as recorded in the vendor manifest.
Contributions are dual-licensed under the same terms unless stated otherwise.
