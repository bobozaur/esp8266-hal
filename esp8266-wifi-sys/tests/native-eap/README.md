# Optional native EAP regression tests

Run explicitly on Linux x86_64 with 4 KiB pages, Python 3.8+, host GCC/glibc,
an unmodified ESP8266 RTOS SDK checkout at
`da7ce37be424d71874241ca9f17cf8da0e3cdb70`, and the pinned Xtensa GCC 8.4
newlib headers:

```sh
python3 esp8266-wifi-sys/tests/native-eap/run.py \
  --sdk /path/to/ESP8266_RTOS_SDK \
  --sysroot /path/to/toolchain/xtensa-lx106-elf \
  --build-dir target/native-eap
```

`--sysroot` contains `include/sys/reent.h` and `include/stdio.h`.
From an unpacked sys crate, use `tests/native-eap/run.py`. Omit `--build-dir`
for temporary output. No network, target compiler, Cargo build, SDK writes or
device access is involved. Normal Cargo builds do not run this fixture.

The runner compiles current canonical `esp_wpa2_patched.c` and
`eap_tls_common_patched.c`, plus the pinned SDK's real `eap.c`, EAP message builder and `wpabuf.c`.
It imports
the existing native-WPS runner's header/feature manifest; those tests are not
modified. Host `-fcommon` matches GCC 8's default for the tentative globals in
the pinned SDK's `eap.h`. Other SDK/newlib host warnings remain visible.

Each case runs in a separate process:

- `short`: guard pages follow exact inputs. Physical headers shorter than four
  bytes, null requests and declared EAP lengths zero through five with only a
  four-byte header must be rejected without reading a missing type byte or
  reaching transmit dependencies.
- `cached`: retransmission aliases the cached response; an EAPOL send failure at
  BSSID lookup must release it exactly once and clear the cache.
- `distinct`: the same pre-transmit failure with a newly produced response must
  release both distinct buffers once.
- `fragments`: a 1,537-byte phase-two input in four fragments reserves its full
  capacity once. The decrypt dependency must receive all bytes, including the
  final eight-byte fragment. First-allocation failure must reset input state
  without leaking or calling decryption.
- `empty-output`: the actual handshake helper must preserve a fatal TLS result
  when the TLS dependency returns an empty buffer. A normal empty buffer still
  requests an ACK. Both paths must free the buffer and leave no output or input
  allocation behind.
- `lifecycle-init`: discover peer-initialization allocation sites from a successful
  run, fail each in turn (including partial credential configuration, command
  queue, completion semaphore and task), and require complete cleanup and retry.
  A public `esp_wifi_get_mac` error must also roll back before resources publish;
  a later successful request must copy the returned station MAC into the peer.
- `lifecycle-queue`: commands are copied by value without per-command allocation;
  a failed queue send must roll back admission so later commands run normally.
- `lifecycle-suspended`: scheduler-suspended teardown must request worker exit
  without blocking or freeing live state. Initialization cannot overwrite that
  state; ordinary disable later completes cleanup and permits a fresh enable.
- `lifecycle-active`: a controlled method yields while its RX caller waits for
  normal completion. Suspended teardown closes admission but retains the method
  and its credentials; the worker finishes before ordinary disable frees them.
  Deletion must not consume or give the normal command completion semaphore.
- `lifecycle-credentials`: configure credentials once, then enable, process a
  request through the controlled method and disable three times. The actual SDK config
  and blob accessors must still expose the same identity, password and material.

The tracked allocator checks duplicate frees and writes beyond requested sizes.
Protocol methods, the public MAC getter, BSSID lookup, native transmission,
TLS handshake and TLS decryption are controlled dependencies. No production function is copied or
included into the fixture, and no static visibility is changed. A pass does not qualify TLS
cryptography, native-TX error propagation, real scheduler timing, target ABI,
stack depth, IRQ/NMI or radio behavior. Lifecycle schedules use host `ucontext`
and `MAP_32BIT` stacks to keep control pointers within the SDK's 32-bit ABI.
The native API mutex and configured credentials deliberately survive disable;
allocation checks compare against that persistent ownership baseline. Reassembly
tests initialize the public internal input counters; they do not test EAP TLS-flag
decoding.

Use repeated `--case NAME` options to select cases. `--request-source PATH`
and `--tls-source PATH` select old request or TLS-common implementations for
explicit negative controls. Canonical headers remain unchanged.
