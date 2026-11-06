# ESP8266 native boundary

Normal firmware builds use the checked-in ABI declarations and archives. They
need no SDK checkout, bindgen, libclang, Python, network access or C compilation.
Final linking uses the pinned ESP8266 toolchain. The HAL owns tasks, queues,
interrupts, allocation, events, timers, initialization and packet ownership in Rust.

## Retained inputs

| Inputs | Origin and responsibility |
| --- | --- |
| `libcore.a`, `libnet80211.a`, `libpp.a`, `libphy.a` | Unchanged SDK radio, MAC and PHY binaries; their implementation sources are not supplied in the pinned SDK. |
| `libclk.a` | Unchanged SDK RTC/RF binary. The PHY directly calls its sleep/wakeup routines and PP calls `phy_open_rf`; these remain required even though the HAL does not expose SDK CPU light sleep. |
| `libespnow.a` | Unchanged SDK ESP-NOW binary, linked only with `esp-now`. |
| `libsmartconfig.a` | Unchanged SDK provisioning decoder, linked only with `smartconfig`; Rust owns event delivery and the application owns acknowledgement. |
| `libwpa_supplicant.a` | Locally derived open C WPA/AP/WPS/EAP implementation, including its crypto adapters and required ownership repairs. |
| `libmbedtls_all.a` | Locally derived open C TLS/X.509/RSA/P-256 implementation used by the retained security protocols. |
| `libc.a` | Unchanged pinned-toolchain newlib. Native formatting and string operations remain real implementations; allocation, time and descriptor syscalls use Rust. |
| `ld/esp8266.rom.ld`, `ld/esp8266.peripherals.ld` | Unchanged SDK ROM and peripheral symbol maps. ROM memory/string/arithmetic routines are chip firmware, not vendored C source. |

The two locally derived security archives are an explicit remaining C dependency,
not proprietary radio blobs. Replacing them requires WPA/WPS state machines and
TLS 1.2 PEAP/EAP-TLS with key export, supported certificate profiles and recoverable
allocation failure. A qualified Rust replacement preserving those contracts has
not been established. Keeping these snapshots preserves the existing capabilities;
it does not complete a closed-binaries-plus-Rust implementation.

The generic SDK support archive, Xtensa `libhal`, duplicate `librtc`, and separate
`libm` are absent. Rust supplies the only required libhal counter helper; the
retained newlib archive already supplies formatting's `frexp`. There is no native
lwIP stack, SDK VFS registry, SDK event registry, socket adapter or SDK startup
implementation. Native code still needs the HAL's blocking, interrupt-exclusion,
allocation and callback-lifetime contracts. Compiler builtins and chip ROM remain
separate from the radio boundary.

## Provenance

- SDK: <https://github.com/espressif/ESP8266_RTOS_SDK>
- SDK commit: `da7ce37be424d71874241ca9f17cf8da0e3cdb70`
- Original driver derivation: `2624d1c959f71b98ff93c4f611f987a9c3d9fc71`
- Security snapshot base: `ec403f845a375a2c0771ee6a1790934a797ed34b`
- C toolchain: Xtensa GCC 8.4.0, crosstool-NG `esp-2020r3-49-gd5524c1`
- Initial ABI declaration generator: rust-bindgen 0.70.1

The SDK binaries and linker scripts are byte-for-byte copies from
`components/esp8266/{lib,ld}` of that SDK. `libc.a` is the toolchain's unchanged
newlib archive. `VERSION` records the radio component revisions. The mbedTLS
archive is unchanged from the security base above. The WPA archive preserves
that base's ordered members, with only `048-tls_mbedtls.o` and `060-esp_wpa2.o`
recompiled from the current canonical sources to correct TLS 1.2 EAP key export
and response-cache validity. Every other member remains byte-identical.

The base's ordered source lists, flags and deterministic archive recipe remain
available in repository history as that commit's
`esp8266-wifi-sys/vendor/rebuild.py`. The two replacements use its pinned compiler,
`SDK_INCLUDES`, `WPA_DEFINES`, canonical-first includes, path-prefix mappings and
`-Os -ffunction-sections -fdata-sections -mlongcalls -w` flags. Deterministic
`ar rDs` replaces just those members in place. There is no maintained SDK rebuild
or binding-generation workflow in this crate.

`patched-sources/` preserves the canonical modified security translation units,
their original notices and the configuration used for these exact snapshots.
They are not compiled by normal builds. Headers and declarations in
`src/bindings.rs` have been narrowed to the actual driver ABI; unused generic
event, IP-stack and crypto-table declarations are absent. `tests/abi_layout.c`
independently checks layouts and bitfield images against the pinned target
headers, including newlib's storage and timeval conventions.

The pinned station-info header disagrees with its radio binary. Its
`wifi_sta_info_t` puts PHY flags at byte 6, but
`libnet80211.a(ieee80211_api.o):esp_wifi_ap_get_sta_list` writes an extra byte at
offset 6 and PHY flags at offset 7 (section instructions `0xa5` and `0xd9`). Rust
follows the retained binary, preserving the 12-byte entry and 124-byte list.
The C fixture retains the header's byte-6 image as discrepancy evidence; a Rust
regression checks the binary's byte-7 output with conflicting adjacent bytes.
Station configuration flags occupy byte 126, confirmed by the C image and the
binary's `esp_wifi_get_config` halfword accesses at instructions `0x12b`, `0x134`
and `0x148`. Their six-byte tail includes reserved storage; forcing u32 alignment
would place the flags at the wrong offset. Scan-record WPS flags remain at byte
64, matching both the header image and `esp_wifi_scan_get_ap_records`' word store
at instruction `0x213`.

`SHA256SUMS` covers the retained inputs and declarations. Consumers can check it
without an SDK checkout:

```sh
(cd esp8266-wifi-sys/vendor && sha256sum --check --strict SHA256SUMS)
```

## Retained security contracts

The canonical TLS configuration disables optional Camellia, mbedTLS DES and
Blowfish. TLS retains AES CBC/CCM/GCM with RSA, ECDHE-RSA and ECDHE-ECDSA; WPA's
separate DES implementation supports MSCHAPv2. The SDK-default TLS server
configuration remains enabled because removing it caused measured enterprise
client handshake timeouts; the EAP API implements client handshakes only.

TLS record content is limited to 2,048 bytes in each direction, MPI/RSA to
2,048 bits and elliptic-curve groups to 256 bits. EAP fragmentation does not raise
the record limit. Certificate chains and cryptographic work share the application
heap with radio traffic; these limits alone do not ensure an arbitrary chain fits.

The TLS backend accepts a null connection during teardown. Handshake allocation
failures remain recoverable. EAP reserves a declared reassembly flight once; TLS
borrows worker-owned input synchronously and clears the borrow on every return.
Consumed tails of owned reassembled flights can be compacted. Output capacity
grows in 256-byte increments with at most 255 spare bytes. Idle record buffers
are released and restored around shared cryptographic operations.

Raw client certificate/key/password material remains available until use. Parsing
occurs at `CLIENT_CERTIFICATE` after server verification; parsed CA, client
certificate and key allocations are released after their last uses. Parsing
consumes every DER certificate and rejects trailing malformed bytes and partially
invalid PEM bundles. Fatal TLS errors remain failures even without output; a
nonfatal empty response still requests an ACK.

EAP key export uses the negotiated TLS PRF: MD5/SHA1 for TLS1.0/1.1, SHA384 for
TLS1.2 SHA384 suites, and SHA256 for other TLS1.2 suites, including CBC-SHA.
It borrows the saved client/server randoms in that order without another copy.
An EAP identifier denotes a retransmission only while its response remains
cached. New requests release the old response before processing, so ignored
requests and allocation failures cannot replay an earlier response.

Enterprise receive processing uses its dedicated worker with a 4,096-byte stack
request plus the Rust runtime's context storage. Packet size or memory pressure
must not move TLS work onto the smaller Wi-Fi stack. Commands are queued by value;
receive headers and payloads share one allocation. Partial startup rolls back;
teardown closes admission and waits for worker and producer completion before
freeing peer state. Scheduler-suspended callbacks request exit without freeing
live state. Credentials survive disable for reuse.

WPS uses one cleanup path for partial initialization and normal teardown, queues
events by value, and applies checked credentials once before publishing success.
Its attempt owns one checked fragment-reassembly buffer. Terminal events release
that buffer and quiesce pending work. Temporary IEs go directly to the native
copying setter. The native EAP and WPS host regression fixtures under `tests/`
cover these ownership and failure contracts without rebuilding firmware archives;
cryptography, scheduler timing and stack depth need target validation.

## Licensing

The upstream SDK, wpa_supplicant, mbedTLS and newlib notices remain in `licenses/`,
along with the SDK's lwIP notice. These notices do not imply that a native IP stack
is present. Modified security sources preserve their original and modification
notices. The Rust ABI crate's original code is available under MIT or Apache-2.0;
redistributing archives does not change their upstream terms.
