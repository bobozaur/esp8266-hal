# Vendored ESP8266 wireless runtime

This directory is the complete native input to `esp8266-wifi-sys`. Normal
firmware builds do not clone or inspect an ESP8266 RTOS SDK checkout and do not
run bindgen, libclang, or compile C sources. Final firmware linking still uses
the pinned Xtensa GCC linker driver required by the ESP8266 Rust target.

## Provenance

- SDK: <https://github.com/espressif/ESP8266_RTOS_SDK>
- SDK commit: `da7ce37be424d71874241ca9f17cf8da0e3cdb70`
- Driver source tree used to derive this snapshot:
  `2624d1c959f71b98ff93c4f611f987a9c3d9fc71`
- Generated: 2026-07-22
- C toolchain: `xtensa-lx106-elf-gcc 8.4.0`, crosstool-NG
  `esp-2020r3-49-gd5524c1`
- Wi-Fi bindings generator: `rust-bindgen 0.70.1`
- lwIP bindings generator: `rust-bindgen 0.72.1`

The derived archives were produced once from the pinned SDK/toolchain with all
Wi-Fi features enabled, then normalized with deterministic archive headers:

```text
libesp8266_sdk.a
libwpa_supplicant.a
libmbedtls_all.a
```

`libesp8266_sdk.a` contains the SDK event, lwIP, VFS, flash, and support objects
still selected by the native Wi-Fi libraries. The former C AP netif bridge and
generic SDK stub object are intentionally absent; their live behavior is now
implemented in Rust under `src/wifi`.

The SDK Xtensa vector/context objects and the former local NMI assembly are not
vendored. Their required interrupt, context, and NMI sequences now live next to
the Rust scheduler as narrowly scoped inline assembly. `patched-sources/`
contains the canonical WPA/WPS/mbedTLS translation units and helper inputs used
to build the three derived archives. It is provenance material and is not
compiled during a normal build.

The closed ESP8266 archives and linker scripts are byte-for-byte copies from
the pinned SDK's `components/esp8266/{lib,ld}` directories. `libc.a` and
`libm.a` are the newlib archives from the toolchain above. No libgcc archive is
redistributed; the one C ABI helper not supplied by Rust compiler builtins is a
small Rust implementation. SmartConfig and ESP-NOW archives are linked only
for their corresponding Cargo features.

`src/bindings.rs` and `src/lwip.rs` are checked-in generated ABI declarations.
Manual packed/bitfield types live in `src/lib.rs`, where their layouts can be
reviewed and tested explicitly.

## Reproduction

The snapshot is intentionally reproduced out of band, never by a package
build. The original source manifest and compiler flags live in
`esp8266-wifi-sys/build.rs` at driver commit `2624d1c`.

Two checked-in patches make the derivation auditable:

- `post-generator-normalization.patch` is the complete diff from the retained
  historical generator output to `patched-sources/`. It removes temporary
  allocation-context hooks, debug printing, and direct-UART WPS tracing, and
  adds the required modification notices. The notices use `#line` so they do
  not perturb the source line identities that reach object metadata.
- `generator-rebuild.patch` applies to driver commit `2624d1c`. It adds stable
  `-ffile-prefix-map`/`-fmacro-prefix-map` values and makes each generated
  translation unit prefer the canonical file of the same name from
  `ESP8266_WIFI_CANONICAL_PATCHED_SOURCES`. The archive is therefore compiled
  from the exact checked-in canonical inputs, not from an undocumented local
  intermediate.

The following is the reproduction procedure, with the current checkout in
`$WIFI_TREE` and disposable worktrees in `$DRIVER_TREE` and `$SDK_TREE`:

```sh
git -C "$WIFI_TREE" worktree add --detach "$DRIVER_TREE" \
  2624d1c959f71b98ff93c4f611f987a9c3d9fc71
git -C /path/to/ESP8266_RTOS_SDK worktree add --detach "$SDK_TREE" \
  da7ce37be424d71874241ca9f17cf8da0e3cdb70
git -C "$DRIVER_TREE" apply \
  "$WIFI_TREE/esp8266-wifi-sys/vendor/generator-rebuild.patch"

env -u ESP8266_HAL_EAP_TRACE \
  -u ESP8266_HAL_WPS_ENROLLEE_TRACE \
  -u ESP8266_HAL_MBEDTLS_SOFTWARE_AES \
  ESP8266_RTOS_SDK="$SDK_TREE" \
  ESP8266_WIFI_CANONICAL_PATCHED_SOURCES="$WIFI_TREE/esp8266-wifi-sys/vendor/patched-sources" \
  CARGO_TARGET_DIR="$REBUILD_TARGET" \
  rustup run esp-1.80.0 cargo build -Zbuild-std=core --release \
  --target xtensa-esp8266-none-elf \
  --manifest-path "$DRIVER_TREE/esp8266-wifi-sys/Cargo.toml" \
  --features smartconfig,esp-now
```

The build script finishes the three archives before Cargo compiles the tiny
sys-crate library, so a later target-core diagnostic does not invalidate the
archive output. Verify the ten canonical generated inputs byte for byte, then
remove the obsolete `ap_netif_bridge` and generated `sdk_stubs` members from
`libesp8266_sdk.a`; those symbols are now provided by Rust. Copy the three
derived archives into `libs/`, run `./normalize-archives.sh`, and regenerate
`SHA256SUMS`. The upstream closed archives, newlib archives, linker scripts,
and generated bindings are direct copies from the pinned inputs described
above.

`normalize-archives.sh` preserves member order while setting archive member
timestamps, owners, groups, and modes deterministically. The compiler prefix
maps remove checkout-specific paths from object metadata. Together they make a
repeated snapshot reviewable without embedding workstation identity.

The current tree can be verified without any SDK checkout:

```sh
ESP8266_RTOS_SDK=/not/used LIBCLANG_PATH=/not/used \
CARGO_INCREMENTAL=0 cargo build --release --example tcp_server \
  -Zbuild-std=core \
  --no-default-features \
  --features interrupt,rt,wifi-embassy-net

(cd esp8266-wifi-sys/vendor && sha256sum --check SHA256SUMS)
```

## Licensing

`licenses/` contains the upstream licenses for the ESP8266 RTOS SDK and the
third-party lwIP, mbedTLS, wpa_supplicant, and newlib code included in the
archives. The crate's Rust code remains dual-licensed under MIT or Apache-2.0.
