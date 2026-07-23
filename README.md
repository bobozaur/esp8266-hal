# esp8266-hal

A `no_std` hardware abstraction layer and Wi-Fi driver for the ESP8266.

The Wi-Fi implementation uses a typed, caller-owned Rust API inspired by the
current esp-rs drivers. It supports station, access-point, mixed AP+STA,
Embassy networking, scanning, promiscuous capture, ESP-NOW, WPS, SmartConfig,
and WPA2-Enterprise operation.

## Wi-Fi architecture

- `WifiDriver` uses typestates to make initialization, configuration, start,
  and stop transitions explicit.
- `WifiResources` owns event, packet, and optional feature storage. Packet and
  event hot paths do not allocate.
- The ESP8266 target backend maps the Rust configuration directly to the
  checked-in SDK ABI. There is no second controller or event model underneath.
- The FreeRTOS-compatible scheduler, queues, timers, allocator, NVS shim, AP
  netif bridge, RX bridge, and SDK runtime adapters are implemented in Rust.
- `embassy-net` and `smoltcp` adapters consume the same owned station/AP links.

Espressif did not publish the ESP8266 Wi-Fi and PHY implementation as source.
Those closed archives remain native, along with the SDK/WPA/mbedTLS objects
they require. The scheduler, interrupt dispatch, context switching, and NMI
handler are Rust; only the architectural instructions themselves use local
inline assembly. Exact native versions, sources, licenses, and hashes are
recorded in [`esp8266-wifi-sys/vendor`](esp8266-wifi-sys/vendor/README.md).

Normal builds do not need an ESP8266 RTOS SDK checkout, bindgen, or libclang,
and compile no C sources. `esp8266-wifi-sys` links checked-in bindings and
archives and exports its vendored ROM scripts to the HAL for the final firmware
link, which still uses the Xtensa GCC driver. ESP-NOW and SmartConfig archives
are linked only when their Cargo features are enabled.

## Toolchain

The repository pins the `esp-1.80.0` Rust toolchain in
[`rust-toolchain.toml`](rust-toolchain.toml). Building for the ESP8266 also
requires `xtensa-lx106-elf-gcc` on `PATH`; `.cargo/config.toml` selects the
`xtensa-esp8266-none-elf` target and linker scripts.

Build a station-mode Embassy example with:

```sh
CARGO_INCREMENTAL=0 cargo build --release --example tcp_server \
  -Zbuild-std=core \
  --no-default-features \
  --features interrupt,rt,wifi-embassy-net
```

The examples accept credentials through compile-time environment variables:

```sh
ESP8266_HAL_WIFI_SSID=my-network \
ESP8266_HAL_WIFI_PASS=my-password \
CARGO_INCREMENTAL=0 cargo build --release --example tcp_server \
  -Zbuild-std=core \
  --no-default-features \
  --features interrupt,rt,wifi-embassy-net
```

## Wi-Fi features

| Feature | Purpose |
| --- | --- |
| `wifi` | Wi-Fi core, station/AP, and scanning |
| `wifi-embassy-net` | Embassy network-driver adapter |
| `wifi-esp-now` | ESP-NOW peers, RX, and TX status |
| `wifi-smartconfig` | ESP-Touch/AirKiss provisioning |
| `wifi-enterprise` | WPA2-Enterprise EAP/TLS credentials and supplicant path |
| `wifi-wps` | WPS PBC/PIN provisioning |
| `wifi-promiscuous` | Promiscuous capture and packet filters |

On the ESP8266 target, every `wifi` configuration also requires the `rt` and
`interrupt` features; invalid combinations fail with a direct compile error.

Feature-specific storage is explicit in the trailing `WifiResources` const
generics and defaults to zero. For example, ordinary station firmware can use
`WifiResources::<8, 4>`, while the ESP-NOW examples additionally provide peer
and packet capacities. Copied AP receive storage uses the final slot-count and
frame-byte capacities.

## Examples and hardware tests

Maintained examples live in [`examples`](examples). The hardware harness can
discover the attached fixture, flash firmware, coordinate network peers, and
retain UART/host evidence:

```sh
cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw list
cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw doctor
cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw run sta_connect
```

Use `wifi-hw matrix` for the automated suite. Scenarios requiring a WPS button,
phone provisioning, enterprise fixture, or companion radio describe their
prerequisites in `wifi-hw list`.

## License

The Rust crates are licensed under either Apache-2.0 or MIT at your option. The
vendored wireless runtime retains its upstream licenses; see the vendoring
manifest for details.

Contributions are dual-licensed under the same terms unless explicitly stated
otherwise.
