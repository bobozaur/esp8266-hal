# Optional native WPS regression test

Run explicitly on Linux x86_64 with Python 3.8+, host GCC/glibc, an unmodified
ESP8266 RTOS SDK checkout at `da7ce37be424d71874241ca9f17cf8da0e3cdb70`, and
the newlib headers from the pinned Xtensa GCC 8.4 toolchain:

```sh
python3 esp8266-wifi-sys/tests/native-wps/run.py \
  --sdk /path/to/ESP8266_RTOS_SDK \
  --sysroot /path/to/toolchain/xtensa-lx106-elf \
  --build-dir target/native-wps
```

`--sysroot` must contain `include/sys/reent.h` and `include/stdio.h`. The host
compiler is selected with `--cc`; the Xtensa compiler itself is not executed.
From an unpacked sys crate, use `tests/native-wps/run.py` instead of the
workspace-prefixed script path above.
Omit `--build-dir` to discard temporary objects after the run. This test is not
part of ordinary Cargo builds or tests. It fetches nothing, generates no SDK
configuration, and writes nothing into the SDK or production sources.

The runner compiles the current canonical `esp_wps_patched.c` and
`wps_attr_build_patched.c` directly, using canonical `sdkconfig.h`. It also
compiles the pinned SDK's real WPS IE builders, attribute parser, EAP message
builder, and `wpabuf` implementation. No production function is copied into the
fixture. The allocator, SDK calls, RTOS queue/task boundaries, and enrollee
cryptographic processing are controlled test dependencies.

Coverage includes initialization allocation failures and retry in both PBC/PIN
modes, partial cleanup, copied IE/callback ownership, first-credential selection,
configuration rejection, zero-allocation completion, value queue/coalescing,
failed start/disable and retry, stale queued work, fragment bounds and lifetime,
response allocation failures, and scan selection/error handling. Guard pages
make reads past exact malformed-input lengths fail; the tracked allocator checks
leaks, double frees, and writes past requested sizes. A valid fragmented message
must reach the enrollee dependency with exactly the expected bytes.

The fixture intentionally retains the boot-owned API mutex. Its task/queue mocks
check transaction ownership and value copying, not the target allocator's
internal allocation count or scheduler timing. Diffie-Hellman and enrollee message
processing are dependency boundaries, so a pass does not qualify cryptography,
radio protocols, IRQ/NMI behavior, target ABI/layout, or hardware stack headroom.

The native event parameter is 32 bits. The Linux host bridge uses `ucontext`,
`MAP_32BIT` stacks, and a non-PIE executable to keep real control pointers within
that range. Guarded inputs assume 4 KiB pages. This fixture does not currently
support other host operating systems or architectures. The newlib/lwIP header
compatibility define `SSIZE_MAX` is test-only; production configuration is
unchanged.

Compiler diagnostics stay enabled with `-Wall`. The pinned newlib/SDK headers
produce known host declaration/attribute warnings, and the canonical WPS code
warns about the intentional 32-bit event-pointer conversions described above.
The fixture does not suppress those warnings or change production code to
accommodate the host.
