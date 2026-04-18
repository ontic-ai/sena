# Daemon-CLI Split Refactor — Tracking Document
**Status:** Active development (M-Split)  
**Target branch:** dev  
**Issue:** #68 Group 11  
**Created:** 2026-04-18

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

- [ ] Add `SystemEvent::LoopControlRequested { loop_name, enabled }` bus event
- [ ] Add `SystemEvent::LoopStatusChanged { loop_name, enabled }` bus event
- [ ] Add `IpcPayload::LoopStatusUpdate { loop_name, enabled }` type
- [ ] Add `IpcPayload::ShutdownRequested` type
- [ ] IPC server: maintain loop registry (`loop_states: HashMap<&'static str, bool>`)
- [ ] IPC server: send 5+ `LoopStatusUpdate` messages on client Subscribe
- [ ] IPC server: propagate `LoopStatusChanged` bus events to all connected clients
- [ ] CLI: `/loops`, `/loops <name>`, `/loops <name> on|off` commands → IPC dispatch
- [ ] Actors (CTP, Memory, Platform, Speech): handle `LoopControlRequested`, broadcast `LoopStatusChanged`
- [ ] CLI sidebar: remove logo, add Loops section with colored status indicators (● green = enabled, ● red = disabled)
- [ ] Real-time loop status updates in TUI via `IpcPayload::LoopStatusUpdate`
- [ ] Wire `IpcPayload::ShutdownRequested` to `SystemEvent::ShutdownSignal` broadcast
- [ ] Verification: `/loops` toggles work, sidebar updates in real-time

### Final Verification

- [ ] All slash commands map 1:1 to IPC commands or bus events (no orphaned CLI logic)
- [ ] CLI crash → daemon unaffected (verified by integration test)
- [ ] Multiple CLI sessions → concurrent, no conflicts (verified by integration test)
- [ ] Daemon crash → all CLI sessions detect disconnect and exit gracefully
- [ ] System tray "Open CLI" → spawns new terminal with `sena cli`, connects to running daemon
- [ ] `cargo build --workspace` clean
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` clean
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

### Phase 2: Daemon Process and IPC Server Integration (In Progress)
**Date:** 2026-04-18 (Active)  
**Commit(s):** TBD

**Summary:**

Phase 2 creates the daemon binary (`crates/daemon`) as a separate process that owns all actors, runs the supervision loop, and provides an IPC server for command dispatch. The CLI remains in-process for this phase (Phase 3+ will convert CLI to IPC client).

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

**Phase 2 Limitations (tracked for Phase 3+ resolution):**
1. **Tray icon**: No assets/logo.ico file exists. Using magenta fallback. ICO decoding not implemented.
   - To fix: Add logo.ico to assets/, use `include_bytes!` for embedded asset, implement ICO decode or use `ico` crate.
2. **CLI launch**: Temporary Phase 2 behavior — daemon and CLI are the same binary ("sena"). Launch action spawns "sena cli" in new terminal.
   - Path handling is safe (no unwrap), but non-UTF8 paths will fail with clear error.
   - To fix in Phase 3: Separate CLI binary, update launch logic to spawn the renamed CLI binary.
3. **Supervision readiness**: `runtime_state.mark_ready()` is called immediately after boot, not after supervision confirms all actors are healthy.
   - To fix in Phase 3: Wire `wait_for_readiness()` function, wait for all expected actors to emit `ActorReady`, then mark ready.
4. **runtime.status**: Returns `{"status": "booting"|"ready", "uptime_seconds": N}` based on simple ready flag, not actual actor health.
   - To fix in Phase 3: Query supervisor for per-actor health, return structured status with actor states.
5. **runtime.shutdown**: Sends signal to private shutdown channel. Daemon main loop broadcasts `ShutdownInitiated` on shutdown.
   - To fix in Phase 3: Also broadcast `ShutdownRequested` event on bus from the handler for observability before sending private signal.
6. **Command handlers not fully wired**: Most command handlers return `CommandNotReady` errors with clear messages:
   - `inference.load_model`, `inference.run` — inference dispatch via bus events not wired
   - `speech.listen_start`, `speech.listen_stop` — speech loop control via bus events not wired
   - `memory.query` — memory query via bus events not wired
   - `config.set` — config write subsystem not wired
   - `events.subscribe`, `events.unsubscribe` — event subscription mechanism not wired
   - To fix in Phase 3+: Wire bus event dispatch, implement response collection, return real results.
7. **CLI still in-process**: CLI boots runtime directly. No IPC connection.
   - To fix in Phase 4: CLI detects running daemon, connects via IPC, dispatches commands, renders responses.

**Architecture compliance fixes applied (arch-guard blockers resolved):**
- [x] Removed `anyhow` dependency from daemon — replaced with typed `DaemonError` enum
- [x] Removed `unwrap()` from `launch_cli()` — safe UTF-8 path conversion with clear error
- [x] Fixed `commands/mod.rs` violation — replaced with `commands/handlers.rs` module
- [x] Tray separator — using `PredefinedMenuItem::separator()` instead of disabled menu item

**Prompt alignment fixes applied:**
- [x] `runtime.ping` returns `{"pong": true, "uptime_seconds": N}` as specified
- [x] `runtime.status` returns clearly Phase-2-temporary structure (simple ready flag, not actor health)
- [x] All expected command names registered (even if some return `CommandNotReady`)
- [x] Tray limitations documented in code comments and this scratch file
- [x] CLI launch behavior documented with Phase 2 temporariness noted

**Critical decisions:**
1. **Daemon error handling**: Created typed `DaemonError` enum instead of using anyhow. All error conversions explicit.
2. **Command registration**: All 14 expected command names registered, even if not fully implemented. Unimplemented commands return `IpcError::CommandNotReady` with descriptive messages.
3. **Tray message pump**: Explicit acknowledgment that tray-icon does NOT pump Windows messages internally — daemon must do it in the tray loop.
4. **Supervisor readiness**: Deferred to Phase 3. Phase 2 uses simple boolean flag set immediately after boot.

**Follow-up work for Phase 3 (Supervision Readiness Fix):**
- Implement `wait_for_readiness()` in `crates/runtime/src/supervisor.rs`
- Track expected actors via `Runtime.expected_actors` field
- Move `BootComplete` broadcast to post-readiness (after supervision confirms all actors healthy)
- Wire `runtime.status` to query supervisor for per-actor health
- Wire `runtime.shutdown` to also broadcast `ShutdownRequested` on bus before private channel signal

**Follow-up work for Phase 4 (CLI as IPC Client):**
- Implement daemon detection in CLI (check pipe existence + connectivity)
- Implement IPC client connection in CLI (`run_with_ipc()` replaces `run_with_runtime()`)
- Map all slash commands to IPC command dispatch
- Handle daemon not running: auto-start daemon, wait for readiness, connect
- Remove CLI's in-process runtime boot path entirely
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

### Phase 1: Supervision Loop

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

### Phase 2: IPC Protocol

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
