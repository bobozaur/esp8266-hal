# ESP32 Companion Rig Plan

## Table of Contents
- [Goal](#goal)
- [Context and Constraints](#context-and-constraints)
- [Recommended Approach](#recommended-approach)
- [Alternatives Considered](#alternatives-considered)
- [Architecture](#architecture)
- [File Map](#file-map)
- [Implementation Tasks](#implementation-tasks)
- [Verification](#verification)
- [Risks and Edge Cases](#risks-and-edge-cases)
- [Open Questions](#open-questions)

## Goal

Turn the plugged-in ESP32-PICO-D4 into a reliable companion device for the ESP8266 WiFi hardware matrix.

The companion should cover the remaining hardware gaps that need a second radio:

- soft-AP client join, station-list, DHCP lease, and deauth validation;
- AP+STA downstream-client validation;
- ESP-NOW unicast, encrypted, receive-path, and bad-key validation;
- repeatable traffic generation for promiscuous data/control capture where the ESP32 can produce the traffic.

The ESP32 tooling must be installed repo-locally only. If setup requires writing outside this repository or installing system packages, stop and ask before proceeding.

## Context and Constraints

- DUT: ESP8266 on `/dev/ttyUSB0`.
- Companion: ESP32-PICO-D4 on `/dev/ttyACM0`, MAC `4c:c3:82:82:32:80`.
- Current host has `esptool` and `espflash`, but no `idf.py`, PlatformIO, Arduino CLI, or ESP32 Rust target installed.
- Network access is available, but all downloaded tools/build products must live under this repository.
- Existing automatic ESP8266-only matrix passes; companion work should extend it rather than destabilize it.
- Standard ESP-IDF supports ESP-NOW and encrypted peers on ESP32, so it is a good fit for ESP-NOW companion roles.
- Standard ESP-IDF documents WPS as an ESP32 enrollee feature and SmartConfig as an ESP32 credential receiver. That makes the ESP32 useful for many WiFi companion roles, but not a guaranteed WPS registrar or SmartConfig sender.

Primary references checked:

- ESP-IDF ESP-NOW docs: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/network/esp_now.html>
- ESP-IDF WiFi guide, including WPS enrollee note: <https://docs.espressif.com/projects/esp-idf/en/latest/api-guides/wifi.html>
- ESP-IDF SmartConfig docs: <https://docs.espressif.com/projects/esp-idf/en/latest/api-reference/network/esp_smartconfig.html>

## Recommended Approach

Use a small ESP-IDF C companion firmware plus `xtask` orchestration.

Why this is recommended:

- ESP-IDF is the most reliable route for ESP32 WiFi, ESP-NOW, STA, AP, and promiscuous support.
- A C firmware keeps the companion independent of Rust ESP32 target/toolchain availability.
- A UART command protocol lets `xtask` flash the companion once and reuse it across multiple ESP8266 DUT scenarios.
- `xtask` can record both DUT and companion UART logs into the same evidence directory.
- Keeping ESP-IDF under `.tools/esp32/` satisfies the repo-local tooling requirement.

Repo-local tooling policy:

- Use `.tools/esp32/esp-idf` for the ESP-IDF checkout.
- Use `.tools/esp32/idf-tools` for `IDF_TOOLS_PATH`.
- Use `.tools/esp32/home`, `.tools/esp32/cache`, and `.tools/esp32/pip-cache` for `HOME`, `XDG_CACHE_HOME`, and `PIP_CACHE_DIR` during ESP-IDF setup/build.
- Use `target/esp32-companion/` for CMake/build products.
- Add these tool/build directories to `.gitignore`.
- If any setup command requires `sudo`, system packages, or writes outside the repo, stop and ask.

## Alternatives Considered

### 1. ESP-IDF C Companion Firmware

- Good: mature WiFi/ESP-NOW support.
- Good: examples and APIs directly match companion needs.
- Good: no Rust ESP32 target setup needed.
- Cost: ESP-IDF checkout/toolchain is large.
- Decision: recommended.

### 2. Rust ESP32 Companion Firmware

- Good: keeps everything Rust-sided.
- Bad: current host does not have ESP32 Rust targets installed.
- Bad: WiFi companion roles would need additional crate/toolchain setup and likely more bring-up time.
- Decision: defer until the ESP8266 DUT is production-proven.

### 3. Host/Phone-Only Companion

- Good: no ESP32 firmware setup.
- Bad: not reliable or automated enough for AP/ESP-NOW/promiscuous validation.
- Decision: not sufficient.

## Architecture

### Tooling Layer

Add an `xtask` companion subcommand:

```text
cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw companion <command>
```

Initial commands:

- `doctor`: identify DUT and companion ports, chip types, MACs, and local tool status.
- `setup`: clone/install ESP-IDF under `.tools/esp32/` using repo-local env vars only.
- `build`: build the companion firmware into `target/esp32-companion/`.
- `flash`: flash the companion on `/dev/ttyACM0`.
- `command`: send one UART command and wait for an `OK`/`ERR` response.

### Companion Firmware

Add a single ESP-IDF app under `companion/esp32-pico-d4/`.

The firmware exposes a line-oriented UART protocol. Every command returns deterministic markers:

```text
READY companion esp32-pico-d4 mac=...
OK ...
ERR code=... detail=...
EVENT ...
```

Core commands:

- `PING`
- `MAC`
- `RESET_WIFI`
- `STA_JOIN ssid=<...> pass=<...|open> timeout_ms=<...>`
- `STA_DISCONNECT`
- `UDP_SEND host=<ipv4> port=<u16> count=<n> size=<n>`
- `TCP_CONNECT host=<ipv4> port=<u16> payload=<hex>`
- `ESPNOW_RX channel=<1..14> pmk=<hex|none> peer=<mac|any> lmk=<hex|none>`
- `ESPNOW_TX channel=<1..14> peer=<mac|ff:ff:ff:ff:ff:ff> encrypt=<0|1> lmk=<hex|none> payload=<hex>`
- `PROMISC_TRAFFIC ssid=<...> pass=<...|open> host=<ipv4> port=<u16> count=<n>`

### ESP8266 DUT Integration

Extend `xtask wifi-hw` with companion-aware scenarios:

- `ap_station_list_companion`
- `ap_dhcp_companion`
- `ap_sta_downstream_companion`
- `esp_now_unicast_companion`
- `esp_now_encrypted_companion`
- `esp_now_bad_key_companion`
- `promisc_data_companion`
- `promisc_control_companion` if ESP32 can produce repeatable control traffic through standard APIs

Evidence for companion scenarios includes:

- DUT build/flash/UART logs;
- companion build/flash/UART logs;
- host-side TCP/UDP logs where applicable;
- a manifest with both serial ports and both MAC addresses.

### AP+STA Downstream Strategy

The existing AP+STA proof validates upstream STA DHCP while the AP is started. Full downstream IP validation needs the DUT to drive both STA and AP data links concurrently.

Implementation path:

1. First validate companion AP join and station-list/deauth in mixed mode.
2. Then add a Rust-side mixed dual-link owner if the current typestate API cannot concurrently run STA `embassy-net` and AP `embassy-net`.
3. Use the ESP32 companion as the downstream client to prove DHCP and traffic through the DUT AP side.

## File Map

Planned tracked files:

- Modify: `.gitignore`
- Modify: `xtask/src/main.rs`
- Modify: `xtask/src/wifi_hw.rs`
- Create: `xtask/src/esp32_companion.rs`
- Create: `companion/esp32-pico-d4/CMakeLists.txt`
- Create: `companion/esp32-pico-d4/main/CMakeLists.txt`
- Create: `companion/esp32-pico-d4/main/main.c`
- Create: `companion/esp32-pico-d4/sdkconfig.defaults`
- Create/modify: `tests` or `xtask/tests/*` for command/CLI behavior
- Modify: `examples/wifi_ap.rs` as needed for companion markers
- Modify: `examples/wifi_ap_sta.rs` as needed for downstream validation
- Modify: `examples/wifi_esp_now_tx.rs` and `examples/wifi_esp_now_rx.rs` as needed for peer coordination
- Modify: `research.md`

Planned untracked/ignored files:

- `.tools/esp32/`
- `target/esp32-companion/`

## Implementation Tasks

### Task 1: Add Repo-Local Tooling Sandbox

**Why:** ESP-IDF setup must not pollute the host or user home directory.

**Files:**
- Modify: `.gitignore`
- Create: `xtask/src/esp32_companion.rs`
- Modify: `xtask/src/main.rs`
- Modify: `xtask/src/wifi_hw.rs` if shared serial helpers are needed

**Steps:**
- [ ] Add ignored paths for `.tools/esp32/` and `target/esp32-companion/`.
- [ ] Add helper functions that construct repo-local ESP-IDF environment variables.
- [ ] Add `wifi-hw companion doctor` to identify `/dev/ttyUSB0` and `/dev/ttyACM0`.
- [ ] Add `wifi-hw companion setup` that clones/installs ESP-IDF only under `.tools/esp32/`.
- [ ] Make setup fail before running any command that needs `sudo` or system package installation.

**Verification:**
- Run: `cargo test -p xtask --target x86_64-unknown-linux-gnu`
- Expect: CLI tests pass.
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw companion doctor`
- Expect: ESP8266 DUT and ESP32-PICO-D4 companion are both identified.
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw companion setup`
- Expect: ESP-IDF/toolchain files appear only under `.tools/esp32/`.

### Task 2: Build And Flash Companion Firmware

**Why:** The ESP32 must expose deterministic UART-controlled WiFi roles.

**Files:**
- Create: `companion/esp32-pico-d4/*`
- Modify: `xtask/src/esp32_companion.rs`

**Steps:**
- [ ] Scaffold a minimal ESP-IDF app with UART command parsing.
- [ ] Implement `READY`, `PING`, `MAC`, and `RESET_WIFI`.
- [ ] Add `build` and `flash` commands in `xtask`.
- [ ] Capture companion build and flash logs under the scenario evidence directory.

**Verification:**
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw companion build`
- Expect: firmware builds under `target/esp32-companion/`.
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw companion flash --serial-port /dev/ttyACM0`
- Expect: companion flashes and emits `READY`.
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw companion command PING`
- Expect: `OK PONG`.

### Task 3: Validate AP Client Roles

**Why:** AP station-list, deauth, AP DHCP, and AP+STA downstream tests need a reliable station client.

**Files:**
- Modify: `companion/esp32-pico-d4/main/main.c`
- Modify: `xtask/src/wifi_hw.rs`
- Modify: `examples/wifi_ap.rs`
- Modify: `examples/wifi_ap_sta.rs`

**Steps:**
- [ ] Implement `STA_JOIN` and `STA_DISCONNECT`.
- [ ] Add companion orchestration for `ap_station_list_companion`.
- [ ] Add companion orchestration for `ap_dhcp_companion`.
- [ ] Add mixed-mode downstream-client validation.
- [ ] If mixed-mode AP IP validation needs simultaneous STA/AP data links, add the smallest correct typed dual-link owner in `wifi_core`.

**Verification:**
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw run ap_station_list_companion`
- Expect: ESP32 joins the DUT AP, DUT enumerates a non-zero station list, and DUT deauths the ESP32.
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw run ap_dhcp_companion`
- Expect: ESP32 receives a `192.168.4.x` lease from the Rust DHCP service.
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw run ap_sta_downstream_companion`
- Expect: DUT upstream STA and downstream AP client IP behavior coexist.

### Task 4: Validate ESP-NOW Two-Device Roles

**Why:** ESP-NOW unicast, encrypted peers, RX, and bad-key cases cannot be proven on one board.

**Files:**
- Modify: `companion/esp32-pico-d4/main/main.c`
- Modify: `xtask/src/wifi_hw.rs`
- Modify: `examples/wifi_esp_now_tx.rs`
- Modify: `examples/wifi_esp_now_rx.rs`

**Steps:**
- [ ] Implement ESP-NOW init/deinit in the companion.
- [ ] Implement `ESPNOW_RX`.
- [ ] Implement `ESPNOW_TX`.
- [ ] Add unicast scenario where DUT TX reaches ESP32 RX.
- [ ] Add encrypted scenario with fixed test PMK/LMK.
- [ ] Add bad-key scenario that proves predictable failure or no receive.

**Verification:**
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw run esp_now_unicast_companion`
- Expect: ESP32 logs payload received from ESP8266.
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw run esp_now_encrypted_companion`
- Expect: encrypted payload is received with matching LMK.
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw run esp_now_bad_key_companion`
- Expect: mismatched-key case fails predictably and is recorded.

### Task 5: Validate Promiscuous Traffic Generation

**Why:** Ambient RF is not enough to prove data/control filter behavior.

**Files:**
- Modify: `companion/esp32-pico-d4/main/main.c`
- Modify: `xtask/src/wifi_hw.rs`
- Modify: `examples/wifi_promisc.rs`

**Steps:**
- [ ] Implement `PROMISC_TRAFFIC` to join a configured AP and generate UDP/TCP data traffic.
- [ ] Add a DUT promiscuous data-filter scenario that captures ESP32-generated data frames.
- [ ] Investigate whether standard ESP-IDF APIs can reliably trigger selected control subtypes.
- [ ] Add control-subtype scenarios only for traffic that can be generated reproducibly.

**Verification:**
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw run promisc_data_companion`
- Expect: DUT captures data frames with valid metadata.
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw run promisc_control_companion`
- Expect: if implemented, DUT captures the selected control subtype; otherwise the plan records the precise unsupported subtype gap.

### Task 6: Record Provisioning Feasibility

**Why:** The ESP32 companion may not replace a WPS registrar or SmartConfig sender through standard ESP-IDF APIs.

**Files:**
- Modify: `research.md`
- Modify: `docs/superpowers/plans/2026-04-23-wifi-full-functionality.md`
- Modify: `xtask/src/wifi_hw.rs` if any provisioning helper becomes feasible

**Steps:**
- [ ] Check ESP-IDF examples/APIs inside the repo-local checkout for WPS registrar support.
- [ ] Check ESP-IDF examples/APIs inside the repo-local checkout for SmartConfig sender support.
- [ ] If supported, add companion provisioning scenarios.
- [ ] If unsupported, document that WPS and SmartConfig still need router/phone/host-side sender orchestration.

**Verification:**
- Run: `rg -n "wps|smartconfig|esptouch" .tools/esp32/esp-idf/examples .tools/esp32/esp-idf/components`
- Expect: evidence recorded for supported vs unsupported companion provisioning roles.

### Task 7: Final Two-Device Matrix And Evidence

**Why:** The companion rig is only useful if it produces reproducible evidence.

**Files:**
- Modify: `xtask/src/wifi_hw.rs`
- Modify: `research.md`

**Steps:**
- [ ] Add a two-device matrix mode that includes companion scenarios.
- [ ] Preserve per-board logs and MAC/serial identity in each evidence directory.
- [ ] Update `research.md` with pass/fail evidence and remaining manual limitations.
- [ ] Keep the single-board matrix passing.

**Verification:**
- Run: `cargo test -p xtask --target x86_64-unknown-linux-gnu`
- Expect: all host-side harness tests pass.
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw matrix`
- Expect: original single-board matrix still passes.
- Run: `cargo run -p xtask --target x86_64-unknown-linux-gnu -- wifi-hw matrix --companion-port /dev/ttyACM0`
- Expect: companion-capable scenarios pass and record both UART logs.

## Verification

Final verification for this project:

- Host harness tests pass.
- ESP-IDF companion builds from repo-local tooling only.
- ESP32 companion flashes and responds to `PING`.
- Original ESP8266 one-board matrix still passes.
- Companion AP scenarios pass.
- Companion ESP-NOW scenarios pass.
- Companion promiscuous data scenario passes.
- Any unsupported WPS, SmartConfig, or exact control-subtype cases are documented with API/tooling evidence.

## Risks and Edge Cases

- **ESP-IDF side effects:** installer scripts default to user-home paths unless environment variables are set. Every setup/build command must run with repo-local `HOME`, `XDG_CACHE_HOME`, `IDF_TOOLS_PATH`, and `PIP_CACHE_DIR`.
- **System package dependency:** ESP-IDF may require host tools not currently installed. Stop and ask before installing anything outside the repo.
- **Serial reset behavior:** DUT and companion have different serial adapters and reset behavior; flashing and monitoring must not cross ports.
- **Channel coupling:** ESP-NOW and AP+STA scenarios need explicit channel coordination between ESP8266 and ESP32.
- **AP+STA dual data links:** full downstream IP proof may require additional typestate/API work on the ESP8266 driver, not only companion firmware.
- **Control frames:** standard APIs may not allow deterministic generation of every 802.11 control subtype.
- **Provisioning:** ESP32 standard APIs may not provide WPS registrar or SmartConfig sender roles.

## Open Questions

- No blocker remains for repo-local ESP-IDF setup and companion firmware bring-up.
- WPS registrar and SmartConfig sender support must be determined from the repo-local ESP-IDF checkout before promising those scenarios.
