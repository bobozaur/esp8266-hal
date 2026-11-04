# ESP8266 HAL refactor plan

Status: implementation, adversarial review and final validation are complete for
the qualified configurations below. The OpenWrt follow-up is complete and the
router is restored. Remaining qualification limits are explicit; publication is separate.
Updated: 2026-11-04.

The decisions below were agreed during discussion. The user subsequently
authorized autonomous planning, adversarial review to saturation, implementation
and hardware validation. Implementation choices and progress are now tracked here
without requiring further approval. Commits and publication are outside this task.

## Agreed scope

- Add a Wi-Fi driver to the published `esp8266-hal`.
- ESP8266 is the only deployment target.
- Review the entire project iteratively, including existing implementation and
  design choices, with simplicity, readability, correctness, performance and
  less unnecessary code as priorities.
- Work through small, digestible topics with brief, clear explanations.
- Maintain this plan as decisions are agreed.
- Take heavy inspiration from the official esp-rs ESP32 crates and Wi-Fi
  driver, including `esp-hal`, `esp-radio`/`esp-wifi`, `esp-alloc` and their
  runtime integration. Use their designs as references, scrutinizing and
  adapting them to ESP8266 requirements.
- Minimize vendored crates/code, SDK patches, shims and custom replacements
  where practical. Prefer existing upstream dependencies and unmodified SDK
  behavior; retain only adaptations justified by ESP8266 requirements.
- Challenge the reasons for existing customizations and look for cleaner
  alternatives. Existing functionality is evidence to preserve, not proof
  that its current customization is still necessary.

## Agreed release policy

A new minor release may break the API when necessary, or when the change
improves simplicity, performance or quality. Compatibility does not require
preserving a worse design.

## Agreed library and runtime direction

- The application explicitly initializes Wi-Fi and controls starting and
  stopping it. Merely using the HAL should not initialize Wi-Fi automatically.
- Prefer Wi-Fi APIs available without a Cargo feature where practical. A
  feature is acceptable if scrutiny shows that it is the better design.
  Feature selection and explicit runtime activation are separate choices.
- Treat this as a configurable library: leave application policy and usage
  choices to its users while keeping the deployment target ESP8266.
- These are design goals. Exact startup/shutdown behavior and resource costs
  still need investigation; full runtime teardown has not been promised.

## Agreed application runtime boundary

- Keep the scheduling machinery required by the Wi-Fi SDK internal to the
  driver. Its implementation and resource costs remain subject to review.
- Wi-Fi must not require the application to use Embassy. Provide Embassy
  integration optionally, leaving application execution choices to the user.

## Agreed blocking and async API direction

- Support blocking and async forms of operations that wait, such as connecting
  and scanning, through one driver with shared state and operation logic.
- Immediate configuration operations remain ordinary synchronous methods.
- Avoid separate blocking and async driver implementations. Exact method
  signatures remain to be decided.

## Agreed allocation direction

- The application selects and initializes its allocator. Wi-Fi should be able
  to share it through a small integration interface with clear requirements.
- Investigate shared application allocation as the preferred direction.
  Require a separate Wi-Fi memory region only for a concrete, demonstrated
  benefit or hardware/SDK requirement.
- Exact allocator interfaces, SDK constraints and the recommended setup remain
  to be determined; this is an agreed direction, not a completed design.

## Agreed network boundary

- Keep the Wi-Fi driver independent of the application's network stack. Its
  responsibilities cover scanning, connecting, station/AP control and sending
  and receiving frames.
- Leave IP addressing, DHCP, TCP, UDP and sockets to the selected network
  stack. Provide standard integration interfaces and straightforward examples.
- Review existing custom networking services against this boundary. SDK-internal
  networking machinery needs separate investigation before removal is proposed.

## Agreed connection policy

- Leave retry and reconnection policy to the application: retry limits, delays,
  alternative networks and when to give up are application choices.
- The driver provides connection/disconnection operations and reports connection
  failures and disconnection reasons, without an automatic reconnection loop.
- Keep normal Wi-Fi protocol retries inside the SDK. Provide simple application
  examples demonstrating retry and reconnection loops.

## Agreed connection timeout and cancellation direction

- Follow the official `esp-radio` approach for connection attempts: the async
  connection operation has no built-in timeout; callers apply their own.
- Dropping the connection future stops waiting without automatically
  disconnecting. The connection attempt may still complete afterwards.
- Use explicit disconnection to abort an attempt or close an established
  connection, with a way to wait for completion.
- Exact blocking interfaces remain open. ESP8266 SDK cancellation and completion
  behavior still need verification before implementation guarantees are settled.

## Agreed public driver abstraction

- Expose an ESP8266-specific Wi-Fi driver without a public hardware-backend
  parameter or backend replacement framework.
- Keep SDK implementation details and any abstractions needed for meaningful
  host tests internal. Preserve application configuration and network-stack
  integration choices.

## Agreed controller state model

- Replace the public lifecycle/mode typestate chain with one controller type,
  following the official `esp-radio` model. Track lifecycle and mode internally.
- Configuration, start and stop operate on that controller. Invalid operations
  return errors rather than being excluded by lifecycle/mode type parameters.
- Accept the loss of those compile-time operation checks in exchange for simpler
  storage, reconfiguration and public types. Preserve resource-access and memory
  safety through ownership, lifetimes and appropriate internal checks.

## Agreed queue sizing and storage

- Express buffer and queue capacities as ordinary initialization configuration
  with sensible defaults, rather than carrying them through public driver types.
- Preallocate the Rust driver's queue storage through the application-selected
  allocator during initialization and keep its capacities fixed while initialized.
- Report insufficient memory as an initialization error. Accept initialization-
  time allocation failure as the tradeoff compared with caller-supplied static
  arrays.
- SDK packet allocations remain subject to separate review; this direction does
  not establish allocation-free packet handling throughout the SDK.

## Agreed controller and network interfaces

- Keep connection control available while a network stack owns its packet
  interface. Separate controller operations from station/AP frame interfaces.
- All handles refer to one underlying ESP8266 driver. The application can
  reconnect while the stack retains its interface.

## Accepted implementation architecture

The autonomous investigation and adversarial review resolved the remaining API,
feature, allocation and lifecycle details below. The migration guide describes
the implemented public interfaces.

### Runtime and allocation

- Provide explicit once-per-boot runtime initialization before constructing Wi-Fi
  or starting the optional executor. It consumes the timer resource it reserves.
  Ordinary HAL entry does not install an allocator or start Wi-Fi.
- Use an application-owned, permanently live allocator implementing allocation,
  deallocation and truthful available-DRAM statistics. The SDK requests these
  statistics directly, so a Rust global allocator alone is insufficient.
- The allocator must supply byte-addressable DRAM, honor layouts, and support
  the SDK's preemption/interrupt contexts. Provide a concise esp-alloc example;
  do not install a library-owned global allocator.
- Enabling Wi-Fi also requires application registration of a Rust global
  allocator because the upstream fallible containers depend on `alloc`. The
  runtime still allocates through its explicit provider; examples share one
  provider for both roles. Merely registering it does not initialize a heap.
- The SDK allocation ABI keeps only necessary headers/adaptation. Rust-owned
  state and bounded queues use the same allocator through fallible allocation.
- Retain the existing scheduler/port initially, remove implicit entry coupling,
  make startup allocation failures visible before enabling interrupts, and keep
  the application's original stack reserved. Do not reclaim a live boot stack.
- Runtime infrastructure is persistent for the boot. Wi-Fi stop/drop does not
  imply removal of scheduler, timers, event infrastructure or PHY state.
- Concrete entry: `Runtime::init(Timer2, &'static dyn WifiAllocator)` returns a
  permanent runtime token. Reject interrupt/critical-section context and repeated
  initialization. Failed initialization consumes the timer; retry requires reset.
  Reserve FRC2, CCOMPARE and software scheduling interrupts; retain Timer1 for
  applications. The optional executor requires the runtime token explicitly.
- Use published `allocator-api2` 0.2.18 with a single private allocator adapter
  for fallible Box/Vec allocation. Avoid custom owning pointers and any second
  allocator registration. Check the pinned compiler rather than assume compatibility.
- Runtime owns compare scheduling and elapsed-time accounting. Short Embassy
  alarms must not advance SDK ticks prematurely; carry sub-tick elapsed time and
  advance SDK ticks only for full tick periods while dispatching due alarms.
- Reset per-initialization SDK state (including PHY delay mode) explicitly.
  Stop/reinitialize behavior is a hardware validation requirement.

### Driver ownership and operations

- Use one owning Wi-Fi object with stable heap state. Borrow controller and
  station/AP interfaces from it; no public backend, capacity or mode parameters.
  A static owner can support long-lived executor tasks without reference counting.
- Native callbacks never borrow stack-owned application resources. Forgetting
  a public handle must not produce dangling SDK pointers. Failed teardown keeps
  callback-visible allocations and singleton ownership alive rather than freeing
  memory that the SDK might still reference.
- Serialize SDK control operations and keep one authoritative lifecycle/mode
  state. Do not duplicate link state and operation completion unnecessarily.
- `Wifi::new(&'static Runtime, InitConfig)` obtains allocation from the runtime;
  `Wifi::split(&mut self)` borrows a controller and named station/AP interfaces.
  Initialization unwinds allocations on failure, including the state header, when
  SDK quiescence is established; the quarantine rule overrides rollback otherwise.
- Use a common operation error carrying SDK codes and disconnection reasons,
  with separate initialization/configuration errors only where contracts differ.
- Keep controller completion and network-interface wakers separate. Dropping or
  forgetting futures/sessions does not clear actual active operations. Reject a
  new operation while old work remains active; invalidate configuration after
  partial SDK failure. Wake only outside registration/resource locks.
- Completion records distinguish connect success, terminal SDK failure, explicit
  disconnect completion, and merely accepting an SDK command. No synthetic event
  may pretend an asynchronous operation has completed.
- Blocking and async operations share the operation logic. Async connect has no
  internal timeout; explicit disconnect aborts. Blocking waits use a caller-chosen
  deadline and leave cancellation explicit.
- Remove the automatic reconnect flag/loop. Preserve reported SDK failure reasons.
- Scans use SDK completion events where viable; protect cleanup of results after
  cancellation. Preserve/document a demonstrated SDK limitation rather than
  pretending a blocking call is async or dropping unrelated event notifications.
- Preserve ESP-NOW, promiscuous, WPS, SmartConfig and enterprise capabilities with
  smaller session/configuration APIs. Bounded event queues report overflow instead
  of silently losing terminal results.
- Stop/reconfiguration closes packet admission, accounts for checked-out SDK RX
  leases, drains queued packets, and only then stops the SDK. Reject conflicting
  operations while leases are live. Never reuse an AP slot with a live lease.
- Include TX tokens and retained adapter frames in the stop/restart contract.
  Capture a small lifecycle generation and reject stale submissions after restart;
  drain pending TX state on stop. Do not rely on future/token destructors for
  safety when a user can forget them.
- Main will probe SDK nonblocking-scan event delivery on the board before the
  async scan implementation is chosen. This is a prerequisite for that operation,
  not an excuse to wrap the old blocking call in an async function.

### Network and feature boundaries

- Share station/AP frame primitives across network adapters. Retain bounded
  buffering/backpressure where the stack's infallible transmit-token API cannot
  represent SDK rejection. Remove wrapper/generic duplication before adding a
  new pumping mechanism; avoid packet copies without evidence they are needed.
- Move custom DHCP/IP service policy out of the driver into example/application
  support or replace it with an appropriate upstream component. Keep the native
  AP/lwIP ABI bridge only where blob calls demonstrably require it.
- Separate optional Embassy execution/time and network adapters from Wi-Fi core.
  Remove the public host-backend framework while preserving private host tests.
- Measure linked unused-Wi-Fi costs before removing the `wifi` Cargo feature.
  Retain an opt-in native-runtime feature if linker roots, static memory or toolchain
  requirements make unconditional availability materially worse. Runtime start
  stays explicit either way. Protocol flags need concrete dependency/cost reasons.
- Retain `wifi`: the compiler's global-allocator requirement already makes
  unconditional availability inappropriate for allocator-free HAL applications.
  Measure linked sizes as additional evidence, without replacing upstream
  containers with custom allocation code solely to avoid this boundary.

### SDK provenance and publication

- Preserve the pinned native ABI and existing binary provenance. Audit each custom
  patch/replacement against the original SDK; remove it only with link and behavior
  evidence. Closed ESP8266 radio archives are unavoidable and not ESP32-compatible.
- Keep required native artifacts in the sys crate; no new vendored Rust crates,
  patched registry dependencies or ad hoc shims. Document retained adaptations
  and their reasons instead of claiming an unmodified SDK.
- Investigate/remedy duplicate definitions rather than globally suppressing them.
  Link an actual external consumer outside the repository's Cargo-config ancestry.
- Verify HAL, PAC, proc-macro and sys package contents and document dependency
  publication order. Do not claim a registry-ready HAL until unpublished dependency
  constraints are explicitly accounted for; do not publish during this task.

### Non-Wi-Fi HAL corrections

- Replace movable closure interrupt registration with the existing static-handler
  model, fix interrupt mask operations, and compose application/SDK dispatch.
  Explicitly reserve SDK interrupt/timer resources.
- Preserve application dispatch even when Wi-Fi is compiled but runtime is not
  initialized. One dispatcher clears pending bits; SDK registrations take
  precedence, otherwise call the application handler. Raw interrupt enable is
  unsafe with explicit ownership/critical-section preconditions; avoid a separate
  reservation error framework for this low-level escape hatch.
- Fix UART release ownership on WouldBlock, timer overflow/zero handling, GPIO
  output-latch reporting and GPIO16 mode, and SPI clock-count calculations.
- Use `core::time::Duration`, the fixed ESP8266 timer clock and chunked long
  delays; remove unnecessary reciprocal duration/frequency conversions.
- Fix flash sector stride, per-command write enable, and destination page splitting.
  Copy source chunks to DRAM before disabling cache. Ensure command code/data and
  callees are cache-safe; coordinate IRQ/NMI exclusion with the runtime owner.
- Flash transaction code saves processor/NMI state locally, masks both throughout
  cache-off commands and restores cache before NMI/processor state. Use bounded
  DRAM chunks and no allocation inside that transaction.
- Preserve generated PAC code except necessary integration changes. Avoid bulk
  rewrites or dependency upgrades without a concrete correctness/simplicity benefit.

## Campaign checklist and ownership

| Campaign | Owner | State / completion evidence |
| --- | --- | --- |
| Inventory, baseline and board backup | Main | Baseline host tests and target scan passed; full 4 MiB flash backup retained; target cache moved after disk exhaustion |
| Runtime/SDK design investigation | Runtime worker | Source report complete; allocator, stack, failure and reinit constraints identified |
| API/packet design investigation | API worker | Source findings established; callback and RX lease lifetime constraints identified |
| Non-Wi-Fi and packaging investigation | HAL worker | Source report complete; interrupt/flash/peripheral and packaging defects confirmed |
| Integrated plan adversarial review | Main + reviewers | Complete: independent revised-plan pass found no new material findings; lane reviews converged |
| Runtime and SDK refactor | Runtime worker | Corrected and independently reviewed; both boards pass 100 cycles after idle-stack and NMI allocator fixes |
| Controller, resources and protocol refactor | API worker | Complete; revised independent source review found no new material findings |
| Packet adapters and application migrations | Packet worker + migration owner | Complete; mixed task storage, MTU, shared bounded connection policy and DHCP delivery corrections pass source review and affected hardware checks |
| Non-Wi-Fi HAL fixes | HAL worker | Source corrections accepted; linked cache-off flash path and literals verified in IRAM |
| Features, build, docs, packaging integration | Main | Complete; isolated features, package checks and external-consumer link pass; migration and final section measurements documented |
| Development checks and corrections | Main + owners | Complete: corrected mixed upstream/downstream DHCP, AP-only DHCP, 20 TCP echoes, Pi TCP, bounded lifecycle recovery and bare entry pass; earlier protocol/HAL checks retained |
| Final adversarial review and corrections | Independent reviewers + owners | Complete; revised core/adapters, HAL, runtime, NMI exclusion and final mixed-mode/application passes report no new material findings |
| Final integrated host/target/hardware validation | Main | Complete: 27 host checks, 22 target checks, both 100-cycle stress runs, fresh paired/network/peripheral checks and cleanup pass. Both reserved flash tails match their backups; the boards run a verified AP/DHCP-client pair |
| OpenWrt protocol qualification and restoration | Main + protocol owners | Complete: WPS PBC/PIN, SmartConfig DHCP/ACK and post-stop DHCP, PEAP, wrong-password rejection and compact P-256 EAP-TLS pass. Native cleanup/data placement, allocator and NMI polling corrections are reviewed to saturation. TLS record/heap limits are documented. All router restoration checks pass |

## Final results and remaining qualification

- All 27 host checks pass: feature-isolated and combined tests, formatting,
  warnings/lints, documentation, native archive integrity, supporting-package
  verification, HAL package contents and proc-macro diagnostics.
- All 22 target checks pass: bare/no-RT and RAM-resident variants, every retained
  protocol and adapter example, combined features, target documentation and a
  downstream application with independent Cargo configuration.
- Each board completes 100 uninstrumented create/start/scan/stop/drop cycles and
  rejects changed native buffer dimensions after first attachment. The lifecycle
  fixture also passes cancellation, fresh scans, 100 short timer waits, async
  connect/disconnect, blocking reconnect, preserved configuration after stop/start,
  and driver recreation. Its application policy permits three attempts; all RF
  failures remain logged.
- Real network checks pass AP-only DHCP, concurrent station/upstream and
  AP/downstream DHCP, plain and encrypted ESP-NOW, promiscuous reception, a LAN-host
  TCP exchange, and 20 byte-exact echo connections spanning 4..2048-byte payloads.
  The final echo campaign completes in 4.96 seconds; this is a smoke-test duration,
  not a general throughput benchmark.
- Bare entry and HAL main entry markers pass. Timer2 measures 1.105 seconds for
  a 1.1-second wait; Timer1 measures 4.968..5.021 seconds for five-second waits
  before runtime, with AP active and after flash. UART interrupts at 230400 baud
  remain responsive; the AP/UART scenario retains 592 bytes of IRQ-stack headroom.
- Flash checks pass two-sector erase, distinct writes, unaligned/cross-page
  writes and readback. Scratch and adjacent guards are restored. Both boards'
  reserved final 16 KiB match their respective backups. They are left running a
  verified AP/DHCP-client pair. The temporary LAN listener has exited; no host
  files, services or configuration were changed. The router configuration,
  staged UCI state, boot identity and global services match the baseline; its
  temporary AP and fixture files are removed, and its uplink remains connected.
- The all-feature TCP image reports `OutOfMemory` cleanly. Its static footprint
  leaves insufficient heap for that example's radio configuration; successful
  linking is not an operational all-feature memory claim. Final section sizes
  are in [the native runtime notes](docs/native-runtime.md#cost-and-current-evidence).
- WPS PBC/PIN and SmartConfig provisioning, DHCP and acknowledgement now pass
  against an external fixture. SmartConfig also passes fresh DHCP after stop.
  PEAP and compact P-256 client-certificate EAP-TLS pass, including the
  four-way handshake; the wrong-password case is rejected. Larger RSA certificate
  profiles and arbitrary certificate chains remain outside the qualified TLS
  memory budgets. Arbitrary user callback/interrupt stack depth, deterministic RF
  injection and independently mapped flash-cache coherence remain unqualified.
  The tested router exposes no monitor interface for deterministic injection.
  Association on the first attempt is not guaranteed. These limits do not replace
  the host tests, source reviews or hardware evidence above.
- Before publishing HAL 0.6, publish the dependency versions listed in the
  [migration guide](docs/migration-0.6.md). No commits or publication were performed.
  Raw failure and final-run evidence remains in the ignored private workspace;
  no diagnostic production code or private probe examples are packaged.

## Validation and completion criteria

- Preserve meaningful existing contract tests while moving private backend tests
  behind the module boundary. Add focused regressions for reachable safety,
  ownership, queue saturation, operation failure/cancellation and arithmetic bugs.
- Run formatting, warnings/lints, rustdoc, host tests (minimal and retained protocol
  combinations), ABI and tooling tests, and package-content checks. Fix the fresh
  checkout/lockfile behavior in CI rather than relying on local artifacts.
- Link bare HAL, synchronous Wi-Fi, optional async/network, station/AP/mixed, and
  every retained protocol example on the pinned Xtensa toolchain. Check no-RT use
  where supported and a consumer with independent Cargo configuration.
- Compare actual IRAM/DRAM/flash sections for representative baseline/refactor
  images, including Wi-Fi available but unused. Do not treat GNU total `size` as
  a complete RAM or runtime-heap measurement.
- Main agent alone owns the attached board/serial port. Back up flash before
  writing and preserve calibration/configuration sectors. Do not touch unrelated
  connected devices. Keep credentials and device-specific evidence in ignored
  `.codex-space/`.
- On the available board, validate explicit setup, scan, AP start/stop, repeated
  lifecycle where supported, failure/timeout/reconnect, and timer/UART coexistence.
  Use an available authorized network for DHCP/TCP and packet saturation; use
  scratch-flash tests only after confirming and preserving the selected region.
- ESP-NOW peers, AP client traffic, WPS/SmartConfig, enterprise authentication and
  deterministic RF injection require appropriate external fixtures. Run available
  scenarios; mark unavailable evidence explicitly rather than substitute old runs.
- Adversarial reviews cover correctness, soundness, resource bounds, API usability,
  feature/package behavior and simplification. Fix material findings in batches,
  then review the revised integrated result. Saturation requires an independent
  pass with no new material findings, not just completing a preset pass count.
- After development checks and review corrections, run a distinct final integrated
  validation campaign. Report implemented scope, remaining limitations, review
  outcome and actual hardware evidence. No commits, releases or publication.

## Review ledger

- Reviews resolved callback lifetime and shutdown ownership, reentrant waker
  callbacks, cancellation state, stale provisioning events, enterprise restart
  state, WPS format-string validation and SmartConfig BSSID preservation.
  Uncertain SDK quiescence keeps callback resources quarantined. The concrete
  controller and borrowed packet interfaces avoid public backend/typestate
  propagation; fixed queues and application-owned reconnect policy avoid hidden
  resource growth and policy. Simplification remains an acceptance question:
  remove redundant layers while preserving useful tested contracts.
- Hardware proved real nonblocking SDK scan completion and cancellation events.
  Blocking and async waits share operation state and completion barriers; dropping
  a future does not pretend to cancel native work. Distinct wakers, elapsed SDK
  tick accounting and TX generation checks preserve independent progress and
  reject stale submissions. The lifecycle fixture logs every RF result and uses
  three bounded application attempts with explicit disconnect cleanup. Recovery
  after reason 201 qualifies bounded recovery, not first-attempt association.
- Certificate/key APIs retain permanent static buffers directly; copying them
  into heap storage adds cost without stronger lifetime safety. Native identity,
  username and password setters copy their inputs. Dynamic certificate ownership
  remains outside this refactor. SSID and credential text preserves whitespace;
  numeric and boolean example options trim ASCII only. Avoiding Unicode trimming
  recovers 432 bytes of static DRAM in the EAP image.
- The linker map respects 96 KiB physical DRAM ending at `0x40000000`, reserves
  the upper 8 KiB for the inherited application stack, exposes only 512 bytes of
  user RTC memory at `0x60001200`, and bounds IROM to its mapped 1 MiB window.
  Unused Wi-Fi still costs SDK storage and global allocator registration, so its
  compile-time feature boundary remains. Section sizes, runtime heap and stacks
  are accounted for separately; no blanket size or performance improvement is
  claimed. Current measurements belong in the native runtime notes.
- Link integration selects the SDK syscall allocator and three explicit aliases
  instead of blanket duplicate-symbol suppression. Xtensa ELF inspection shows
  native malloc/calloc/realloc/free reach the application provider; no separate
  newlib heap is selected. Obsolete delay, reentrant-allocation and startup
  wrappers are removed. Closed Espressif radio archives are unchanged; the three
  derived SDK/WPA/mbedTLS archives were rebuilt from canonical inputs and the
  pinned toolchain, with deterministic archive/provenance checks.
- Native startup ignores failures in mandatory radio, PP and configuration
  allocations. Nine ordinary allocations are preflighted through the application
  provider and existing C headers, then handed off once at audited SDK sites;
  unused allocations are released. No separate arena is needed. Repeat init
  reserves the temporary 1024-byte NVS buffer; the 884-byte PHY workspace is
  first-attach-only. Native hardware/PP buffers survive deinit, so their
  dimensions lock on first native entry, including later partial-init failure.
  Changed native counts are rejected; Rust queue capacities may change between
  owners. The all-feature low-memory image now reports `OutOfMemory` cleanly.
- Hardware and source audit found the wrong SPI EXT2 address in flash/cache
  adapters. The corrected offset is `0xf8`, replacing `0xf0`; an unsupported
  cache-bit polling loop is removed. Cache-off paths follow the pinned SDK
  sequence and remain in IRAM. Flash checks pass sector erase, distinct writes,
  unaligned/cross-page writes and exact interface readback, with scratch and guard
  sectors restored. These checks do not qualify independently mapped flash-cache
  coherence or attribute every earlier stall to the register correction.
- Two independent NMI entry defects required correction: the return-edge guard
  preserves saved EPC3/EPS3 when an NMI interrupts the final `RFI`, and the
  HAL-owned vector uses EXCSAVE3 so it cannot clobber ordinary exception entry's
  EXCSAVE1. This preserves the SDK sequence without patching dependency sources
  or closed blobs. Native NMI paths also reach allocation/free/statistics, so
  Wi-Fi critical sections mask NMI as well as ordinary interrupts and restore
  both with a nested `u64` token. ELF inspection confirms call-free IRAM entry
  and exit. No speculative runtime-clock reset workaround is retained.
- The intermittent recreation stall was isolated from association failures by
  scan-only runs. A watchdog/RTC capture identified the idle task's stack-canary
  halt: its 512-byte request plus 176-byte context made a 688-byte allocation,
  and an interrupt frame starting 12 bytes above its base overwrote a canary.
  Restoring the SDK's 1024-byte idle request makes 1200 bytes and fixes that
  observed overflow. Both boards pass 100 uninstrumented create/start/scan/stop/
  drop cycles and reject changed native buffer dimensions.
- The separate IRQ stack grows from 512 to 1024 bytes because the AP/UART case
  previously left only 80 bytes unused. One constant controls assembly and
  allocation; native archives contain no conflicting fixed-offset references.
  HAL peripheral checks pass Timer1/Timer2, UART interrupts at 230400 baud and
  flash coexistence; the measured AP/UART case leaves 592 bytes of IRQ headroom.
  That is representative coverage, not a bound on arbitrary user handlers.
- TCP socket buffers are now 512 bytes each rather than 16, costing 992 extra
  static bytes and avoiding the observed small-buffer latency. The current smoke
  test passes 20 byte-exact connections with 4..2048-byte payloads in 4.96 seconds;
  it is not a throughput benchmark. Mixed-mode adapter storage moved to
  `StaticCell` to bound Embassy task size. Small network examples use 672-byte
  Ethernet frames: 640 left only 558 bytes after the DHCP client's header
  deductions, below its required 576-byte advertised minimum. Library defaults
  and native buffer counts are unaffected.
- Mixed DHCP honors broadcast/unicast destinations, preserves a serialized reply
  for up to one second of transient TX admission pressure and yields between
  retries. Assertions require successful TX and an actual downstream lease;
  holding the client until server readiness excludes stale exchanges. The
  final integrated campaign passes lifecycle/reconnect/recreation, AP-only and
  mixed DHCP, plain/encrypted ESP-NOW, 32-frame promiscuous reception and LAN-host
  TCP after the native changes.
- Native TLS cleanup now tolerates a null connection after initialization
  failure. Byte-loaded cipher-suite descriptors and digest metadata stay in
  DRAM; unsafe Camellia byte-S-box flash placement is removed. The canonical
  configuration disables optional mbedTLS DES/Camellia/Blowfish while retaining
  AES CBC/CCM/GCM, RSA/ECDHE key exchanges and WPA's separate MSCHAPv2 DES.
  Removing the RSA reservation pool lets TLS/EAP use the ordinary application
  allocator. Compact P-256 EAP-TLS and PEAP pass, including authentication and the
  four-way handshake; the incorrect-password case is rejected. Larger RSA
  certificate profiles remain limited by the 768/512-byte TLS input/output
  content budgets, 256-byte MPI maximum, 256-bit EC ceiling and shared heap.
  These results do not qualify arbitrary certificate chains or RSA profiles.
- WPS PBC/PIN and SmartConfig provisioning, DHCP and ESPTouch acknowledgement
  pass against the external fixture. SmartConfig uses hardware NMI; its redundant
  polling path is removed. Retained promiscuous polling restores the NMI gate
  that the native FIQ handler expects its hardware return to restore. SmartConfig
  also passes fresh DHCP after stop and a 12-second quiet interval.
- The latest validation checks pass all 27 host and 22 target lanes. Independent
  changed-batch reviews report no new material code findings. The AX50 restoration
  assertions are all true and the temporary Pi listener exited with status zero.
  Board identity uses physical USB paths and checked chip identities because the
  adapters share a serial identifier; reserved/calibration regions remain tied to
  each board's own backup. Both reserved tails match, and a final capture verifies
  the AP/DHCP-client firmware left running on the boards.
