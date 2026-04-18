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
- None (IPC server lives in `crates/runtime`, protocol types in `crates/bus`)

**Modified crates:**
- `crates/runtime` — added `ipc_server.rs`, `supervisor.rs`; split `run_background()` (daemon) from `boot_ready()` (removed)
- `crates/cli` — removed `run_with_boot()`, `run_headless()`, `do_shutdown()`, `open_cli_session()`; added `run_with_ipc()`; removed in-process runtime boot path
- `crates/bus` — added IPC message types (`IpcMessage`, `IpcPayload`, `LineStyle`), new `SystemEvent` variants

**Dependency graph changes:**
- CLI depends on `runtime` (for re-exported IPC utilities) and `bus` (for event types)
- CLI no longer constructs concrete actor instances → removed direct imports of `soul`, `memory`, `inference`, `platform`, etc.

**Boot sequence changes:**
- Daemon mode: `runtime::run_background()` → boot → readiness → BootComplete → supervision loop
- CLI mode (Phase 6+): `sena cli` → detect daemon → if running: connect IPC; if not: auto-start daemon → connect IPC
- CLI mode (pre-Phase 6, removed): `sena cli` → `runtime::boot_ready()` → all actors in-process

---

## Refactor Phases

### Phase 1: Supervision Loop and Process Lifetime Split
**Scope:** M-Refactor milestone

- [ ] Create `crates/runtime/src/supervisor.rs`
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

### Phase 3: CLI as IPC Client
**Scope:** M6.2 (CLI as Separate Process)

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

### Phase 4: Loop Registry and Real-Time Control
**Scope:** M6.2.1 (Loop Registry and Visibility)

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

### Phase 1: Supervision Loop (Completed)
**Date:** 2026-04-XX  
**Commit(s):** TBD

Summary: TBD

### Phase 2: IPC Protocol (Completed)
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
**Context:** Daemon-CLI split is being implemented in the nested workspace (`sena/sena`) on the dev branch. The refactor is isolated from the top-level workspace. Need to decide where to place the refactor tracking file.

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
