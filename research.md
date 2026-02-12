# ESP8266 WiFi Research

## Purpose

- Keep a concise, evidence-backed record of the WiFi driver state.
- Preserve only:
  - current verified runtime state
  - accepted production fixes
  - rejected branches worth not retrying
  - SDK-backed facts that shaped decisions
  - the smallest useful artifact/reference set
- Recover detailed chronology from `git log -- research.md` when needed.

## Environment

- Repo: `/home/bogdan/Coding/Personal/esp8266-hal`
- Board: ESP8266EX
- Flash: `4 MB`
- Crystal: `26 MHz`
- Serial device: `/dev/ttyUSB0`
- App capture baud: `115200`
- SDK tree used for source-of-truth diffs: `/tmp/ESP8266_RTOS_SDK`
- SDK commit in use: `da7ce37be424d71874241ca9f17cf8da0e3cdb70`
- Tooling note: run `source .envrc` before `cargo`, `xtensa-lx106-elf-*`, or SDK-linked tooling.

## Build And Capture Rules

- The `10s` cap applies to runtime test windows, not to build or flash time.
- Do not flash and capture in parallel on `/dev/ttyUSB0`.
- For app output, use `/dev/ttyUSB0` at `115200`.
- On this host, do not pass `dtr=False` / `rts=False` to the `serial.Serial(...)` constructor.
  - Open the port first, then assign `ser.dtr = False` and `ser.rts = False`.
  - Pulse `rts` high, then low.
- Current validated flash flow uses the split `v1` image at:
  - `0x00000`
  - `0x10000`
- Keep probes small.
  - Large diagnostics previously regressed bootability.
  - Large example-local panic/exception handlers also triggered an Xtensa LLVM spill on this toolchain.

## Current Verified State

- Current best verified state is the upstream-dependency build validated in:
  - `/tmp/tcp_server_step437`
  - `/tmp/tcp_server_step439`
- Result:
  - `10/10` completed `tcp_server` reset-to-echo runs in `step437`
  - `10/10` completed `tcp_server` reset-to-echo runs in `step439`
- Final validated build command:
  - `source .envrc && cargo build --release --example tcp_server --features wifi-embassy-net,wifi-sta`
- Final validated runtime properties:
  - uses upstream `smoltcp`
  - uses upstream `embassy-net`
  - uses the repo’s SDK-style station auto-connect flow
  - stays within the `10s` runtime capture budget

## Accepted Production Changes

### `examples/tcp_server.rs`

- Keep `WifiInitConfig::default()`:
  - `examples/tcp_server.rs:139`
- Do not override WiFi buffer counts in the example:
  - earlier reduced/custom counts were not part of the final robust state
- Keep the fixed startup spin removed:
  - `examples/tcp_server.rs:204`
- Keep the current SDK-style connection sequence:
  - `event::enable_sdk_sta_auto_connect(MAX_CONNECT_RETRIES)`
  - `wifi.wait_for_auto_connect_result_blocking(5)`
  - `stack.wait_config_up().await`

### Driver / stack facts that remain true

- `connect_blocking()` must remain a WiFi-link helper on the embassy path.
  - DHCP belongs at `stack.wait_config_up().await`, not inside `connect_blocking()`.
- The final robust result does not require:
  - vendored `smoltcp`
  - vendored `embassy-net`
  - ingress IP/MAC trust

## Final Outcome Of The Latest Investigation

### What solved the real problem

- The final robust state came from keeping the example aligned with the SDK-style flow and removing avoidable timing waste:
  - restore default WiFi init buffers in the example
  - remove the fixed startup spin
- These are the changes that survived hardware validation.

### What did **not** survive validation

- The vendored `smoltcp` experiment with ingress neighbor learning / IP-MAC trust was rejected.
- It improved some successful first-SYN traces, but it regressed total robustness:
  - trust enabled: `7/10` in `/tmp/tcp_server_step435`
  - trust disabled: `10/10` in `/tmp/tcp_server_step437`
- Rebuilding again on normal upstream `smoltcp` kept the result at `10/10`:
  - `/tmp/tcp_server_step439`

## Condensed Evidence Timeline

### 1. Baseline problem narrowed from “TCP server flaky” to “specific startup / first-packet classes”

- Earlier retests showed multiple failure classes:
  - join/association failures
  - slow bring-up that missed the `10s` budget
  - occasional “first SYN seen, no reply” traces
- This ruled out treating the whole problem as a single TCP socket bug.

### 2. SDK-style auto-connect was re-audited and kept

- The repo’s current example already follows the SDK-style flow.
- Reopening speculative `connect_blocking()` branches did not produce a durable fix.
- Durable rule:
  - keep auto-connect at the WiFi/event level
  - keep DHCP waiting at the embassy-net stack level

### 3. Buffer and startup alignment mattered more than speculative stack changes

- Returning the example to `WifiInitConfig::default()` removed join/connect regressions.
- Removing the fixed startup spin eliminated one real late-budget miss:
  - `step427`: `8/10`
  - `step430`: `9/10`

### 4. The “silent first SYN” was measured directly before trying any fix

- The observation-only probe run in `/tmp/tcp_server_step432/run2` proved:
  - the first inbound TCP `SYN` reached the WiFi RX callback
  - the runner forwarded it into the Rust stack
  - the only outbound attempt before timeout was ARP
- That narrowed the old outlier to neighbor-resolution behavior, not “packet lost before Rust”.

### 5. The `smoltcp` trust experiment was source-backed, tested, and rejected

- The experiment used a vendored `smoltcp` feature to learn a neighbor from inbound IPv4 traffic.
- It was based on real SDK/lwIP documentation:
  - `ARP_QUEUEING = 1` is enabled in the SDK lwIP port
  - `LWIP_ETHARP_TRUST_IP_MAC` exists as a documented Kconfig option
- But the experiment did not survive A/B testing:
  - trust enabled gave better traces in some successful runs
  - trust disabled gave the better full-system result

### 6. Final controls proved the simple path was the right one

- `step437`:
  - upstream logic path
  - trust disabled
  - `10/10`
- `step439`:
  - upstream `smoltcp` again, no vendored override
  - `10/10`
- Conclusion:
  - keep the example/runtime alignment fixes
  - discard the vendored neighbor-learning branch entirely

## SDK-Backed Facts Worth Keeping

### WiFi / connect flow

- The SDK-style station bring-up remains the source of truth for the example shape.
- Keep comparing one subsystem at a time against the SDK rather than inventing parallel flows.

### lwIP neighbor-resolution facts

- `/tmp/ESP8266_RTOS_SDK/components/lwip/port/esp8266/include/lwipopts.h`
  - `ARP_QUEUEING = 1`
- `/tmp/ESP8266_RTOS_SDK/components/lwip/Kconfig`
  - `LWIP_ETHARP_TRUST_IP_MAC` exists
  - default is `n`
- Important interpretation:
  - IP/MAC trust is an official option, but not the strongest “SDK default” baseline
  - ARP queueing is the stronger default fact

### Timer / callback tracing facts

- SDK headers alias public `ets_timer_*` names to `os_timer_*`.
- The blob uses both families along the connect path.
- Therefore:
  - maintain mirrors for both `ets_timer_*` and `os_timer_*`
  - treat hidden list-head derivation as a secondary cross-check only
  - prefer outward callback-identity observation over private RAM cells

### ISR / RTOS contract facts

- Generic ISR queue masking was tested and rejected.
- The ESP8266 SDK port contract does not justify using generic interrupt-mask helpers as a permanent fix.
- When revisiting ISR/context-switch behavior, diff directly against:
  - `components/freertos/port/esp8266/port.c`
  - `components/freertos/port/esp8266/xtensa_vectors.S`
  - `components/freertos/port/esp8266/os_cpu_a.S`
  - `components/freertos/freertos/tasks.c`

## Rejected Or Frozen Paths

- Do not reopen speculative `connect_blocking()` redesigns without new evidence.
- Do not move DHCP wait into `connect_blocking()` on the embassy path.
- Do not keep vendored `smoltcp` trust / neighbor-learn behavior.
- Do not rely on intrusive sampler tasks for final runtime validation.
- Do not use synthetic ARP-trust shims as permanent behavior.
- Do not hard-code private SDK RAM addresses into shipping logic.
- Do not use generic ISR queue masking as a production fix.

## Important Local Code Paths

- Example under test:
  - `examples/tcp_server.rs`
- Station control / connection flow:
  - `src/wifi/controller.rs`
- Embassy-net bridge:
  - `src/wifi/embassy_net.rs`
- Driver packet path:
  - `src/wifi/device.rs`
- Interrupt / compat layers:
  - `src/interrupt.rs`
  - `src/wifi/compat/port.rs`
  - `src/wifi/compat/task.rs`
  - `src/wifi/compat/timer.rs`
- Probe/wrap source of truth:
  - `build.rs`

## Important SDK References

### SDK tree

- `/tmp/ESP8266_RTOS_SDK`

### lwIP / WiFi references

- `/tmp/ESP8266_RTOS_SDK/components/lwip/port/esp8266/include/lwipopts.h`
- `/tmp/ESP8266_RTOS_SDK/components/lwip/Kconfig`
- `/tmp/ESP8266_RTOS_SDK/components/esp8266/include/esp_wifi_types.h`

### FreeRTOS / port references

- `/tmp/ESP8266_RTOS_SDK/components/freertos/port/esp8266/port.c`
- `/tmp/ESP8266_RTOS_SDK/components/freertos/port/esp8266/xtensa_vectors.S`
- `/tmp/ESP8266_RTOS_SDK/components/freertos/port/esp8266/os_cpu_a.S`
- `/tmp/ESP8266_RTOS_SDK/components/freertos/freertos/tasks.c`

### Example references

- `/tmp/ESP8266_RTOS_SDK/examples/common_components/protocol_examples_common/connect.c`
- `/tmp/ESP8266_RTOS_SDK/examples/protocols/sockets/tcp_server/main/tcp_server.c`

## Key Artifacts

### Final validated matrices

- `/tmp/tcp_server_step437`
  - trust-disabled A/B control
  - `10/10`
- `/tmp/tcp_server_step439`
  - upstream `smoltcp` cleanup control
  - `10/10`

### Pivotal first-SYN proof

- `/tmp/tcp_server_step432/run2/uart.log`
- `/tmp/tcp_server_step432/run2/connect.log`
- `/tmp/tcp_server_step432/run2/tcpdump.log`
- Meaning:
  - inbound `SYN` reached Rust
  - forwarding into the stack happened
  - only ARP transmit occurred before timeout

### Rejected trust experiment

- `/tmp/tcp_server_step435`
  - trust-enabled path after removing probe residue
  - `7/10`

### Earlier “spin removal helps but not enough” proof

- `/tmp/tcp_server_step429`
  - post-spin-removal matrix
  - `9/10`

## Next Justified Step

- Keep the final runtime path as-is unless new evidence appears.
- If a new WiFi regression appears:
  - start from the current upstream-dependency `tcp_server` baseline
  - preserve the SDK-style auto-connect flow
  - preserve the default example WiFi init buffers
  - keep the startup spin removed
- If another packet-startup regression appears:
  - reproduce it first with bounded `10s` captures
  - compare against SDK/lwIP behavior one subsystem at a time
  - prefer wire captures and outward probes over hidden-cell or synthetic-packet tricks
