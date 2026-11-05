# Native runtime adaptations

The ESP8266 radio uses pinned Espressif binary libraries; ESP32 radio binaries
cannot replace them. The exact SDK commit, archive inputs, generated bindings,
licenses and reproduction procedure remain in the
[vendor manifest](../esp8266-wifi-sys/vendor/README.md). The SDK ABI is preserved.
The three derived archives, `libesp8266_sdk.a`, `libwpa_supplicant.a` and
`libmbedtls_all.a`, are rebuilt from the canonical sources and SDK configuration.
A null-connection guard in the TLS backend fixes cleanup after initialization
failure. The configuration uses SDK defaults that disable optional Camellia,
mbedTLS DES and Blowfish; AES-based TLS and WPA's separate DES implementation for
MSCHAPv2 remain. The closed Espressif radio archives remain byte-for-byte copies
of the pinned SDK inputs. Normal builds need no SDK checkout, C compilation or
bindgen.

## Link boundary

[`ld/wifi.x`](../ld/wifi.x) selects the SDK syscall object through `EXTERN(abort)`
so newlib's separate nano allocator is not selected. The SDK's reentrant
allocation entry points reach the Rust `heap_caps` ABI and the application's
explicit `WifiAllocator` provider.

Three GNU ld assignments intentionally redirect SDK symbols to Rust functions
with the same native signatures:

| SDK symbol | Rust responsibility |
| --- | --- |
| `esp_timer_get_time` | Runtime-owned monotonic time |
| `ieee80211_deliver_data` | SDK packet delivery into registered station/AP callbacks |
| `pm_wait4wakeup` | Bounded PHY settling delay in place of polling unconfigured IDF RTC wake bits |

These substitutions have explicit linker roots. The generic
`--allow-multiple-definition` option is removed; unrelated duplicate definitions
remain link errors. Obsolete delay, reentrant-allocation, pbuf and startup wrappers
were removed where the selected SDK implementation already supplies the behavior.
`uart_tx_wait_idle` uses the pinned SDK C implementation; no fixed-spin Rust
replacement remains.

## Packet ownership and transmit admission

Both station and AP callbacks transfer a native receive-buffer lease. The pinned
SDK's `tcpip_adapter_lwip.c` uses the same deferred-release custom-pbuf contract
for both interfaces: release the SDK buffer through
`esp_wifi_internal_free_rx_buffer`, or free the heap buffer when no SDK handle
exists. Rust queues that ownership directly and releases it once after consumption
or rejection. AP reception needs no additional frame pool or callback copy.
Queued packets drain before native teardown, and checked-out packets make radio
stop return `Busy`. Holding packets longer consumes native RX capacity, so
applications should release them promptly.

Admission, both RX rings and lease counts share one state lock. Packet admission
and checkout need one CPU/NMI exclusion each. Disconnect and stopped events close
admission, release queued frames outside the lock, then notify consumers. The
native event loop serializes these events, so a following connection event cannot
reopen admission during the drain.

The common native TX copy reserves at least 60 payload bytes and initializes the
unused tail, while retaining the caller's logical frame length. The pinned
`ieee80211_output_pbuf` reads fixed IP-field offsets even for shorter Ethernet
frames, including valid 18-byte EAPOL-Start packets. Both heap and reused fallback
storage therefore need initialized lookahead; inputs below the 14-byte Ethernet
header are rejected.

AP RX and TX use direct radio callbacks and descriptors. The private lwIP netif
and its persistent TCP/IP task are removed: no radio or protocol archive consumes
that interface, and its output callback only discarded packets. Application IP
addressing remains entirely within the selected network stack. Two-board AP and
mixed DHCP, retained-packet pressure, peer rejoin and stop/restart validate this
path; the refactor plan records the qualification scope.

The native `ieee80211_output_pbuf` returns descriptor exhaustion as byte-valued
`ERR_MEM` (positive 255), while the Rust static-TX-buffer fallback returns signed
`-1`. Both map to `WouldBlock`, preserving an adapter's pending frame for retry.
Other native errors retain their original meaning.

Embassy retries pending TX after 1 ms. Native completion callbacks run before
descriptor recycling, and shared management/AP traffic can consume capacity
without an application completion event. A completion-only wakeup would therefore
be insufficient. The existing timer avoids introducing extra notification state
while still guaranteeing another attempt.

Native queue headers use bounded byte-sized task IDs for their waiters: 56 bytes
instead of 168 on Xtensa, preserving priority and equal-priority slot selection.
Task deletion detaches the old stack and resets the complete slot under scheduler
exclusion before making it reusable; freeing the detached stack touches no slot
metadata. The FreeRTOS timer daemon owns software timers used for WPA PMKSA
expiration. The SDK's separate ETS timer list is driven by FRC2 interrupt 10,
which posts event 16 to the Wi-Fi task. That task dispatches `ets_timer_proc`;
the HAL does not add another worker or poll the same list from another context.

WPS uses one cleanup path for partial initialization and normal teardown. Its
worker queue stores events by value, credentials are applied once from checked
stack storage, and failed configuration cannot report success. Receive fragments
have explicit session ownership and checked bounds. Every terminal failure or
timeout disarms timers and discards partial input before a later attempt. Scan
parsing borrows the received IE and checks its parser allocation; the 500-byte
parser state stays off the small radio-task stack.

## NMI entry and return

The earlier NMI handler omitted the SDK's final-`RFI` guard. If `EPC3` points to
that return instruction, saving it again overwrites the original saved
`EPC3`/`EPS3` and can make the handler return to itself. The corrected handler
preserves the existing return state in that case.

The `xtensa-lx-rt` 0.12 default vector also stores `A0` in `EXCSAVE1`, which
ordinary exception entry uses. An NMI during that entry can destroy its saved
state. Wi-Fi now selects a HAL-owned vector using `EXCSAVE3`, paired with the
corrected handler. The [Wi-Fi vector layout](../ld/wifi-exception.x) preserves the
other vector offsets and fallbacks. This needs no vendored crate, dependency-source
edit or blob patch. Linked-ELF inspection and repeated scan/recreation runs on two
boards cover the corrected integration.

## Allocation and reinitialization

The pinned native code ignores allocation failures in hardware/RX setup
(`wdev`/`esf`), packet-processing startup (`pp`), miscellaneous state and Wi-Fi NVS.
Returning null at those call sites can cause a native fault instead of a usable
initialization error.

Before entering the SDK, Rust reserves nine ordinary allocations through the
application provider, each using the normal C allocation header and ownership
rules. Handoff matches the audited allocation size, capabilities and source
file/line, or the corresponding queue/task boundary. Each reservation transfers
once; unused reservations are released. This is a bounded adaptation of the
pinned SDK, without a separate arena, vendor patch or new crate. Changes to those
native allocation sites require re-auditing the matching handoff and dimensions.

Hardware buffers and the PP queue/task remain allocated across native
deinitialization. Their dimensions therefore lock when first native entry
begins, including when a later initialization phase fails. Subsequent
`Wifi::new` calls with different native RX buffer, RX packet or TX buffer counts
return `InitError::BufferConfigurationLocked`. Reset is required to change them.
Rust `QueueConfig` storage is independent and may change between Wi-Fi instances.
Allocation failure before native entry does not lock the native dimensions.

A same-configuration recreation reserves only the temporary 1024-byte Wi-Fi NVS
buffer from that set. The separate 884-byte PHY workspace is needed only for the
first attach.

TLS and EAP handshake allocations use the application provider directly. The RSA
reservation pool and its C hooks have been removed. The nine initialization
reservations above remain necessary for the audited native startup allocations
whose failures the SDK does not check.

## Shared-heap exclusion and idle stack

Wi-Fi NMI paths can allocate from the application's shared heap. Masking ordinary
interrupts alone therefore cannot protect an allocator operation. In Wi-Fi
builds, the HAL's `critical-section` implementation masks both ordinary interrupts
and NMI, then restores the complete processor status and NMI-enable register from
a `u64` token. Each nested section preserves its caller's state. Non-Wi-Fi builds
do not access the NMI register. Protected allocator operations must stay bounded
and must not wait for an interrupt. This uses the existing critical-section API,
without a new allocator wrapper or dependency patch.

The repeated scan/recreation stall was an idle-task stack overflow. Its previous
512-byte stack plus 176 bytes of context storage allocated 688 bytes; an interrupt
save frame reached the fourth canary at allocation base plus 12 bytes. Restoring
the pinned SDK's 1024-byte idle stack allocates 1200 bytes in total, costing 512
additional DRAM bytes. This correction, rather than changing radio buffer counts,
addresses that observed stall.

## Crypto data placement

Mapped flash supports aligned word reads. Audited cipher, public-key and OID
metadata lives there; its byte strings remain in DRAM. Cipher-suite descriptors
also remain in DRAM because their flags are read as bytes. The previous flash
placement caused a hardware fault during TLS key derivation. Camellia's byte
S-boxes must likewise stay in DRAM if that optional algorithm is enabled again.
Digest metadata stays in DRAM because a compiled getter uses a byte load.

## TLS memory limits

The canonical configuration has fixed TLS and cryptographic size limits:

| Budget | Limit |
| --- | ---: |
| Incoming TLS record content | 2,048 bytes |
| Outgoing TLS record content | 2,048 bytes |
| MPI integer size / RSA modulus | 256 bytes / 2,048 bits |
| Elliptic-curve group size | 256 bits |

Record buffers also require protocol overhead. Certificates, parsed chains,
cryptographic work and radio traffic share the remaining application heap. EAP
fragmentation does not raise the 2 KiB TLS record-content cap. A small leaf alone
does not establish compatibility: the server can send additional certificates
in the same record. Arbitrary chains and applications with less available heap
are not covered by the profiles below.

EAP reassembly reserves the declared flight size once. TLS consumes the
worker-owned request without another packet copy and compacts an owned input
tail after most of the flight has been consumed. Static client certificate,
private-key and password bytes remain borrowed for the connection lifetime;
parsing waits until `CLIENT_CERTIFICATE`, after server verification. Parsed CA,
client certificate and client key allocations are freed after their respective
last uses. Shared helpers release and restore idle input/output record buffers
around pre-encryption cryptographic work. Output growth rounds capacity to
256-byte increments, leaving at most 255 spare bytes.
Certificate parsing consumes complete concatenated DER bundles and rejects
malformed trailing bytes and partially invalid PEM bundles. Fatal TLS status is
propagated to EAP even when output is empty; nonfatal empty output requests an ACK.

Every EAP packet runs on the dedicated WPA2 worker; packet size and allocation
failure must not move cryptographic work onto the smaller Wi-Fi task stack. The
worker requests a 4,096-byte stack, with another 176 bytes allocated for the Rust
runtime context. Commands are stored by value. Initialization publishes the peer
only after all dependencies exist; each failure releases partial ownership.
Teardown closes admission, then waits for the worker and returning producers
before freeing method, configuration and TLS state. A native callback with the
scheduler suspended may request exit but retains those owners until cleanup can
complete. Ordinary stop releases handshake state and preserves credentials for
restart; replacing the mode clears those credentials.

The library reserves 8 KiB for the application stack by default.
Applications may override the absolute `STACK_SIZE` linker symbol; the linker
checks that it is positive, 16-byte aligned, within DRAM and clear of static data.
The `wifi_eap` example defines that symbol with an assembler assignment to reserve
4 KiB. That example budget does not qualify other applications' stack depths.

The current interoperability profiles complete EAP-TLS authentication and the
WPA four-way handshake:

| Profile | Server Certificate message | Client Certificate message |
| --- | ---: | ---: |
| P-256 leaves with intermediate certificates | 1,095 bytes | 1,049 bytes |
| RSA-2048 server leaf, client leaf and intermediate | 971 bytes | 1,867 bytes |

Instrumented runs with 4 KiB application and WPA2 worker stacks measured these
minimum unused stack amounts:

| Profile | WPA2 worker | Application |
| --- | ---: | ---: |
| P-256 intermediate chains | 956 bytes | 1,600 bytes |
| RSA-2048 client chain | 1,340 bytes | 1,584 bytes |

The ordinary, uninstrumented RSA client-chain example also completed EAP success
and the four-way handshake in 28.4 seconds. These measurements establish the
listed profiles and stack budgets, without a throughput or latency improvement
claim. P-256 EAP-TLS, PEAP and wrong-password rejection also pass with 64-byte
EAP fragments, including fragmented encrypted PEAP phase-two messages. Wrong CA,
trailing DER, partially invalid PEM and malformed or oversized client material
are rejected; each recovery fixture then connects twice without rebooting.
Stop/start and ordinary reconnect each pass three connections using one credential
configuration. These checks establish recovery capability, without a guarantee
of first-attempt radio success.

Optional [native EAP](../esp8266-wifi-sys/tests/native-eap/README.md) and
[native WPS](../esp8266-wifi-sys/tests/native-wps/README.md) regression harnesses
exercise canonical C ownership, malformed input, allocation failure and fragment
handling. They do not qualify target cryptography, scheduling, stacks or radio
behavior.

## Provisioning and polled radio interrupts

SmartConfig uses the hardware NMI path. Its example keeps the provisioning
session alive through DHCP and ESPTouch acknowledgement, then explicitly stops it.
Networking after stop is tested separately. The current qualification completes
a fresh DHCP exchange after stopping provisioning and waiting 12 seconds, with
new transaction identifiers confirmed in the packet capture.

Promiscuous reception still polls the native FIQ handler from the timer path.
That handler masks the NMI-enable register and relies on its hardware interrupt
return path to reopen it. Polling must therefore save and restore the gate through
the HAL critical section. Without that boundary, polling can leave hardware
reception disabled. A 32-frame capture passes with the protected polling path;
removing promiscuous polling remains unqualified.

## Retained adaptations

| Adaptation | Reason retained | Evidence needed before removal |
| --- | --- | --- |
| Scheduler, interrupt and newlib adapters | Native tasks require preemption, blocking primitives, context layout and newlib state. | Task/timer progress, peripheral interrupt coexistence and startup allocation failures. |
| Allocation and volatile NVS adapters | C allocation requires layout recovery and free-DRAM statistics. Volatile storage preserves PHY calibration across radio restarts. | Exhaustion/rollback, shared allocator behavior and repeated initialization. |
| PHY, cache and system adapters | Calibration, clocks, MAC addresses and cache-off execution are chip-specific ABI requirements. | Cold/repeated initialization, cache-off call paths and flash/radio coexistence. |
| Narrow SDK service stubs | Linked native objects reference selected IDF services that are absent from the application stack. | Audit the actual consumers and validate the affected feature before removal. |
| Historical AES, WPS and enterprise patches | The snapshot includes aligned AES tables, provisioning fixes and TLS certificate/buffer lifetime adaptations. | Encrypted ESP-NOW, WPS PBC/PIN and enterprise authentication with appropriate peers and allocation pressure. |

Older investigations support retaining the historical patches. They do not
qualify the new runtime, allocator or controller. An unavailable peer or
authentication fixture leaves that behavior unverified.

## Cost and current evidence

The optional `wifi` feature remains necessary: `allocator-api2` enables the
upstream `alloc` dependency, which requires an application Rust global allocator
even when Wi-Fi is unused. The committed refactor's representative dormant-Wi-Fi build adds
7,435 bytes of BSS over the bare example. Global registration does not initialize
the heap or runtime.
`Runtime::init` still receives an explicit permanent `WifiAllocator`; applications
may share it with their global allocator, as the examples do.

The original and committed refactor (`f31504b`) scan images have these linked sections:

| Linked section | Original | Refactored |
| --- | ---: | ---: |
| IROM bytes | 321,936 | 370,736 |
| IRAM, including vectors | 5,710 | 5,910 |
| Read-only DRAM bytes | 21,736 | 22,880 |
| Initialized writable DRAM bytes | 1,249 | 1,121 |
| BSS bytes | 15,512 | 17,000 |

These are section measurements, not total RAM or a performance result. Runtime
heap, permanent application stack and SDK task stacks require separate accounting.
The refactor does not establish a general reduction in code size or runtime cost.
The original linker heap extent exceeded the corrected DRAM map, so it is not a
valid available-memory comparison.

Committed refactor release images (`f31504b`), built with the pinned Xtensa toolchain and thin LTO:

| Image | IROM | IRAM + vectors | Static DRAM | Heap region before runtime initialization |
| --- | ---: | ---: | ---: | ---: |
| Bare HAL | 8,682 | 1,493 | 2,029 | 88,080 |
| Wi-Fi enabled, unused | 26,738 | 5,450 | 11,664 | 78,436 |
| Scan | 370,736 | 5,910 | 41,001 | 49,096 |
| TCP server | 449,200 | 6,158 | 60,753 | 29,344 |
| AP + station | 451,112 | 5,970 | 61,629 | 28,480 |
| All-feature TCP server | 569,720 | 6,118 | 73,301 | 16,800 |
| Enterprise PEAP | 477,784 | 5,974 | 50,533 | 39,576 |
| Enterprise EAP-TLS | 478,016 | 5,974 | 51,093 | 39,016 |

All values are bytes. Static DRAM includes read-only data, initialized data and
BSS. Heap-region figures exclude the permanently reserved 8 KiB application
stack, but precede SDK task, packet and queue allocations; they are not free heap
while networking is active. These historical rows used the 8 KiB application
stack reservation, before the current EAP example's 4 KiB override. Enterprise
rows include the compact DER certificate fixtures used for authentication;
different credentials change these figures. The all-feature TCP example fails
initialization cleanly with `OutOfMemory`; it is an exhaustion check, not an operational memory
configuration.

The packet optimization commit (`51d8b41`) changed representative images as follows,
with the same feature selections and toolchain:

| Image | IROM change | Static DRAM change | Heap-region change |
| --- | ---: | ---: | ---: |
| AP-only | −3,240 | −1,720 | +1,720 |
| AP + station | −1,928 | −1,640 | +1,640 |
| SmartConfig | −2,296 | −16 | +16 |
| TCP server | −2,120 | −24 | +32 |

All values are bytes; linker alignment explains the TCP heap-region rounding.
The AP examples now use the raw interface directly, removing unused smoltcp
staging. Removing the AP receive pool additionally removes preallocated frame
payload of 1,024 bytes (2 × 512) and 1,344 bytes (2 × 672), plus pool bookkeeping.
Those allocation savings are separate from the linked section changes. Native
frames remain live until consumed, so active free heap and peak packet capacity
depend on application consumption; these measurements do not establish a
throughput improvement. SmartConfig keeps its caller-owned TX buffer on the
existing application stack and introduces no heap allocation.

That commit passed AP/mixed DHCP, retained-packet pressure and peer reconnect,
20 TCP echo connections, separate-host TCP and direct smoltcp DHCP plus five exact
UDP payloads, plain/encrypted ESP-NOW, 32 management-frame captures and 100
create/scan/stop/drop cycles per board. A SmartConfig retry passes provisioning,
DHCP and sender-validated ESPTouch acknowledgement on both boards. Fresh DHCP also
passes after stopping provisioning and waiting 12 seconds. Three of five
optimized-image attempts passed; two failed before credentials were decoded.
The committed-baseline comparison failed association after decoding credentials.
These results confirm working provisioning and post-stop networking paths;
first-attempt reliability and the cause of intermittent failures remain unresolved.

The further simplification batch, compared with `51d8b41` using the pinned
toolchain and each example's required features, changes linked sections as follows
(all values are bytes):

| Image | IROM change | Static DRAM change | Heap-region change |
| --- | ---: | ---: | ---: |
| AP-only | −33,312 | −1,849 | +1,864 |
| AP + station | −31,440 | −2,085 | +2,096 |
| TCP server | −32,456 | −2,297 | +2,304 |
| WPS | −33,264 | −2,257 | +2,256 |

IROM includes flash-mapped code and data; these are linked section measurements,
not throughput results. The removal also benefits station-only images because
the shared controller previously retained the AP bridge's dependencies.

On the same board, a matching standalone AP fixture with no clients has 28,192
bytes of free heap after startup, versus 22,336 at `51d8b41`: a 5,856-byte gain.
Its AP-start allocation falls from 4,024 to 1,208 bytes. These include runtime
allocation effects and must not be added to the section table. The current
fixture completes 256 self-deleting tasks across two producers with no net heap
loss after cleanup; this exercises reuse without claiming deterministic
reproduction of the former race.

The baseline refactor's final two-board stress completed 100 create/start/scan/stop/drop cycles on
each board and rejected changed native buffer counts. Integrated checks cover
AP-only and mixed upstream/downstream DHCP, plain and encrypted ESP-NOW,
promiscuous capture, a separate LAN-host TCP exchange, and 20 byte-exact TCP echo
connections with payloads from 4 to 2048 bytes. Flash erase/write/readback,
Timer1/Timer2 and UART interrupt coexistence also pass.

An all-features integration image reported `OutOfMemory` cleanly where native
startup previously faulted with `LoadProhibited`; that result establishes the
bounded failure case. The latest NMI-exclusion correction passed review and
linked-ELF checks for IRAM placement and call-free critical-section entry/exit.
The [refactor plan](../plan.md#final-results-and-remaining-qualification) records
final stress, lifecycle and cleanup results, including reserved-sector comparison
and the working AP/client pair left on the boards.
Association attempts can report RF failures; examples and the lifecycle fixture
use explicit bounded application retry policies. These checks do not establish
first-attempt association reliability, arbitrary callback/interrupt stack depth,
or independently mapped flash-cache coherence. Those earlier campaigns passed
WPS PBC/PIN, SmartConfig provisioning with DHCP and acknowledgement,
PEAP/MSCHAPv2 with incorrect-password rejection, and compact P-256
client-certificate EAP-TLS with the four-way handshake. Those results predate
the current TLS changes. The refreshed P-256/RSA, PEAP, negative and recovery
checks pass as recorded under [TLS memory limits](#tls-memory-limits). WPS PBC/PIN
and three uninstrumented SmartConfig runs also pass, including fresh DHCP after
provisioning stops. Intermittent transport failures against one AP remain
unexplained. The
[current review results](../plan.md#remaining-interoperability-work) distinguish
refreshed hardware coverage from earlier campaigns.
