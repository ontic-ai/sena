---

# SENA ARCHITECTURAL GAP REPORT

**Date:** 2025-07-14  
**Scope:** Full workspace audit — `cargo check --workspace` + static code analysis  
**Code changes made:** ZERO

---

## 1. BUILD STATE

```
cargo check --workspace — CLEAN
Finished `dev` profile in 31.02s
Errors: 0   Warnings: 0
```

---

## 2. CONCRETE TYPE VIOLATIONS

Three actors hold concrete types where the architecture requires trait objects or where the concreteness creates extension risk:

| Actor | Field | Declared type | Classification |
|---|---|---|---|
| `crates/speech/src/stt_actor.rs:55` | `backend` | `SttBackend` (enum) | **VIOLATION — no trait, all backends wired directly into actor** |
| `crates/speech/src/stt_actor.rs:~90` | `backend_handle` | `Option<SttBackendHandle>` (enum) | **VIOLATION — extends the same concrete fan-out** |
| `crates/speech/src/tts_actor.rs:48` | `active_backend` | `Option<ActiveTtsBackend>` (enum) | ADVISORY — lower internal branching than STT |
| `crates/memory/src/actor.rs:57` | `store` | `Store<SenaEmbedder,SenaExtractor>` | ACCEPTABLE — ech0 ownership rule requires concrete generic |
| `crates/soul/src/actor.rs:~42` | `db` | `Option<EncryptedDb>` | ACCEPTABLE — sole-owner rule per architecture §9 |

**SttActor branching inventory** — the `SttBackend` enum violation manifests as at minimum 4 separate match sites in a single file:

- stt_actor.rs `initialize_backend()` — full per-backend init match
- stt_actor.rs — telemetry constants differ per backend (vram/sample rate)
- stt_actor.rs — `if self.backend == SttBackend::Parakeet` special-case flag
- stt_actor.rs — listen-mode routing via `(SttBackend, SttBackendHandle)` tuple match

Every new STT backend (e.g. StyleTTS2, any Phase 8 addition) requires modifying `SttActor` source. This is an open-closed violation against `docs/architecture.md §3`.

---

## 3. ORPHANED EVENTS (emitted — zero subscribers)

These events are broadcast on the bus and consumed by nothing. They consume broadcast slot allocations and mislead future developers reading the code.

| Event | Emitter | Subscriber count |
|---|---|---|
| `MemoryEvent::WriteCompleted` | actor.rs | **0** |
| `MemoryEvent::SemanticIngestComplete` | actor.rs | **0** |
| `SpeechEvent::SttTelemetryUpdate` | stt_actor.rs | **0** |
| `SpeechEvent::WakewordSuppressed` | wakeword.rs | **0** |
| `SpeechEvent::WakewordResumed` | wakeword.rs | **0** |
| `SoulEvent::RichSummaryReady` | actor.rs | **0** |
| `SoulEvent::IdentitySignalDistilled` | actor.rs | **0** |
| `SoulEvent::TemporalPatternDetected` | actor.rs | **0** |
| `InferenceEvent::InferenceRoundCompleted` | actor.rs | **0** |

**Total: 9 orphaned events.**

`SoulEvent::RichSummaryReady`, `IdentitySignalDistilled`, and `TemporalPatternDetected` are particularly high-value — these represent Soul's distilled intelligence outputs and are currently emitted into a void. The inference actor has a `// TODO Phase 7B: switch to RichSummaryRequested` comment at actor.rs confirming this is known but deferred.

---

## 4. DEAD SUBSCRIPTIONS (subscribed — never emitted in production)

These are event variants that an actor handles in its receive loop but which no production code ever broadcasts. The handler code is unreachable at runtime.

| Event | Handler location | Production emitter | Notes |
|---|---|---|---|
| `SoulEvent::RichSummaryRequested` | actor.rs | **NONE** | Only reference is the TODO comment in inference |
| `SoulEvent::PreferenceLearningUpdate` | actor.rs | **NONE** | No CLI command, no actor emits this |
| `SoulEvent::ExportRequested` | actor.rs | **NONE** | No `/export` CLI command exists |
| `SoulEvent::ExportCompleted` | defined in events/soul.rs | **NEVER** | Handler returns `ExportFailed` unconditionally — `ExportCompleted` is a stub variant |

**Total: 4 dead subscriptions.**

The `ExportRequested` handler at actor.rs always returns `SoulEvent::ExportFailed` and contains no implementation body. This is a compound dead subscription (unreachable input path) and orphaned event pair (`ExportCompleted` defined, never reachable).

---

## 5. IPC SERVER vs CLI GAPS

The CLI's `SLASH_COMMANDS` constant (used for autocomplete and help text) and the IPC server's `dispatch_slash_command()` are not in sync.

| Command | In `SLASH_COMMANDS` (shell.rs) | In `dispatch_slash_command()` (ipc_server.rs) |
|---|---|---|
| `/status` | **YES** | **MISSING** |

The `/status` command appears in the CLI's autocomplete list and help output. When a user types `/status`, it is sent to the daemon via `IpcPayload::SlashCommand`. The daemon falls through to the catch-all `_` branch in `dispatch_slash_command()`, which returns an "unknown command" error.

All other commands present in `SLASH_COMMANDS` (`/observation`, memory, `/explanation`, `/config`, `/reload`, `/actors`, `/models`, `/voice`, `/speech`, `/listen`, `/microphone`, `/screenshot`, `/verbose`, `/copy`, `/help`, `/close`, `/shutdown`, `/loops`, `/stt-backend`) have corresponding IPC server handlers. The gap is `/status` only.

---

## 6. INVISIBLE MEMORY QUERIES

Both query paths in the memory actor complete and broadcast results without logging the intermediate scores. This makes memory retrieval opaque — when CTP gets a weak context or a user gets a poor answer, there is no trace to show what chunks were considered, at what relevance, or why they were included or excluded.

| Function | File | Gap |
|---|---|---|
| `handle_query()` | actor.rs | Two-level merge+sort by score, result count, and per-chunk relevance_score never emitted to `tracing::debug!` |
| `handle_context_query()` | actor.rs | Mean relevance score computed but not logged before `ContextQueryCompleted` broadcast |

The only `tracing` calls in the memory actor are `tracing::warn!` on error paths (actor.rs, actor.rs). The happy path is completely silent.

**Recommended addition** (not a fix in this report — audit only):
```rust
tracing::debug!(
    chunks = result.chunks.len(),
    top_score = result.chunks.first().map(|c| c.relevance_score),
    query = %query_text,
    "memory query completed"
);
```

---

## 7. MISSING ABSTRACTION: STT BACKEND TRAIT

There is no `SttBackend` trait. The Whisper, Sherpa, and Parakeet backends are wired directly into `SttActor` via enum matching. This is the single largest structural gap relative to the architecture's trait-object pattern (which `InferenceActor` and `PlatformActor` both follow correctly).

**Current structure:**
```
SttActor { backend: SttBackend }
                    ^
                    |
        match { Whisper => ..., Sherpa => ..., Parakeet => ... }  ← 4 sites
```

**Architecture-correct structure** (as in actor.rs):
```
SttActor { backend: Box<dyn SttBackend> }
                    ^
                    |
              trait methods (.initialize(), .transcribe(), .listen_mode())
              implemented by WhisperBackend, SherpaBackend, ParakeetBackend
```

This is not a compile error. It is an architectural debt item that blocks clean addition of any Phase 8 backend without forking `SttActor`.

---

## 8. IPC TRANSPORT SILENT STUBS

**None found.**

Both transport paths are fully implemented:

- **Windows:** runtime/src/ipc_server.rs `start_windows_on()` uses `tokio::net::windows::named_pipe::ServerOptions::new().first_pipe_instance(false).create()` with real read/write loops
- **Unix:** `start_unix_on()` uses `StdUnixListener::bind()` with real accept loops
- **Client (Windows):** cli/src/ipc_client.rs connects via `tokio::net::windows::named_pipe::ClientOptions::new().open()`
- **Client (Unix):** connects via `tokio::net::UnixStream::connect()`

No silent no-ops, no `todo!()` stubs, no `unimplemented!()` in the IPC transport layer.

---

## 9. CAUSAL TRACING

**Absent across the entire workspace.**

Searched for: `CausalId`, `correlation_id`, `causal_id`, `request_chain`, `trace_id`  
Result: **Zero matches.**

There is no mechanism to trace a request chain from its trigger (user input, CTP signal, scheduled event) through bus hops to its terminal effect (inference response, soul write, memory ingest). When a request fails silently mid-chain or produces an unexpected output, there is no way to reconstruct which actor handled what, in which order, with what inputs.

This is a diagnosability gap. It does not affect correctness today, but becomes blocking when asynchronous failures need root-cause analysis across actor boundaries (e.g., "why did CTP not trigger inference for this context snapshot?").

---

## PRIORITY RANKING

| Priority | Finding | Impact | Phase relevance |
|---|---|---|---|
| **P1** | Missing STT backend trait (`stt_actor`) | Blocks clean Phase 8 backend addition; open-closed violation | M7.2 / Phase 8 |
| **P2** | 9 orphaned events | Soul intelligence outputs (RichSummaryReady, IdentitySignalDistilled) are computed and discarded; CTP gets no Soul signal | M7.x and ongoing |
| **P3** | 4 dead subscriptions in Soul actor | ExportRequested handler is dead code; PreferenceLearningUpdate has no emitter | Phase 6/7B completion |
| **P4** | Invisible memory queries | Memory scoring opaque to all observability tooling | Any debugging session |
| **P5** | No causal tracing | Multi-hop failures undiagnosable | Phase 8+ diagnosability |
| **P6** | `/status` IPC gap | User-visible: command in autocomplete returns "unknown command" error | M7.x / CLI polish |

---

**END OF AUDIT REPORT**