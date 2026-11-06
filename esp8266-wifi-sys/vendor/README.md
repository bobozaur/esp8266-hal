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
- Baseline snapshot generated: 2026-07-22
- Derived archives rebuilt: 2026-11-06
- C toolchain: `xtensa-lx106-elf-gcc 8.4.0`, crosstool-NG
  `esp-2020r3-49-gd5524c1`
- Wi-Fi bindings generator: `rust-bindgen 0.70.1`

The derived archives are built out of band from the pinned SDK/toolchain with all
Wi-Fi features enabled and deterministic archive headers:

```text
libesp8266_sdk.a
libwpa_supplicant.a
libmbedtls_all.a
```

`libesp8266_sdk.a` contains the SDK event, VFS, flash, and support objects.
The unused lwIP stack, mbedTLS socket adapter, C AP netif bridge and generic
SDK stub object are absent. AP and station frames use
the Rust receive/transmit paths directly, without an AP lwIP netif lifecycle.

The SDK Xtensa vector/context objects and the former local NMI assembly are not
vendored. Their required interrupt, context, and NMI sequences now live next to
the Rust scheduler as narrowly scoped inline assembly. `patched-sources/`
contains the canonical WPA/WPS/mbedTLS translation units and helper inputs used
to build the three derived archives. It is provenance material and is not
compiled during a normal build.

The canonical SDK configuration keeps the SDK-default optional Camellia,
mbedTLS DES, and Blowfish algorithms disabled. TLS retains AES with CBC, CCM,
and GCM plus RSA, ECDHE-RSA, and ECDHE-ECDSA key exchanges; optional Camellia
and 3DES TLS compatibility is omitted. WPA's separate DES implementation
continues to support MSCHAPv2. Keep the SDK TLS server configuration enabled:
disabling it caused measured EAP client slowdowns and handshake timeouts. The EAP
backend still does not implement server handshakes. The canonical `sdkconfig.h`
supplies the configuration for both native compilation and binding generation.

The canonical TLS backend accepts a null connection during teardown. EAP
initialization failure can leave or reset the connection pointer to null before
shared cleanup runs; the guard preserves that failure path without a native
null dereference. It does not change the original initialization error.
TLS and EAP handshake allocations use the application-provided allocator. EAP
reassembly reserves the declared flight once; TLS borrows the worker-owned
request during handshake and phase-two decryption, clearing each borrow before
returning. Owned reassembled handshake flights can compact consumed input tails.
The backend retains static raw client certificate, key and password material, parsing it at `CLIENT_CERTIFICATE` after
server verification. Parsed CA, client certificate and key allocations are freed
after their respective last uses. Shared pre-encryption helpers release and
restore idle record buffers around cryptographic work. Output growth rounds
capacity to 256-byte increments with at most 255 spare bytes.
Certificate parsing consumes every DER certificate and rejects malformed trailing
bytes and partially invalid PEM bundles. Fatal TLS errors remain EAP failures even
when TLS produces no output; a nonfatal empty response still requests an ACK.

The canonical TLS record-content limits are 2,048 bytes in each direction; EAP
fragmentation does not increase that cap. MPI/RSA and elliptic-curve limits remain
2,048 and 256 bits. Certificates, parsed chains and cryptographic work share the
application heap with radio traffic. The record limits alone do not establish
that an arbitrary certificate chain fits the available heap or worker stack.

Enterprise EAP receive processing always uses the dedicated WPA2 worker when
`USE_WPA2_TASK` is enabled. It requests a 4,096-byte stack; the Rust runtime adds
176 bytes of context storage. Allocation failures return `ESP_ERR_NO_MEM`; packet
size or heap pressure must not move TLS work onto the smaller Wi-Fi task stack.
Commands are queued by value. Receive headers and payloads share one allocation.
Staged initialization rolls back partial ownership;
teardown closes admission and waits for worker and producer completion before
freeing peer state. Scheduler-suspended callbacks request exit without freeing
live state. Credential inputs survive disable for reuse on the next enable.
The `wifi_eap` example separately reserves a measured 4 KiB application stack;
the library default remains 8 KiB and can be overridden through `STACK_SIZE`.

WPS owns partial initialization and normal teardown through the same cleanup
path. Its worker queue stores events by value, and completed credentials are
applied once from a checked stack-local configuration before success is
published. Temporary probe/association IEs are submitted directly to the SDK's
copying setter rather than copied through another buffer.
Incoming WPS fragments have one attempt-owned buffer with checked message and
append lengths; terminal events release it and quiesce pending transaction work.

The closed ESP8266 archives and linker scripts are byte-for-byte copies from
the pinned SDK's `components/esp8266/{lib,ld}` directories. `libc.a` and
`libm.a` are the newlib archives from the toolchain above. No libgcc archive is
redistributed; the one C ABI helper not supplied by Rust compiler builtins is a
small Rust implementation. SmartConfig and ESP-NOW archives are linked only
for their corresponding Cargo features.

`src/bindings.rs` contains checked-in generated Wi-Fi ABI declarations.
Manual bitfield types live in `src/lib.rs`. `tests/abi_layout.c` compiles
independent size, alignment and offset checks against the pinned target headers;
its `abi_rx_control` byte image cross-checks the Rust accessors. Generated IP
address types are dependencies of SDK event payloads, not an embedded IP stack.
The SDK minimum-free-heap query is not exported because the application allocator
contract provides current free space rather than a lifetime low-water mark.

Optional [native EAP](../tests/native-eap/README.md) and
[native WPS](../tests/native-wps/README.md) host regression harnesses compile the
canonical C against controlled dependencies. They cover input bounds, fragment
and allocation ownership; hardware cryptography, scheduling and stack depth
require separate validation. Normal Cargo builds do not run them.

## Reproduction

[`rebuild.py`](rebuild.py) is the complete ordered source, include, compiler-option
and binding manifest. Each modified translation unit maps directly to its pinned
SDK original in `PATCHED_SOURCES`; the AES header is selected beside its canonical
translation units. The original driver revision above records their derivation.
A historical driver checkout, source generator and normalization patches are not
required. Original notices and modification notices remain in the canonical files.

Run explicitly with Python 3.8+, the clean pinned SDK checkout and the exact
Xtensa toolchain. The toolchain directory contains `bin/xtensa-lx106-elf-gcc`
and `xtensa-lx106-elf/include/`. The output must be outside the SDK and vendor
trees; nothing is downloaded or installed into either source tree.

```sh
python3 esp8266-wifi-sys/vendor/rebuild.py \
  --sdk /path/to/ESP8266_RTOS_SDK \
  --toolchain /path/to/xtensa-lx106-elf \
  --output /path/to/disposable-native-output
```

The script compiles the canonical inputs directly, creates all three archives
and compiles `abi_layout.o`. Member names depend only on source-list order and
basenames. Compiler prefix maps remove checkout paths, and `ar crsD` fixes archive
timestamps, ownership and modes. Repeat in another output directory and compare
the three archives before importing them into `libs/`; configuration changes
always require rebuilding all three. Preserve the documented library link order.

Bindings are checked in and normally need no regeneration. To regenerate them
with the same command, add `--bindings /path/to/bindgen-0.70.1` and set
`LIBCLANG_PATH` to an Xtensa-capable libclang installation. The script writes
`bindings.rs` into the output directory for review; it never replaces the checked-in
file automatically. Public Wi-Fi, event, provisioning and supported system
interfaces remain allowlisted; the promiscuous bitfield types are maintained by hand.

The closed SDK archives, linker scripts and newlib archives are direct copies
from the pinned input directories described above. Preserve them and their
licenses when importing rebuilt derived archives. Regenerate `SHA256SUMS` after
updating canonical inputs, artifacts or this manifest. Normal consumers can
verify the snapshot without an SDK checkout:

```sh
(cd esp8266-wifi-sys/vendor && sha256sum --check --strict SHA256SUMS)
```

## Licensing

`licenses/` contains the upstream licenses for the ESP8266 RTOS SDK and the
third-party lwIP, mbedTLS, wpa_supplicant, and newlib code included in the
archives. The crate's Rust code remains dual-licensed under MIT or Apache-2.0.
