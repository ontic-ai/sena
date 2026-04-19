# Daemon-CLI Split Refactor — Tracking Document
**Status:** Active development (M-Split)  
**Target branch:** dev  
**Issue:** #68 Group 11  
**Created:** 2026-04-18

---

## Workflow Ledger

This section is the canonical recovery ledger for the daemon/CLI split workflow. If any legacy checklist or template section below drifts, this ledger wins.

### Approved Workflow Defaults (2026-04-19)

- Start every session from `dev` and recover state from this file first.
- Create detailed GitHub issues up front, but create the physical git branch only when that batch becomes active.
- Default execution is sequential: one active batch branch at a time.
- If recovery starts off `dev` with uncommitted work, stash it or checkpoint it on the current branch. Do not copy unfinished feature code onto `dev`.
- CI failures are temporarily waived for all PRs until the CI-failure follow-up issues are addressed. Merges may proceed under that waiver only when the ledger records the waiver state and the follow-up issues explicitly exist.
- Merge plans for blocked PRs go in the local-only ignored directory `docs/_scratch/local/`.
- Session PRs wait until the full session queue is open before merging.
- Session PRs merge into `dev` with merge commits.

### Current Session Queue

| Order | Batch | Issue | Branch | Status | PR | Notes |
|---|---|---|---|---|---|---|
| 1 | Loop registry and real-time control | #68 | `feat/loop-registry` | Merged to `dev` under temporary CI waiver | #80 | Merged at commit `929e58dd86f22c65e9fd652fd370cd2221501c7f`. CI follow-up issues filed: #81 (Windows speech crash), #82 (macOS CoreGraphics build), #83 (Ubuntu glib dependency). `cargo fmt --check` remains blocked by pre-existing unrelated debt. |

### Recovery Notes

- Canonical PR from this session: #80 → `feat/loop-registry` targeting `dev`, merged on 2026-04-19.
- Merge status: PR #80 had no git conflict with `dev` and was merged under the temporary CI waiver after dedicated CI follow-up issues were filed.
- Temporary policy override: CI failures are currently waived for all PRs until issues #81, #82, and #83 are addressed and the waiver is explicitly removed from this ledger.
- Duplicate legacy docs issue detected during recovery: #67. Reuse #68 as the canonical issue for Group 11.
- Local-only merge/conflict plans must be written under `docs/_scratch/local/` and kept out of Git history.

---

## Refactor Context

### Objective

Split the monolithic Sena CLI from the runtime into two independent processes:

1. **Daemon process** (`sena` with no args) — owns all actors, boots runtime, runs supervision loop, persists across sessions
2. **CLI process** (`sena cli`) — thin wrapper that connects to daemon over IPC, dispatches typed bus events, renders responses

### Rationale

The pre-M-Split architecture tightly coupled CLI and runtime:
- CLI invoked `runtime::boot_ready()` which spawned all actors in-process
- CLI crash → all actors terminated, persistent state at risk
- No ability to run multiple CLI sessions against a single runtime
- No ability to inspect/control Sena when CLI is closed

The post-M-Split architecture:
- Daemon is the single source of truth for all actors and persistent state
- CLI becomes a disposable interface — multiple instances can attach simultaneously
- CLI crash isolated — daemon continues uninterrupted
- System tray menu can spawn new CLI sessions on-demand
- Foundation for future non-CLI interaction surfaces (web UI, mobile companion, etc.)

### Design Contract

**CLI is a wrapper, not an owner.**
- CLI dispatches events via IPC, renders responses
- CLI never constructs actors
- CLI never duplicates business logic that daemon actors already provide
- Every CLI slash command maps to exactly one IPC command or bus event

See `architecture.md §4.3` and `copilot-instructions.md §8.1` for full design principles.

### Architecture Changes

**New crates:**
- `crates/ipc` — leaf crate (no dependencies on other Sena crates) providing wire protocol, command dispatch, server/client APIs

**Modified crates:**
- (Phase 1: none — `crates/ipc` is standalone)
- (Phases 2+: `crates/runtime`, `crates/cli`, `crates/bus`)

**Dependency graph changes:**
- CLI depends on `runtime` (for re-exported IPC utilities) and `bus` (for event types)
- CLI no longer constructs concrete actor instances → removed direct imports of `soul`, `memory`, `inference`, `platform`, etc.

**Boot sequence changes:**
- Daemon mode: `runtime::run_background()` → boot → readiness → BootComplete → supervision loop
- CLI mode (Phase 6+): `sena cli` → detect daemon → if running: connect IPC; if not: auto-start daemon → connect IPC
- CLI mode (pre-Phase 6, removed): `sena cli` → `runtime::boot_ready()` → all actors in-process

---

## Refactor Phases

### Phase 1: IPC Foundation — Wire Protocol and Command Dispatch
**Scope:** Foundational infrastructure (issues #61, #62, #63)

- [x] Create `crates/ipc` as leaf crate (no dependencies on other Sena crates)
- [x] Define pipe name constant `PIPE_NAME = r"\\.\pipe\sena"` as single source of truth
- [x] Implement async framing functions (`write_frame`, `read_frame`) with 4-byte little-endian length prefix
- [x] Define `IpcRequest` and `IpcResponse` envelope types
- [x] Implement `CommandHandler` trait with `name()`, `description()`, `requires_boot()`, `async handle()`
- [x] Implement `CommandRegistry` with duplicate registration panic, async dispatch
- [x] Add built-in `list_commands` meta-handler
- [x] Implement `IpcServer` with Windows named pipe support, concurrent client handling
- [x] Implement `IpcClient` with `connect()`, `send()`, `daemon_running()`, `subscribe_events()` APIs
- [x] Platform gates: Windows implementation, non-Windows stubs return clear errors
- [x] Unit tests: framing round-trip, dispatch correctness, unknown command error, duplicate panic
- [x] Verification: `cargo build -p ipc` clean, `cargo test -p ipc` passes (7 tests)

### Phase 2: Supervision Loop and Process Lifetime Split
**Scope:** M-Refactor milestone (formerly Phase 1)
3: IPC Server Integration
**Scope:** M6.1 (IPC Runtime Server) (formerly Phase 2pervisor.rs`
- [ ] Implement `wait_for_readiness()` function (30s timeout, waits for all `expected_actors` to emit `ActorReady`)
- [ ] Implement `supervision_loop()` function (handles ShutdownSignal, CliAttachRequested, ActorFailed retry logic)
- [ ] Create `runtime::run_background()` public API (daemon entry point)
- [ ] Add `Runtime.expected_actors: Vec<&'static str>` field to track actors for readiness gate
- [ ] Move `BootComplete` broadcast from `boot::boot()` to post-readiness in supervision loop
- [ ] Add tray "Open CLI" menu item → broadcast `CliAttachRequested` event
- [ ] Implement `open_cli_in_new_terminal()` platform-specific function
- [ ] Remove all diagnostic `eprintln!` from production paths
- [ ] CLI: remove `run_with_boot()`, `run_headless()`, `do_shutdown()`, `open_cli_session()`
- [ ] CLI: add `run_with_runtime()` function (pre-IPC path for M-Refactor)
- [ ] CLI main: `None =>` calls `runtime::run_background()`, `Some("cli") =>` calls pre-IPC boot path
- [ ] Add post-boot TTS greeting when `config.speech_enabled`
- [ ] Verification: build, test, clippy clean

### Phase 2: IPC Protocol and Server
**Scope:** M6.1 (IPC Runtime Server)

- [ ] Define IPC transport (Unix domain socket on macOS/Linux, Named pipe on Windows)
- [ ] Add `IpcMessage`, `IpcPayload`, `LineStyle` types to `crates/bus/src/ipc.rs`
- [ ] Implement JSON-over-newline serialization protocol
- [ ] Create `crates/runtime/src/ipc_server.rs` module
- [ ] Implement IPC server task: listen, authenticate, handle Subscribe/Unsubscribe/Command
- [ ] Wire IPC server into boot sequence (spawn after core actors)
- [ ] Add socket file permissions verification (Unix) / pipe ACL (Windows)
- [ ] Implement broadcast bus event → IPC client forwarding
- [ ] Verification: daemon boots with IPC server listening, socket/pipe created

### Phase 4: CLI as IPC Client
**Scope:** M6.2 (CLI as Separate Process) (formerly Phase 3)

- [ ] Detect running daemon (check socket/pipe existence + connectivity)
- [ ] Implement IPC client connection in CLI
- [ ] CLI: `run_with_ipc()` function replaces `run_with_runtime()`
- [ ] Map all slash commands to IPC payloads (no orphaned CLI logic)
- [ ] Handle daemon not running: auto-start daemon, wait for IPC readiness (30s timeout), connect
- [ ] Handle daemon already running: connect directly
- [ ] Handle IPC disconnect: gracefully exit CLI, do not crash daemon
- [ ] Remove CLI's in-process runtime boot path entirely (no more `boot_ready()` calls)
- [ ] Multiple CLI session support: simultaneous connections without conflict
- [ ] Integration tests: `ipc_server_survives_client_disconnect`, `ipc_multiple_clients_connect_simultaneously`
- [ ] Verification: CLI crash does not affect daemon, multiple CLI instances work

### Phase 5: Loop Registry and Real-Time Control
**Scope:** M6.2.1 (Loop Registry and Visibility) (formerly Phase 4)

Recovered implementation note: the governed `feat/loop-registry` branch uses dedicated IPC commands `loops.list` / `loops.set` plus daemon push event type `loop_status_changed` rather than the older `IpcPayload::*` wording below.

- [x] Add `SystemEvent::LoopControlRequested { loop_name, enabled }` bus event
- [x] Add `SystemEvent::LoopStatusChanged { loop_name, enabled }` bus event
- [ ] Add `IpcPayload::LoopStatusUpdate { loop_name, enabled }` type
- [ ] Add `IpcPayload::ShutdownRequested` type
- [x] IPC server: maintain loop registry (`loop_states: HashMap<&'static str, bool>`)
- [ ] IPC server: send 5+ `LoopStatusUpdate` messages on client Subscribe
- [x] IPC server: propagate `LoopStatusChanged` bus events to all connected clients
- [x] CLI: `/loops`, `/loops <name>`, `/loops <name> on|off` commands → IPC dispatch
- [x] Actors (CTP, Memory, Platform, Speech, Inference): handle `LoopControlRequested`, broadcast `LoopStatusChanged`
- [x] CLI sidebar: remove logo, add Loops section with colored status indicators (● green = enabled, ● red = disabled)
- [ ] Real-time loop status updates in TUI via `IpcPayload::LoopStatusUpdate`
- [ ] Wire `IpcPayload::ShutdownRequested` to `SystemEvent::ShutdownSignal` broadcast
- [ ] Verification: `/loops` toggles work, sidebar updates in real-time

## Active Salvage Execution Checklist

- [x] Recover Phase 1 IPC foundation on governed branch `feat/bus-model-metadata`
- [x] Recover runtime-owned bootstrap foundations on governed branches `feat/runtime-download-manager-rehome` and `feat/runtime-onboarding-boot-logic`
- [x] Recover daemon onboarding and bootstrap IPC forwarding on governed branch `feat/daemon-ipc-forwarding`
- [x] Recover remaining supervision core on governed branch `feat/runtime-supervision`
- [x] Recover richer Phase 4 CLI IPC client on governed branch `feat/cli-ipc-client`
- [ ] Reconcile Phase 2 and Phase 4 checklist states against the governed branches and actual code paths
- [x] Recover or implement Phase 5 loop registry and real-time control on governed branch `feat/loop-registry`
- [ ] Run full nested workspace verification, architecture/security audits, reviewer, and PR handoff to `dev`

### Final Verification

- [ ] All slash commands map 1:1 to IPC commands or bus events (no orphaned CLI logic)
- [ ] CLI crash → daemon unaffected (verified by integration test)
- [ ] Multiple CLI sessions → concurrent, no conflicts (verified by integration test)
- [ ] Daemon crash → all CLI sessions detect disconnect and exit gracefully
- [ ] System tray "Open CLI" → spawns new terminal with `sena cli`, connects to running daemon
- [x] `cargo build --workspace` clean
- [x] `cargo test --workspace` passes
- [x] `cargo clippy --workspace -- -D warnings` clean
- [ ] `cargo fmt --check` clean
- [ ] Architecture doc §4.3 reflects new process lifetime model
- [ ] Architecture doc §2 dependency graph updated (CLI no longer depends on actor crates directly)
- [ ] Copilot instructions §8.1 reflects CLI design principle

---

## Phase Summaries

### Phase 1: IPC Foundation (Completed)
**Date:** 2026-04-18  
**Commit(s):** TBD (pending commit)

**Summary:**

Created `crates/ipc` as a foundational leaf crate providing the wire protocol and command dispatch infrastructure for daemon-CLI communication. The crate is fully decoupled from all other Sena workspace crates, making it reusable and testable in isolation.
TBD  
**Commit(s):** TBD

Summary: TBD

### Phase 4: CLI as IPC Client (Pending)
**Date:** TBD  
**Commit(s):** TBD

Summary: TBD

### Phase 5: Loop Registry (Pendingr
- Unknown command error handling
- Duplicate handler registration panic detection
- List commands meta-handler functionality
- Oversized frame rejection
- Connection closed detection

**Critical decisions:**
1. **Leaf crate architecture**: No dependencies on `bus`, `runtime`, or any other Sena crate. Protocol is generic and self-contained.
2. **Command handler trait design**: `requires_boot()` defaulting to `true` enables pre-boot commands (e.g., "ping", "status") to opt-out.
3. **Built-in list_commands**: Phase 1 implementation is a placeholder; Phase 2+ will provide full registry introspection.
4. **Platform stubs**: Non-Windows platforms return clear `PlatformNotSupported` errors rather than silent no-ops.

**Follow-up constraints for Phase 2:**
- Wire IPC server into runtime boot sequence (spawn after encryption init, before actors)
- Implement concrete command handlers in `crates/runtime` (inference, list_models, shutdown, etc.)
- Add `CommandRegistry` to `Runtime` struct for handler registration during boot
- CLI must use `ipc::IpcClient` to communicate with daemon; no in-process boot path

### Phase 2: Daemon Process and IPC Server Integration (Completed)
**Date:** 2026-04-18  
**Commit(s):** TBD (pending commit)

**Summary:**

Phase 2 creates the daemon binary (`crates/daemon`) as a separate process that owns all actors, runs the supervision loop, and provides an IPC server for command dispatch. The CLI remains in-process for this phase (Phase 3+ will convert CLI to IPC client).

This phase also fixes critical readiness/health semantics:
- Boot gate now only decides WHEN BootComplete fires — health tracking continues for process lifetime
- ActorReady events update health unconditionally at any time, not just during boot window
- 30-second timeout emits BootComplete anyway with a warning for missing actors
- Missing actors remain in Starting state (not marked Failed) and transition to Running when ActorReady arrives later
- Daemon waits for BootComplete before marking runtime as ready (not immediately after boot)
- runtime.status returns actual per-actor health via HealthCheckRequest/Response pattern
- runtime.shutdown broadcasts ShutdownRequested on bus for observability

**Implemented:**
- Created `crates/daemon` binary crate with typed error handling (no anyhow)
- Daemon boots runtime via `runtime::boot()` and spawns supervision loop
- IPC server integration: `IpcServer` runs on Windows named pipe `\\.\pipe\sena`
- Command registry with 14 registered handlers covering all expected Phase 2 command names:
  - Runtime: `runtime.ping`, `runtime.status`, `runtime.shutdown`
  - Inference: `inference.list_models`, `inference.load_model`, `inference.status`, `inference.run`
  - Speech: `speech.listen_start`, `speech.listen_stop`, `speech.status`
  - Memory: `memory.stats`, `memory.query`
  - Config: `config.get`, `config.set`
  - Events: `events.subscribe`, `events.unsubscribe`
- System tray implementation (Windows only):
  - Main-thread tray loop with menu: Launch CLI, Config Editor, Open Models Folder, Shutdown
  - Uses `PredefinedMenuItem::separator()` for proper menu separator
  - Magenta fallback icon (32x32 solid color) — no .ico asset available in Phase 2
  - Explicit Windows message pump handling (50ms polling loop)
- Runtime state tracking: boot time, ready flag for uptime and status queries
- Graceful shutdown: shutdown command sends signal to daemon main loop, which broadcasts `ShutdownInitiated` on bus
- **ActorStatus enum extended** with `Starting` state to distinguish actors that haven't yet signaled ready
- **ActorRegistry** now starts actors in Starting state and transitions to Running on ActorReady
- **Supervision loop** processes ActorReady events throughout process lifetime, not just during boot
- **Readiness gate timeout** emits BootComplete with warning instead of marking missing actors as Failed
- **Daemon BootComplete subscription** waits for BootComplete event before marking runtime ready
- **runtime.status handler** queries supervisor for per-actor health via HealthCheckRequest/Response
- **runtime.shutdown handler** broadcasts ShutdownRequested on bus before sending private shutdown signal

**Phase 2 Limitations (tracked for Phase 3+ resolution):**
1. **Tray icon**: No assets/logo.ico file exists. Using magenta fallback. ICO decoding not implemented.
   - To fix: Add logo.ico to assets/, use `include_bytes!` for embedded asset, implement ICO decode or use `ico` crate.
2. **CLI launch**: Temporary Phase 2 behavior — daemon and CLI are the same binary ("sena"). Launch action spawns "sena cli" in new terminal.
   - Path handling is safe (no unwrap), but non-UTF8 paths will fail with clear error.
   - To fix in Phase 3: Separate CLI binary, update launch logic to spawn the renamed CLI binary.
3. **Command handlers not fully wired**: Most command handlers return `CommandNotReady` errors with clear messages:
   - `inference.load_model`, `inference.run` — inference dispatch via bus events not wired
   - `speech.listen_start`, `speech.listen_stop` — speech loop control via bus events not wired
   - `memory.query` — memory query via bus events not wired
   - `config.set` — config write subsystem not wired
   - `events.subscribe`, `events.unsubscribe` — event subscription mechanism not wired
   - To fix in Phase 3+: Wire bus event dispatch, implement response collection, return real results.
4. **CLI still in-process**: CLI boots runtime directly. No IPC connection.
   - To fix in Phase 4: CLI detects running daemon, connects via IPC, dispatches commands, renders responses.

**Architecture compliance fixes applied (arch-guard blockers resolved):**
- [x] Removed `anyhow` dependency from daemon — replaced with typed `DaemonError` enum
- [x] Removed `unwrap()` from `launch_cli()` — safe UTF-8 path conversion with clear error
- [x] Fixed `commands/mod.rs` violation — replaced with `commands/handlers.rs` module
- [x] Tray separator — using `PredefinedMenuItem::separator()` instead of disabled menu item

**Prompt alignment fixes applied:**
- [x] `runtime.ping` returns `{"pong": true, "uptime_seconds": N}` as specified
- [x] `runtime.status` returns per-actor health with structured ActorStatus (Starting/Running/Stopped/Failed)
- [x] `runtime.shutdown` broadcasts ShutdownRequested on bus before private shutdown signal
- [x] All expected command names registered (even if some return `CommandNotReady`)
- [x] Tray limitations documented in code comments and this scratch file
- [x] CLI launch behavior documented with Phase 2 temporariness noted
- [x] Boot gate timeout behavior fixed: BootComplete fires anyway, missing actors remain in Starting state
- [x] ActorReady events processed throughout process lifetime, not just during boot window
- [x] Daemon waits for BootComplete before marking ready, not immediately after boot

**Critical decisions:**
1. **Daemon error handling**: Created typed `DaemonError` enum instead of using anyhow. All error conversions explicit.
2. **Command registration**: All 14 expected command names registered, even if not fully implemented. Unimplemented commands return `IpcError::CommandNotReady` with descriptive messages.
3. **Tray message pump**: Explicit acknowledgment that tray-icon does NOT pump Windows messages internally — daemon must do it in the tray loop.
4. **ActorStatus extension**: Added `Starting` state to distinguish actors that haven't yet emitted ActorReady from Running actors. All actors start in Starting state.
5. **Readiness semantics**: Boot gate only decides WHEN BootComplete fires. Health tracking is ongoing for process lifetime. Late ActorReady events transition actors from Starting to Running.
6. **Health query pattern**: runtime.status uses existing HealthCheckRequest/Response bus events to query supervisor for real actor health, not a simple boolean flag.

**Follow-up work for Phase 3 (CLI as IPC Client):**
- Implement daemon detection in CLI (check pipe existence + connectivity)
- Implement IPC client connection in CLI (`run_with_ipc()` replaces `run_with_runtime()`)
- Map all slash commands to IPC command dispatch
- Handle daemon not running: auto-start daemon, wait for readiness, connect
- Remove CLI's in-process runtime boot path entirely

**Follow-up work for Phase 4 (Loop Registry):**
- Add SystemEvent::LoopControlRequested/LoopStatusChanged
- IPC server loop registry with real-time status updates to connected clients
- CLI `/loops` commands and sidebar loop status display

**Verification:**
- `cargo build -p sena`: clean
- `cargo test -p runtime`: 26 tests passed
- `cargo clippy -p sena -- -D warnings`: clean
- `cargo fmt --check`: clean
- **Live smoke test**: Daemon boots, all 8 actors report ready within <1ms, BootComplete fires immediately without timeout, no "readiness gate timeout" warning
  - Boot time: 0ms (was 30,011ms pre-fix)
  - Expected log sequence observed:
    ```
    SUPERVISOR: waiting up to 30s for 8 actors
    SUPERVISOR: ActorReady received actor="memory"
    SUPERVISOR: ActorReady received actor="inference"
    SUPERVISOR: ActorReady received actor="platform"
    SUPERVISOR: ActorReady received actor="soul"
    SUPERVISOR: ActorReady received actor="stt"
    SUPERVISOR: ActorReady received actor="tts"
    SUPERVISOR: ActorReady received actor="prompt"
    SUPERVISOR: ActorReady received actor="ctp"
    SUPERVISOR: all actors ready
    SUPERVISOR: readiness gate passed
    SUPERVISOR: broadcasting BootComplete
    SUPERVISOR: BootComplete broadcast successful boot_time_ms=0
    ```

**Readiness race condition fix (2026-04-18):**

The initial Phase 2 implementation had a confirmed race condition where the supervisor subscribed to ActorReady events AFTER actors had already been spawned and emitted those events. This resulted in a false 30-second timeout on every boot.

**Root cause:**
1. `boot()` subscribed to broadcast channel, spawned all actors
2. Actors immediately emitted `ActorReady` after `start()` completed
3. `await_readiness_gate()` called `subscribe_broadcast()` AFTER actors spawned
4. Early `ActorReady` events were missed, supervisor waited 30s, then timed out with all 8 actors marked as missing

**Fix applied:**
1. Subscribe to broadcast channel in `boot()` BEFORE spawning any actors (step 3, after EventBus init)
2. Store the pre-subscribed receiver in `BootResult.readiness_rx: Option<Receiver<Event>>`
3. `await_readiness_gate()` takes ownership of that receiver (via `take()`) instead of subscribing
4. Update `actor_registry.mark_running()` as each ActorReady arrives during the gate
5. Fixed CTP actor name mismatch: actor returned `"CtpActor"` but boot.rs expected `"ctp"`

**Changed files:**
- `crates/runtime/src/boot.rs`: Added `readiness_rx` field to BootResult, subscribed before actor spawn
- `crates/runtime/src/supervisor.rs`: Changed `await_readiness_gate` to consume readiness_rx, update actor_registry during gate
- `crates/ctp/src/actor.rs`: Changed actor name from `"CtpActor"` to `"ctp"` to match expected_actors list

**Result:** Readiness gate now completes in <1ms with all actors accounted for. Boot time reduced from 30,011ms to 0ms.
- Manual tray inspection: NOT YET VERIFIED (requires Windows GUI environment)

**Phase 2 status:** COMPLETE
**Date:** 2026-04-18  
**Commit(s):** TBD (pending commit)

**Summary:**

Created `crates/daemon/` as a standalone binary package that owns the Sena runtime lifecycle.
The daemon boots the runtime, registers IPC command handlers, spawns the IPC server on a
background Tokio task, and provides a system tray with menu items for user interaction.

**Implemented:**

1. **Package structure:**
   - `crates/daemon/Cargo.toml` with package name `sena`, binary name `sena`
   - Dependencies: runtime, ipc, bus, tokio, tracing, async-trait, anyhow, serde_json, tray-icon

2. **Command handler modules** (`crates/daemon/src/commands/`):
   - `runtime_commands.rs`: PingHandler, StatusHandler, ShutdownHandler
   - `inference_commands.rs`: ListModelsHandler, RunInferenceHandler (stubs with typed errors)
   - `speech_commands.rs`: SpeechStatusHandler (stub)
   - `memory_commands.rs`: MemoryStatsHandler (stub)
   - `config_commands.rs`: ConfigGetHandler, ConfigSetHandler (stubs with typed errors)
   - `events_commands.rs`: EventsSubscribeHandler, EventsUnsubscribeHandler (stubs)
   - `mod.rs`: `register_all()` function registers 11 command handlers with IPC registry

3. **RuntimeState shared state:**
   - Daemon-owned struct with boot time, readiness flag
   - Shared across command handlers via Arc cloning
   - Passed to StatusHandler to report uptime and readiness status

4. **System tray module** (`crates/daemon/src/tray.rs`):
   - Main-thread tray loop (Windows only)
   - Menu items: Launch CLI, Config Editor, Open Models Folder, Shutdown Sena
   - Tooltip updates via `std::sync::mpsc` channel
   - Icon loading from `assets/logo.ico` with magenta 32x32 fallback on failure
   - Message pump handling via sleep loop (tray-icon handles Windows messages internally)

5. **Daemon main.rs:**
   - Boots runtime via `runtime::boot()`
   - Registers all IPC command handlers
   - Spawns IPC server in Tokio background task
   - Updates tray tooltip ("Sena — Booting..." → "Sena — Ready")
   - Handles tray actions (Launch CLI, Open Models, Shutdown)
   - Runs supervision loop in background
   - Blocks on main thread tray loop
   - Handles graceful shutdown on tray exit or shutdown command

6. **IPC error variants added:**
   - `InvalidPayload`: command payload validation failures
   - `CommandNotReady`: daemon not booted or feature not implemented
   - `Internal`: internal daemon errors (e.g., shutdown channel closed)

7. **Dependency fixes:**
   - Fixed clippy errors in `inference`, `prompt`, `soul`, `ctp` crates
   - All workspace crates now pass `cargo clippy -- -D warnings`

**Critical decisions:**

1. **Temporary binary name collision:**
   - Both `crates/daemon` and `crates/cli` produce a binary named `sena`
   - This is acceptable for Phase 2; Phase 3 will rename CLI binary to `sena-cli` or similar
   - Logged in this doc as a known limitation for Phase 2

2. **Command handler stubs with typed errors:**
   - Handlers that require capabilities not yet implemented (inference dispatch, config subsystem,
     event subscription) return `IpcError::CommandNotReady` with descriptive messages rather
     than fake success responses
   - This preserves type safety and makes the limitation observable to CLI clients

3. **Daemon-owned shared state:**
   - `RuntimeState` is a minimal shared state struct owned by daemon
   - Populated from boot result and bus events (readiness signal)
   - Sufficient for Phase 2 status/ping commands without violating actor isolation
   - Phase 4 will move command registration into owning crates; this is intentionally deferred

4. **Tray icon loading limitation:**
   - ICO decoding not implemented in Phase 2
   - Always falls back to magenta 32x32 solid-color icon
   - `assets/logo.ico` path logic is present but unused until ICO decoder added

5. **Launch CLI action:**
   - Launches `sena.exe cli` in new console window
   - Temporarily assumes CLI binary is also named `sena.exe` (same collision as above)
   - Will be updated in Phase 3 when CLI binary is renamed

**Verification:**

- `cargo build -p sena`: clean
- `cargo clippy -p sena -- -D warnings`: clean
- `cargo fmt -p sena --check`: clean
- No tests added (command handler logic is minimal stubs; real tests come in Phase 3+ integration)

**Follow-up constraints for next Phase 2 unit (readiness/health timeout fix):**

- Daemon currently marks `runtime_state.mark_ready()` immediately after boot without waiting
  for supervision readiness gate to pass
- StatusHandler reports "ready" immediately, which is inaccurate if actors are still booting
- Next unit must wire supervision `BootComplete` event to daemon state so StatusHandler reflects
  true readiness

**Phase 2 remaining units:**

- Readiness/health timeout fix (wire supervision BootComplete to daemon RuntimeState)
- Full Phase 2 verification (daemon runs, IPC server accepts connections, ping/status work)
- Update this doc with Phase 2 complete summary

---

### Phase 3: CLI as IPC Client (Pending)
**Date:** TBD  
**Commit(s):** TBD
### D2: IPC Crate as Leaf Node
**Date:** 2026-04-18  
**Context:** Phase 1 implementation choice — should the IPC protocol and framing logic live in `crates/bus` (alongside event types), `crates/runtime` (where the server will be wired), or in a dedicated `crates/ipc`?

**Decision:** Create `crates/ipc` as a leaf crate with zero dependencies on other Sena workspace crates.

**Rationale:**
- **Separation of concerns**: IPC wire protocol is orthogonal to event bus semantics. Bus owns typed events; IPC owns serialized command envelopes.
- **Testability**: Leaf crate can be tested in isolation without spinning up actors, bus, or runtime.
- **Reusability**: Protocol and framing layer are generic — no Sena-specific types in the API surface. Could be extracted as a standalone crate.
- **Dependency direction**: Keeps the graph clean. `runtime` depends on `ipc` to instantiate server; `cli` depends on `ipc` to instantiate client. Neither `ipc` nor `bus` depend on each other.
- **Prevents circular dependencies**: If IPC lived in `bus`, `runtime` would need to import `bus` for the server, which it already does for events — this blurs the line. If IPC lived in `runtime`, `cli` would need to import `runtime` just for the client API, which violates CLI's thin-wrapper design.

**Alternative considered:**
- Putting protocol types in `crates/bus/src/ipc.rs` and framing/server/client in `crates/runtime/src/ipc/`. Rejected because it splits the abstraction across two crates and makes testing harder.


Summary: TBD

### Phase 3: IPC Server Integration (Pending)
**Date:** 2026-04-XX  
**Commit(s):** TBD

Summary: TBD

### Phase 3: CLI as IPC Client (Completed)
**Date:** 2026-04-XX  
**Commit(s):** TBD

Summary: TBD

### Phase 4: Loop Registry (Completed)
**Date:** 2026-04-XX  
**Commit(s):** TBD

Summary: TBD

---

## Decision Log

### D1: Tracking File Location
**Date:** 2026-04-18  
**Context:** IPC Foundation

**Build:**
```
# Command: cargo build -p ipc
# Date: 2026-04-18
# Result: PASS
# Notes: Clean build with no warnings after fixing unused import and dead field.
```

**Tests:**
```
# Command: cargo test -p ipc
# Date: 2026-04-18
# Result: PASS (7 tests, 0 failures)
# Coverage:
#   - write_frame_then_read_frame_round_trips_correctly
#   - read_frame_returns_connection_closed_on_eof
#   - write_frame_rejects_oversized_payload
#   - dispatch_routes_to_correct_handler
#   - unknown_command_returns_error
#   - duplicate_registration_panics
#   - list_commands_returns_all_registered_handlers
```

**Clippy:**
```
# Command: cargo clippy -p ipc -- -D warnings
# Date: 2026-04-18
# Result: PASS
# Notes: Clean, no warnings.
```

**Fmt:**
```
# Command: cargo fmt -p ipc -- --check
# Date: 2026-04-18
# Result: PASS
# Notes: Formatting applied via `cargo fmt -p ipc`, all files now compliant.
```

### Phase 2: Daemon-CLI split is being implemented in the nested workspace (`sena/sena`) on the dev branch. The refactor is isolated from the top-level workspace. Need to decide where to place the refactor tracking file.

**Decision:** Place tracking file at `sena/docs/_scratch/daemon-cli-split.md` in the nested workspace.

**Rationale:**
- The nested workspace is the refactor target — all code changes happen there
- Tracking file must be versioned alongside the refactored code on the same dev branch
- Top-level workspace docs (`docs/`) remain stable; nested workspace has independent governance during dev
- Once the refactor is complete and merged, the nested workspace becomes the canonical source, and this tracking file becomes the historical record of the migration

**Status:** Implemented

---

## Verification Log

### Phase 1: IPC Foundation

**Build:**
```
# Command: cargo build -p ipc
# Date: 2026-04-18
# Result: PASS
# Notes: Clean build with no warnings after fixing unused import and dead field.
```

**Tests:**
```
# Command: cargo test -p ipc
# Date: 2026-04-18
# Result: PASS (7 tests, 0 failures)
# Coverage:
#   - write_frame_then_read_frame_round_trips_correctly
#   - read_frame_returns_connection_closed_on_eof
#   - write_frame_rejects_oversized_payload
#   - dispatch_routes_to_correct_handler
#   - unknown_command_returns_error
#   - duplicate_registration_panics
#   - list_commands_returns_all_registered_handlers
```

**Clippy:**
```
# Command: cargo clippy -p ipc -- -D warnings
# Date: 2026-04-18
# Result: PASS
# Notes: Clean, no warnings.
```

**Fmt:**
```
# Command: cargo fmt -p ipc -- --check
# Date: 2026-04-18
# Result: PASS
# Notes: Formatting applied via `cargo fmt -p ipc`, all files now compliant.
```

---

### Phase 2: Daemon Process and IPC Server Integration

**Build:**
```
# Command: cargo build -p sena
# Date: 2026-04-18
# Result: PASS
# Notes: Clean build after readiness/health fixes.
```

**Tests:**
```
# Command: cargo test -p runtime
# Date: 2026-04-18
# Result: PASS (26 tests, 0 failures)
# Coverage:
#   - All builder tests (8 actors)
#   - All health registry tests (mark_running_transitions_from_starting, etc.)
#   - Supervisor tests (readiness_gate_passes_with_no_actors, supervision_loop_completes_with_no_actors)
#   - Boot sequence tests (boot_completes_successfully, spawn_actors_creates_expected_list)
#   - IPC server tests (ipc_server_constructs, ipc_server_receives_commands, spawn_ipc_server_works)
```

**Clippy:**
```
# Command: cargo clippy -p sena -- -D warnings
# Date: 2026-04-18
# Result: PASS
# Notes: Clean, no warnings across all dependencies.
```

**Fmt:**
```
# Command: cargo fmt --check
# Date: 2026-04-18
# Result: PASS
# Notes: Formatting applied via `cargo fmt`, all files compliant.
```

**Manual tray inspection:**
```
# Date: TBD
# Result: NOT YET VERIFIED
# Notes: Requires Windows GUI environment. Deferred to pre-merge verification.
# Expected behavior:
#   - Tray icon appears in system tray (magenta square fallback)
#   - Tooltip updates from "Sena — Booting..." to "Sena — Ready" after BootComplete
#   - Menu items: Launch CLI, Config Editor, Open Models Folder, Shutdown Sena
#   - Shutdown action closes daemon cleanly
```

---

### Phase 3: CLI as IPC Client

**Build:**
```
# Command: cargo build --workspace
# Date: TBD
# Result: PASS/FAIL
# Notes: TBD
```

**Tests:**
```
# Command: cargo test --workspace
# Date: TBD
# Result: PASS/FAIL (N tests, M failures)
# Notes: TBD
```

**Clippy:**
```
# Command: cargo clippy --workspace -- -D warnings
# Date: TBD
# Result: PASS/FAIL
# Notes: TBD
```

**Fmt:**
```
# Command: cargo fmt --check
# Date: TBD
# Result: PASS/FAIL
# Notes: TBD
```

---

### Phase 3: CLI as IPC Client

**Build:**
```
# Command: cargo build --workspace
# Date: TBD
# Result: PASS/FAIL
# Notes: TBD
```

**Tests:**
```
# Command: cargo test --workspace
# Date: TBD
# Result: PASS/FAIL (N tests, M failures)
# Notes: TBD
```

**Clippy:**
```
# Command: cargo clippy --workspace -- -D warnings
# Date: TBD
# Result: PASS/FAIL
# Notes: TBD
```

**Fmt:**
```
# Command: cargo fmt --check
# Date: TBD
# Result: PASS/FAIL
# Notes: TBD
```

---

### Phase 4: Loop Registry

**Build:**
```
# Command: cargo build --workspace
# Date: TBD
# Result: PASS/FAIL
# Notes: TBD
```

**Tests:**
```
# Command: cargo test --workspace
# Date: TBD
# Result: PASS/FAIL (N tests, M failures)
# Notes: TBD
```

**Clippy:**
```
# Command: cargo clippy --workspace -- -D warnings
# Date: TBD
# Result: PASS/FAIL
# Notes: TBD
```

**Fmt:**
```
# Command: cargo fmt --check
# Date: TBD
# Result: PASS/FAIL
# Notes: TBD
```

---

### Final Verification

**Build:**
```
# Command: cargo build --workspace
# Date: TBD
# Result: PASS/FAIL
# Notes: TBD
```

**Tests:**
```
# Command: cargo test --workspace
# Date: TBD
# Result: PASS/FAIL (N tests, M failures)
# Notes: TBD
```

**Clippy:**
```
# Command: cargo clippy --workspace -- -D warnings
# Date: TBD
# Result: PASS/FAIL
# Notes: TBD
```

**Fmt:**
```
# Command: cargo fmt --check
# Date: TBD
# Result: PASS/FAIL
# Notes: TBD
```

---

## Notes

- This tracking file is intentionally placed in the nested workspace (`sena/docs/_scratch/`) rather than the top-level workspace to version it alongside the refactored code on the dev branch.
- All checklist items correspond to work that was completed across M-Refactor and Phase 6 milestones in the top-level workspace ROADMAP.md. This document serves as the historical record of the migration to the nested workspace structure.
- The "Phase Summaries" and "Verification Log" sections are templates to be filled in as each phase is completed and verified in the nested workspace context.
