# Sena — Architecture
**Version:** 0.6.0  
**Status:** Governing Document — no implementation decision may contradict this file without a formal revision  
**Reconcile against:** `docs/PRD.md` first, this document second

---

## 0. How to Read This Document

This document is **prescriptive, not descriptive.** It does not describe what was built. It describes what must be built and why. Every section answers three questions:

1. What is this?
2. What are the hard rules?
3. What does it connect to?

If your code contradicts a hard rule, the code is wrong — not the rule.

---

## 1. Workspace Layout

```
sena/
├── Cargo.toml              ← virtual manifest ONLY. No src/ at root. Ever.
├── Cargo.lock
├── rust-toolchain.toml     ← pins Rust edition and toolchain version
├── .cargo/
│   └── config.toml         ← workspace-level Cargo config (target dir, features)
│
├── crates/
│   ├── bus/                ← event bus, actor trait, all typed events
│   ├── crypto/             ← encryption primitives, key management, file encryption
│   ├── daemon/             ← daemon binary — boots runtime, hosts IPC server, runs tray
│   ├── ipc/                ← protocol layer for daemon-CLI communication
│   ├── runtime/            ← boot sequence, actor registry, shutdown
│   ├── platform/           ← OS adapter trait + per-OS implementations
│   ├── ctp/                ← continuous thought processing loop
│   ├── inference/          ← llama-cpp-rs wrapper, model manager, queue
│   ├── memory/             ← tiered memory, dual-routing, consolidation
│   ├── prompt/             ← dynamic prompt composition engine
│   ├── text/               ← sentence boundary detection and text utilities
│   ├── soul/               ← SoulBox: identity, schema, event log
│   ├── speech/             ← STT, TTS, wakeword — primary interaction surface
│   └── cli/                ← CLI binary — IPC client, TUI renderer
│
├── xtask/                  ← cargo xtask automation (Rust, not shell scripts)
├── docs/
│   ├── PRD.md
│   ├── architecture.md     ← this file
│   └── subsystems/         ← one .md per crate with deep-dive spec
├── tests/                  ← workspace-level integration tests only
└── examples/               ← isolated runnable examples per subsystem
```

**Hard rules:**
- The root `Cargo.toml` is a virtual manifest. It has no `[package]` section and no `src/`.
- Every crate under `crates/` is named without a `sena-` prefix. Names are functional: `bus`, `runtime`, `soul`.
- Binary crates: `daemon` (produces `sena` binary) and `cli` (produces `sena-cli` binary). All other crates are `lib`.
- Crates are published to crates.io only if explicitly decided. Default: workspace-internal.
- No `Makefile`. No shell scripts in root. All automation lives in `xtask/`.

---

## 2. Dependency Graph

This is the law. Arrows mean "may depend on." Absence of an arrow means the dependency is **forbidden.**

```
daemon
 ├── runtime      ← boots runtime, owns supervision loop
 ├── ipc          ← hosts IPC server
 └── bus          ← event subscriptions for shutdown and tray menu

cli
 └── ipc          ← IPC client only — all work dispatched to daemon

ipc               ← protocol layer: no Sena crate dependencies, external-only leaf

runtime           ← composition root: constructs ALL concrete actor instances
 ├── bus
 ├── crypto
 ├── soul
 ├── platform
 ├── ctp
 ├── memory       ← runtime constructs MemoryActor
 ├── inference    ← runtime constructs InferenceActor
 └── speech

ctp
 ├── bus
 └── platform

crypto
 (no Sena crate dependencies — leaf node)

inference
 ├── bus
 └── text

memory
 ├── bus
 ├── crypto
 └── soul

prompt
 └── bus

soul
 ├── bus
 └── crypto

text
 (no Sena crate dependencies — leaf node — sentence boundary detection)

platform
 └── bus

speech
 └── bus
```

**Hard rules:**
- `runtime` is the composition root. It constructs all concrete actor instances (soul, platform, ctp, memory, inference, speech) inside `boot()`. The daemon binary calls `runtime::boot()`. CLI never constructs actors.
- `daemon` imports `runtime`, `ipc`, and `bus`. It is the process owner for Sena's background operation.
- `cli` imports only `ipc`. It is a thin IPC client with no business logic. All work is dispatched to the daemon via IPC commands.
- `ipc` is a protocol-only crate with zero Sena crate dependencies. It is an external-only leaf node, depending only on `tokio`, `serde`, `thiserror`, and `async-trait`.
- `crypto` has zero dependencies on any other Sena crate. Like `bus` and `ipc`, it is a leaf node in the graph. It provides encryption primitives consumed by `runtime`, `soul`, and `memory`.
- `soul` has no knowledge of `ctp`, `inference`, `memory`, or `prompt`. It only knows `bus` and `crypto`. Other crates emit events; soul absorbs them. Soul's internals are never reached into from outside.
- `speech` depends only on `bus`. It receives events and emits events. It never imports `inference`, `memory`, or `soul`.
- `bus` has zero dependencies on any other Sena crate. It is the bottom of the graph.
- `cli` is the developer-facing tool surface. It has no business logic, no actor construction, and no runtime import. All functionality is accessed via IPC. If business logic appears in `cli`, it belongs in an actor in the daemon.
- Circular dependencies are a build error. The graph above must remain a DAG.
- `platform` never imports OS-specific crates at the crate root level. Platform-specific code is gated behind `#[cfg(target_os = ...)]` within the crate.

---

## 3. The Bus

The bus is the central nervous system. Every subsystem communicates through it. No subsystem calls another subsystem's functions directly.

### 3.1 Architecture

Two channel types coexist:

| Type | Crate | Use Case |
|---|---|---|
| `broadcast` | `tokio::sync::broadcast` | One-to-many. System-wide events all actors may care about (e.g., `ShutdownSignal`, `ContextSnapshotReady`). |
| `mpsc` | `tokio::sync::mpsc` | One actor to one actor. Directed work (e.g., `InferenceRequest` to the inference actor). |

The `EventBus` struct owns the broadcast sender and a registry of named mpsc senders for directed routing.

### 3.2 Typed Events

All events are defined in `crates/bus/src/events.rs`. **This is the single source of truth for every message that flows through the system.**

Events are organized into modules by domain:

```rust
pub mod system { ... }     // ShutdownSignal, BootComplete, ActorFailed
pub mod platform { ... }   // WindowChanged, ClipboardChanged, FileEvent, KeystrokePattern
pub mod ctp { ... }        // ContextSnapshotReady, ThoughtEventTriggered
pub mod inference { ... }  // InferenceRequested, InferenceCompleted, InferenceSource, streaming events
pub mod memory { ... }     // MemoryWriteRequest, MemoryQueryRequest, MemoryQueryResponse
pub mod soul { ... }       // SoulEventLogged, IdentitySignalEmitted
```

`InferenceSource` (in `bus::events::inference`) replaces the `request_id < 1000` detection convention for proactive requests. All inference routing decisions must use `InferenceSource` variants (`UserVoice`, `UserText`, `ProactiveCTP`, `Iterative`) rather than inferring intent from numeric IDs.

**Hard rules:**
- No string-typed events. Ever. Every event is a typed Rust struct or enum.
- Events are `Clone + Send + 'static`. No exceptions.
- Events carry no logic. They are pure data. Methods on event types are forbidden.
- New events are added to `events.rs` first, before any code that emits or subscribes to them is written.
- Events are immutable once sent. No actor modifies a received event.

### 3.3 Actor Trait

Every subsystem implements the `Actor` trait defined in `crates/bus/src/actor.rs`:

```rust
#[async_trait]
pub trait Actor: Send + 'static {
    fn name(&self) -> &'static str;
    async fn start(&mut self, bus: Arc<EventBus>) -> Result<(), ActorError>;
    async fn run(&mut self) -> Result<(), ActorError>;
    async fn stop(&mut self) -> Result<(), ActorError>;
}
```

**Hard rules:**
- Every actor owns its own state. No shared mutable state between actors.
- Actors communicate exclusively via the bus. Direct function calls between actors are forbidden.
- An actor that panics must not bring down other actors. The runtime catches panics and emits `ActorFailed` onto the bus.
- Actors do not block the async executor. All blocking work (file I/O, model inference) runs in `tokio::task::spawn_blocking`.

---

## 4. Runtime

The runtime lives in `crates/runtime`. It owns the boot sequence, the actor registry, the readiness gate, the supervision loop, and the shutdown protocol. It is the process lifetime owner for daemon mode.

### 4.1 Boot Sequence

Order is strict and non-negotiable:

```
1. Config load           — read user config from disk (or create defaults)
2. Encryption init       — derive or retrieve master key; must complete before any store opens
3. EventBus init         — bus is live before any actor starts
4. Soul init             — SoulBox schema loaded/migrated before anything writes to it
5. Core actors spawn     — Soul, Platform, CTP, Memory, Inference (each pushed to expected_actors)
6. Platform adapter      — OS signal collection begins
7. CTP actor             — begins observation loop
8. Memory actor          — loads indexes, prepares write queues
9. Inference actor       — discovers models, does NOT load weights yet
10. Prompt actor         — ready to compose, idle until ThoughtEvent
11. Speech actors        — STT, TTS spawned conditionally if speech_enabled (pushed to expected_actors)
12. System tray          — tray icon created in dedicated thread
13. (no BootComplete here — emitted by supervisor after readiness gate)
```

If any step from 1–4 fails, Sena exits with a clear error. Steps 5–11 failing emit `ActorFailed`.

### 4.2 Readiness Gate

After `boot::boot()` returns, the supervisor (`crates/runtime/src/supervisor.rs`) waits up to 30 seconds for every actor in `runtime.expected_actors` to emit `SystemEvent::ActorReady`. Only then is `BootComplete` broadcast.

This ensures `BootComplete` listeners (like CTP and inference) only activate once all lower-level actors are confirmed up.

### 4.3 Process Architecture (Two Binaries)

**As of Phase 6, Sena operates as two separate binaries:**

| Binary | Artifact | Entry Point | Lifetime Owner |
|---|---|---|---|
| `sena` | `crates/daemon/` | `daemon/src/main.rs` | `runtime::boot()` → supervision loop → tray loop |
| `sena-cli` | `crates/cli/` | `cli/src/main.rs` | IPC connection → shell → TUI → disconnect |

**Daemon binary (`sena`):**
- Always boots as background process — no subcommand parsing
- Calls `runtime::boot()` to construct all actors
- Hosts IPC server on named pipe (`\\.\pipe\sena-daemon` on Windows, equivalent on macOS/Linux)
- Registers all command handlers via `CommandRegistry`
- Runs supervision loop in background task
- Runs tray loop on main thread (blocking)
- Handles `ShutdownSignal` gracefully, flushes Soul, stops all actors

**CLI binary (`sena-cli`):**
- **CLI design principle — wrapper, not owner:** All business logic (inference, memory, STT, CTP, Soul writes) lives in the daemon and its actors. The CLI dispatches IPC commands to request work and renders the resulting event stream. It has no actors of its own and never boots the runtime.
- Checks if daemon is running via IPC health check
- If daemon not running: auto-starts daemon binary (`sena.exe` or `sena`) in background, waits for IPC readiness (30s timeout)
- Connects to daemon via `IpcClient::connect()`
- All slash commands and user actions dispatch IPC commands to daemon
- Renders responses from IPC event stream in TUI
- On exit, disconnects from daemon — daemon continues running

**IPC connection flow:**
1. CLI starts, calls `ensure_daemon_running()`
2. If daemon not detected, CLI spawns daemon binary as detached child process
3. CLI polls for IPC server availability (retry loop, 30s timeout)
4. CLI connects via `IpcClient` to named pipe
5. CLI sends commands, daemon responds with structured events
6. CLI disconnects on exit; daemon remains alive

**Open CLI from tray:** The "Open CLI" tray menu item spawns a new terminal process running `sena-cli`. This keeps the daemon and CLI as independent processes.

### 4.4 Shutdown Protocol

Shutdown is always graceful. Signal: OS SIGINT/SIGTERM or `ShutdownSignal` event on bus.

```
1. ShutdownSignal emitted on broadcast channel
2. All actors receive signal via their broadcast subscription
3. Each actor calls its own stop() — flushes buffers, closes handles
4. Runtime waits for all actors to confirm stop (with timeout)
5. Soul flushes any pending event log writes
6. Process exits cleanly
```

**Hard rules:**
- There is no `process::exit()` call outside of the runtime's shutdown handler.
- Shutdown timeout is configurable. Default: 5 seconds per actor before force-kill.
- Soul always flushes before exit. Data loss in Soul is treated as a critical failure.

### 4.5 Actor Registry

The registry maps actor names to their `JoinHandle`. The runtime uses this to monitor liveness. The supervisor restarts failed actors up to `MAX_ACTOR_RETRIES` (3) times before triggering shutdown.

---

## 5. Platform Adapter

Lives in `crates/platform`. Provides OS signal collection behind a single trait.

### 5.1 The Trait

```rust
pub trait PlatformAdapter: Send + 'static {
    fn active_window(&self) -> Option<WindowContext>;
    fn subscribe_clipboard(&self, tx: mpsc::Sender<ClipboardEvent>);
    fn subscribe_file_events(&self, path: PathBuf, tx: mpsc::Sender<FileEvent>);
    fn subscribe_keystroke_patterns(&self, tx: mpsc::Sender<KeystrokePattern>);
}
```

### 5.2 Per-OS Implementations

| Signal | macOS | Windows | Linux |
|---|---|---|---|
| Active window | `core-graphics` / Accessibility API | `winapi` GetForegroundWindow | `x11rb` / `atspi` |
| Clipboard | `arboard` | `arboard` | `arboard` |
| File events | `kqueue` via `notify` crate | `ReadDirectoryChangesW` via `notify` crate | `inotify` via `notify` crate |
| Keystroke pattern | `rdev` crate (timing only, no char capture) | `rdev` crate | `rdev` crate |

**Hard rules:**
- `PlatformAdapter` is the only place OS-specific code lives. No OS-specific imports anywhere else in the codebase.
- Keystroke observation captures **timing and cadence only**. Characters are never captured, never stored, never transmitted on the bus. This is enforced at the type level: `KeystrokePattern` contains no character fields.
- Clipboard content is passed as `Option<String>` to CTP. CTP digests it into a signal — raw clipboard text is never written to episodic or semantic memory verbatim.
- File event observation scope is configurable. Default: user's home directory, excluding system paths.

---

## 6. Continuous Thought Processing (CTP)

Lives in `crates/ctp`. This is Sena's observation loop.

### 6.1 Pipeline

```
Platform Events → Signal Buffer → Context Assembler → Trigger Gate → ThoughtEvent
```

- **Signal Buffer:** A rolling time-window accumulator. Collects raw platform events for the last N seconds (configurable).
- **Context Assembler:** Transforms the signal buffer into a `ContextSnapshot`. This is typed, structured data — not a string.
- **Trigger Gate:** Decides whether the current snapshot is interesting enough to trigger inference. Triggers on: significant app/task switch, burst of activity after idle, scheduled reflection interval, explicit user summon.

### 6.2 ContextSnapshot

```rust
pub struct ContextSnapshot {
    pub active_app: WindowContext,
    pub recent_files: Vec<FileEvent>,
    pub clipboard_digest: Option<String>,   // digest/summary, not raw content
    pub keystroke_cadence: KeystrokeCadence,
    pub session_duration: Duration,
    pub inferred_task: Option<EnrichedInferredTask>,  // was Option<TaskHint>
    pub user_state: Option<UserState>,                 // NEW
    pub visual_context: Option<VisualContext>,          // NEW (from Phase 5.5)
    pub timestamp: Instant,
}
```

**Hard rules:**
- `ContextSnapshot` contains no raw keystroke characters. Build fails if a char/String field is added to `KeystrokeCadence`.
- CTP never calls the inference layer directly. It emits `ThoughtEventTriggered` on the bus.
- The trigger gate must be tunable without code changes. Thresholds live in config.

### 6.3 Signal Completeness

**If Sena observes it, CTP must eventually know about it.** This is a general architectural principle — the list below is the *current* implementation status, not a ceiling.

| Signal | Status |
|---|---|
| Active window, clipboard, file events, keystroke cadence | Done |
| Screen captures / visual context | Done |
| Speech transcriptions (STT output) | Planned — see `docs/SUBSYSTEM_AUDIT.md` F3a |

Any new sensor or observation capability added in future phases must include a CTP signal ingestion path as part of its design. A signal that bypasses CTP's buffer is a context gap. Context gaps make CTP's trigger decisions less intelligent.

CTP is the product's core differentiator — it must not be consistently deprioritised in favor of surface-level features.

### 6.4 Actor Coordination During Audio Capture

When multiple speech actors require microphone access (wakeword detection, STT always-listening, listen mode), audio capture lifecycle must be coordinated:

- Only one purpose should hold an active audio stream to a given device at any time.
- When listen mode activates, wakeword detection must be suppressed (via `WakewordSuppressed` event) and its audio stream released.
- When listen mode deactivates, wakeword can resume (via `WakewordResumed` event) and reclaim its stream.
- Shared state (accumulated samples, VAD state) must be scoped per-mode, never shared between concurrent capture purposes.

### 6.5 CTP Intelligence Layer

CTP's intelligence layer analyzes signal buffers and context snapshots to detect behavioral patterns, infer user state, and semantically describe tasks. This layer enables significance-based triggering that considers user context, not just raw context diffs.

**Pattern Engine** (`crates/ctp/src/pattern_engine.rs`):

Detects behavioral patterns from the signal buffer using rule-based detection (no ML). Each pattern has named detection rules with configurable thresholds:

| Pattern | Detection criteria |
|---|---|
| **Frustration** | Rapid window switches (>3 in 30s), high keystroke variance (burst then pause), clipboard copy without paste (abandoned) |
| **Repetition** | Same file edited >3 times in 5 minutes, same app switch back-and-forth |
| **FlowState** | Sustained keystroke cadence in narrow variance band, no app switches for >10 minutes, low idle periods |
| **Anomaly** | Out-of-hours activity, unusual app combination, rapid task switching (>5 context switches in 2 minutes) |

All thresholds are currently compile-time constants, with config-driven tuning planned for a future phase.

**User State Classifier** (`crates/ctp/src/user_state.rs`):

Computes ephemeral user state from the current snapshot + detected patterns. Output fields:
- `frustration_level`: 0–100 integer score
- `flow_detected`: boolean
- `context_switch_cost`: 0–100 integer score representing cognitive load from recent switches

User state is **never persisted** — it is computed per CTP tick and discarded. It influences trigger scoring and is included in the `ContextSnapshot.user_state` field for prompt composition.

**Task Inference Engine** (`crates/ctp/src/task_inference.rs`):

Generates semantic task descriptions from active window context, replacing the old simple app-name matching. Produces `EnrichedInferredTask` with:
- `semantic_description`: String (e.g., "Editing Rust source code", "Debugging a build failure", "Writing documentation")
- `confidence`: f64 (0.0–1.0)

Task descriptions are rule-based inferences from window title + app name patterns. No LLM inference is used for task inference.

**Significance-based triggering** (`trigger_gate.rs` enhancement):

The trigger gate now considers:
1. Context diff magnitude (unchanged from Phase 1–6)
2. Detected patterns (frustration/repetition increase score, flow state increases threshold)
3. Memory relevance (if context-aware memory query returns high relevance score, increase trigger likelihood)
4. User state (high frustration or context switch cost boosts proactive trigger probability)

Trigger scoring is configurable. Default weights:
- Context diff: 40%
- Pattern detection: 30%
- Memory relevance: 20%
- User state: 10%

**Hard rules:**
- All pattern detection is rule-based. Thresholds must be tunable via config.
- User state is ephemeral — computed per CTP tick, not persisted to Soul or memory.
- `EnrichedInferredTask` replaces the old `TaskHint` type in `ContextSnapshot`.
- Task inference is synchronous and fast (<5ms). No blocking I/O or inference calls.
- Pattern engine output is emitted as `PatternDetected` events on the bus for transparency.

---

## 7. Inference

Lives in `crates/inference`. Adapts the external `ontic/infer` crate (https://github.com/ontic-ai/infer, tag v0.1.0) to Sena's bus architecture. The `infer` crate provides the `InferenceBackend` trait, `LlamaBackend`, `MockBackend`, model discovery, and streaming via `std::sync::mpsc::Receiver<String>`. Sena's inference crate re-exports `infer`'s types for backward compatibility.

### 7.1 Model Discovery

On startup, the inference actor scans for Ollama's model manifest at the platform-specific path:

| OS | Ollama model path |
|---|---|
| macOS | `~/.ollama/models/` |
| Windows | `%USERPROFILE%\.ollama\models\` |
| Linux | `~/.ollama/models/` |

It builds a `ModelRegistry` of available GGUFs. **Ollama's inference server is never started.**

### 7.2 Backend Selection

Auto-detected at runtime in priority order:

```
1. Metal (macOS, Apple Silicon)
2. CUDA (Windows/Linux, NVIDIA GPU)
3. CPU (fallback, all platforms)
```

### 7.3 Inference Queue

The inference actor processes one `InferenceRequest` at a time from an mpsc channel. Requests carry a priority level. The queue is bounded.

**Hard rules:**
- Model weights are not loaded at boot. They are loaded on first `InferenceRequest` with a warm-up signal emitted on the bus.
- Inference runs in `tokio::task::spawn_blocking`. It never blocks the async runtime.
- The inference actor never reads from memory or soul directly. It receives a fully-composed prompt string from the prompt actor.
- Inference responses are emitted as `InferenceResponse` events on the bus.

---

## 8. Memory

Lives in `crates/memory`. This crate is an **adapter**, not an implementation. All memory logic — graph traversal, vector search, A-MEM linking, contradiction detection, importance decay, provenance — is owned by `ech0`. Sena's `memory` crate translates Sena's typed domain into ech0's API and translates ech0's results back into Sena's typed domain.

### 8.1 Dependency

ech0 is fetched as a Git dependency:

```toml
[dependencies]
ech0 = { git = "https://github.com/kura120/ech0", tag = "v0.1.0", features = ["full"] }
```

ech0 is never forked or vendored unless a breaking upstream change requires it. All ech0 feature flags are enabled via `"full"`.

### 8.2 Memory Tiers (Conceptual → ech0 Mapping)

| Sena concept | ech0 layer | Storage |
|---|---|---|
| **Working memory** | Not owned by ech0. In-RAM, scoped to inference cycle. | `Vec<MemoryChunk>` in the memory actor's state |
| **Episodic memory** | ech0 graph nodes (`Node`) + edges (`Edge`) | `redb` embedded database on disk |
| **Semantic memory** | ech0 vector index | `hora` index file on disk |

### 8.3 ech0 Trait Implementations

ech0 requires the caller to implement two traits. **These are implemented in `crates/memory`**, not in `crates/inference`. The `memory` crate depends on `inference` to perform the actual llama-cpp-rs calls, which means OQ-6 is resolved as follows: `memory` depends on `inference` for embedding and extraction execution only — `inference` exposes a narrow embedding API on its bus channel, `memory` calls it via a directed mpsc channel. The dependency graph arrow `memory → inference` is therefore valid per architecture §2.

```
Embedder impl (memory crate):
  - Receives raw text
  - Sends EmbedRequest on directed channel to inference actor
  - Awaits EmbedResponse with f32 vector
  - Returns vector to ech0

Extractor impl (memory crate):
  - Receives raw text
  - Sends ExtractionRequest on directed channel to inference actor
  - Awaits ExtractionResult (structured facts/entities)
  - Returns ExtractionResult to ech0
```

**Hard rules:**
- `memory` never calls llama-cpp-rs directly. It goes through `inference` via the bus.
- `inference` never calls ech0 directly. It only responds to embed/extract requests.
- The `Embedder` and `Extractor` implementations live exclusively in `crates/memory`.

### 8.4 Ingest Path

```
ThoughtEvent resolved → inference cycle ends
  → MemoryConsolidator (inside memory actor) receives InferenceResponse
  → Calls store.ingest_text(response_text).await
  → ech0 handles: node creation, embedding, A-MEM linking, contradiction check
  → If ConflictReport returned: emitted as MemoryConflictDetected event on bus
  → IngestResult logged to Soul event log via SoulWriteRequest on bus
```

### 8.5 Retrieval Path (Dual-Routing via ech0)

```
MemoryQueryRequest arrives (from prompt actor)
  → Level 1: store.search(query, SearchOptions { tier: Graph, .. }).await
      → ech0 graph traversal: coarse topic/entity matching
      → Returns ScoredNode list
  → Level 2: store.search(query, SearchOptions { tier: Vector, .. }).await
      → ech0 ANN vector search within relevant clusters
      → Returns ScoredNode list ranked by importance-decayed score
  → Merge results, deduplicate, apply token budget cap
  → Return Vec<MemoryChunk> to prompt actor
```

### 8.6 Contradiction Handling

ech0 returns a `ConflictReport` when a new ingest contradicts an existing node. Sena's memory actor:
1. Emits `MemoryConflictDetected(ConflictReport)` on the bus
2. Soul logs the conflict
3. CTP may incorporate the conflict as a `ContextSnapshot` signal on the next cycle
4. **Silent overwrites never happen.** This is enforced by ech0's design and Sena never calls `ConflictResolution::Overwrite` without a logged, reasoned decision.

### 8.7 Encryption at Rest

Both ech0 storage files (`redb` graph, `hora` vector index) are encrypted. See §15 for the full encryption architecture. The `memory` actor is responsible for providing ech0's `StorePathConfig` with paths that point to encrypted file handles — ech0 itself has no knowledge of encryption.

**Hard rules:**
- Raw clipboard text is never passed to `store.ingest_text()`.
- Raw keystroke data is never passed to `store.ingest_text()`.
- Working memory (`Vec<MemoryChunk>`) is never written to ech0 or disk. It is ephemeral.
- `ConflictResolution::Overwrite` is never called silently. Any overwrite is logged to Soul first.
- The ech0 `Store` instance is owned exclusively by the memory actor. No other actor holds a reference to it.

### 8.8 Context-Aware Memory Queries

CTP can request context-relevant memories via `ContextMemoryQueryRequested` events (defined in `bus::events::memory`). The memory actor performs graph-heavy search (65% graph weight vs. 35% vector weight) with lower importance thresholds (0.10) for broad context retrieval.

The response (`ContextMemoryQueryResponse`) includes:
- `chunks`: `Vec<MemoryChunk>` — retrieved memories ranked by relevance
- `overall_relevance`: f64 (0.0–1.0) — aggregate relevance score for CTP trigger gating

Context queries differ from standard memory queries (which use 50/50 graph/vector weight and 0.30 importance threshold). Context queries prioritize breadth and recency over strict semantic match, enabling CTP to detect when the user's current activity relates to past experiences even if the phrasing differs.

**Hard rule:** Context queries never trigger memory consolidation or ingestion. They are read-only.

---

## 9. Prompt Composition

Lives in `crates/prompt`. **There are no static prompt strings anywhere in the Sena codebase.**

### 9.1 Design

Prompts are assembled at runtime from a tree of `PromptSegment` nodes:

```rust
pub enum PromptSegment {
    SystemPersona(PersonaState),
    MemoryContext(Vec<MemoryChunk>),
    CurrentContext(ContextSnapshot),
    UserIntent(Option<String>),
    ReflectionDirective(ReflectionMode),
    SoulContext(SoulSummary),
    RichSoulContext(RichSoulSummary),  // NEW — multi-section, relevance-scored soul summary
}
```

The composer:
1. Receives a `ThoughtEvent` with associated context
2. Queries memory for relevant chunks
3. Queries soul for current persona state
4. Selects which segments to include based on event type and token budget
5. Assembles segments in order
6. Emits `InferenceRequest` with the composed prompt

**Hard rules:**
- No hardcoded strings in prompt assembly. Every text fragment comes from config, soul, or memory.
- Token budget is always respected. The composer uses llama-cpp-rs's tokenizer to count before emitting.
- Prompt assembly logic has no side effects. It is a pure transformation: inputs → prompt string.

---

## 10. SoulBox

Lives in `crates/soul`. This is Sena's identity and personalization engine.

### 10.1 Principles

- Soul starts empty on first boot. It is never pre-seeded with defaults that imply knowledge of the user.
- Soul evolves only through observation and interaction. Nothing is hardcoded into a user's soul profile.
- Soul's schema is versioned. Every schema change requires a migration. Breaking migrations are forbidden — they must be additive or transformative with data preservation.

### 10.2 Storage

Soul uses `redb` — the same embedded pure-Rust transactional database used by ech0. This is intentional: one database engine in the dependency tree, not two. Soul's redb database is separate from ech0's redb database. Schema lives in `crates/soul/src/schema/`. Migrations are numbered sequentially and run automatically on boot (step 3 of boot sequence). The redb file is encrypted at rest (see §15).

### 10.3 Event Log

Every significant event (inference cycle, memory write, identity signal, user interaction) is written to Soul's append-only event log. This log is the ground truth of Sena's history with this user.

**Hard rules:**
- No other crate writes to Soul's redb database directly. Soul exposes an mpsc channel for write requests and emits read responses via bus events.
- Soul's event log is append-only. No record is ever deleted by the system. Deletion is a user-initiated, explicit, destructive action with confirmation.
- Soul's internal schema types are never exposed in `pub` APIs outside the crate. External crates receive `SoulSummary` and `SoulEventLogged` — opaque, typed summaries.
- Soul does not perform inference. It stores and retrieves. Any "understanding" of soul data is done by CTP or the prompt actor.

### 10.4 Soul Intelligence Layer

Soul's intelligence layer processes absorbed events to identify persistent behavioral patterns, temporal habits, and user preferences. All modules are private (`mod`, not `pub mod`) — only events are public.

**Distillation Engine** (`crates/soul/src/distillation.rs`):

Watches identity signal counters (stored in the `IDENTITY_SIGNALS` table as key-value pairs). When a signal crosses significance thresholds (configurable, e.g., >5 occurrences in 7 days), it distills the pattern into a `DistilledIdentitySignal`. Distilled signals are persisted and included in `RichSoulSummary`.

Example signals:
- Preferred programming languages (file extension frequency)
- Active project contexts (frequent directory patterns)
- Communication style (preferred tone from feedback)

Thresholds are currently compile-time constants (min occurrences: 5, window: 7 days), configurable in a future phase.

**Temporal Model** (`crates/soul/src/temporal_model.rs`):

Records events bucketed by hour-of-day (0–23) and day-of-week (Mon–Sun). Produces `TemporalBehaviorPattern` entries:
- Peak activity hours
- Typical work hours vs. off-hours
- Weekend vs. weekday behavior differences

Temporal patterns are computed from the event log (`EVENT_LOG` table) during the harvest cycle. They enable Sena to adapt proactive behavior to user rhythms (e.g., suppress proactive thoughts during detected focus hours).

**Preference Learner** (`crates/soul/src/preference_learning.rs`):

Tracks user engagement signals from bus events:
- `InferenceAccepted` / `InferenceIgnored` / `InferenceInterrupted`
- `FollowUpQuery` (user asked clarifying question — sign of interest)

After sufficient data (>20 inference cycles with explicit feedback), distills preferences:
- `verbosity` (short vs. detailed responses)
- `proactiveness` (how often proactive thoughts are accepted vs. ignored)
- `tone` (formal, casual, technical)

Preferences are stored as key-value pairs in `IDENTITY_SIGNALS` and included in `RichSoulSummary`.

**Rich Summary Assembler** (`crates/soul/src/summary_assembler.rs`):

Produces `RichSoulSummary` (emitted via `SoulRichSummaryReady` event) with multiple sections:
1. **RecentEvents** (last 10 events from event log)
2. **IdentitySignals** (distilled patterns, sorted by relevance score)
3. **TemporalHabits** (current-hour and current-day patterns)
4. **Preferences** (learned interaction preferences)

Each section is relevance-scored based on:
- Recency (events in last 24h score higher)
- Frequency (signals with >10 occurrences score higher)
- Context match (if current task matches a distilled signal, boost its score)

The prompt actor can selectively include high-relevance sections based on token budget.

**Harvest cycle:**

Every 50 absorbed events (compile-time constant, configurable in a future phase), the soul actor triggers:
1. Distillation: check identity signal counters, distill new patterns
2. Temporal update: bucket recent events by time, update temporal model
3. Preference check: if >20 feedback events exist, recompute preferences

All harvested data is persisted to the `IDENTITY_SIGNALS` table as JSON-serialized key-value pairs.

**Hard rules:**
- All intelligence modules are `mod` (private to soul crate). Only events are public.
- All data persisted through existing `IDENTITY_SIGNALS` table (key-value pairs). No new schema tables for intelligence data.
- Harvest cycle is triggered by event count, not time interval (deterministic, testable).
- `RichSoulSummary` sections are sorted by relevance. The prompt actor decides which to include.
- No LLM inference is performed by Soul. All distillation and preference learning is rule-based heuristics.

---

## 11. Error Handling Philosophy

- **No `unwrap()` or `expect()` outside of tests.** Every production `unwrap()` is a latent crash.
- Errors are typed. Every crate defines its own `Error` enum. `anyhow` is permitted in `cli` only.
- Actor failures emit `ActorFailed { actor_name, error }` on the bus. The runtime handles restart policy.
- User-facing errors are human-readable. Internal errors are structured for logging.
- Sena never panics in production. If a panic is unavoidable, it is isolated to a `spawn_blocking` task and caught by the runtime.

---

## 12. Configuration

- Config lives in a platform-appropriate location (e.g. `~/.config/sena/` on Linux, `~/Library/Application Support/sena/` on macOS).
- Config format: TOML.
- All tunable thresholds (CTP trigger intervals, token budgets, memory window sizes, shutdown timeouts) are in config.
- Speech-related config (`speech_enabled`, `voice_always_listening`, `whisper_model_path`, `stt_energy_threshold`) controls the STT/TTS subsystem. Speech is enabled by default and enabled through onboarding or config.
- Config is loaded once at boot and treated as immutable for the session. Hot-reload is a future concern.
- No hardcoded values for tunables anywhere in the codebase. If it could reasonably change between deployments, it is in config.

---

## 13. Testing Strategy

| Layer | Approach |
|---|---|
| Unit | Each crate tests its own logic in isolation. No bus, no OS, no model. Pure functions only. |
| Integration | `tests/` at workspace root. Spins up a real bus with mock actors. Tests event flow. |
| Platform | Per-OS CI runners. Platform adapter tested on real OS. |
| Inference | Tested with a minimal quantized GGUF (q4_0). CI skips GPU tests if hardware unavailable. |
| Soul | SQLite migrations tested exhaustively. Every migration has a before/after fixture. |

**Hard rules:**
- No test may write to the user's real config or Soul database. Tests use temp dirs.
- No test may hit a remote endpoint.
- `cargo test` must pass on all three platforms before any merge to main.

---

## 14. Speech

Lives in `crates/speech`. This is Sena's primary user-facing interaction surface.

### 14.1 Architecture

Speech is two independent actors:
- **STT Actor:** Captures microphone audio, detects speech, transcribes via whisper-rs v0.16.0
- **TTS Actor:** Receives text, synthesizes speech via Piper (preferred) or OS platform TTS

Both actors communicate exclusively via the bus. They never call each other directly.

### 14.2 STT Pipeline

```
Microphone → cpal audio capture → Voice Activity Detection → whisper-rs v0.16.0 transcription → TranscriptionCompleted event on bus
```

Modes:
- **Wakeword mode (default):** Always listening for wakeword ("Sena") via dedicated tiny model (OpenWakeWord, ~5MB). After wakeword detected, captures speech until silence, then transcribes.
- **Push-to-talk mode:** STT activates only on explicit user action (hotkey or tray button).

Transcription results include word-level confidence scores. Words with confidence below threshold (default 0.6) are flagged in CLI output with visual indicators.

### 14.3 TTS Pipeline

```
SpeakRequested event → Piper synthesis (or OS fallback) → cpal audio playback → SpeechOutputCompleted event
```

Voice personality is warm and concise, derived from Soul state. TTS is the default output for Sena's thoughts and responses.

### 14.4 Model Management

Speech models are downloaded from HuggingFace on first enable:
- Whisper GGUF (~150MB for small model) — transcribed via whisper-rs v0.16.0
- Piper voice model (~60MB)  
- OpenWakeWord model (~5MB)

This is the **only** network exception in Sena's local-first architecture. Downloads are:
- User-consented (explicit enable in config or onboarding)
- Model weights only (no user data transmitted)
- Cached locally after first download

Real-time VRAM usage monitoring is available when GPU acceleration is active. VRAM status is polled every 10 seconds and displayed in the CLI sidebar.

### 14.5 Hard Rules

- STT never captures or stores raw audio persistently. Audio is processed in a rolling in-memory buffer only.
- TTS never sends text to external services. All synthesis is local.
- Wakeword detection model must be <= 20MB and idle CPU < 1%.
- Speech actors failing does not affect core CTP/inference/memory loop.
- All speech functionality degrades gracefully: if no model available, speech is disabled and CLI/tray remain functional.

### 14.6 Sequenced TTS Streaming Queue

When the inference actor operates in streaming mode (user voice or text input), it emits `InferenceSentenceReady` events as sentence boundaries are detected. The TTS actor handles these with an ordered synthesis and playback queue:

1. **Synthesis**: Each `InferenceSentenceReady` spawns a `spawn_blocking` synthesis task. Tasks run in parallel.
2. **Ordered playback**: Synthesized sentences accumulate in a `BTreeMap<sentence_index, SynthResult>`. The actor plays from `next_play_index` upward, ensuring correct order even if synthesis tasks complete out of order.
3. **Queue depth**: Bounded by `speech.tts_queue_depth` (default 5). When full, the oldest pending entry is dropped.
4. **Stream completion**: On `InferenceStreamCompleted`, the actor drains pending synthesis results (up to 30s) and plays all remaining entries in order.
5. **Interruption**: On `TranscriptionCompleted` (new voice input while stream active), the queue is cleared immediately and in-flight synthesis results are discarded (stale request_id check).

The existing `SpeakRequested` path (for proactive/system messages) continues to work independently via the separate FIFO queue.

**Hard rules:**
- Never write partial sentence audio or text to Soul or memory — only the full response (emitted via `InferenceStreamCompleted`) is persisted by the inference actor.
- Proactive (CTP-triggered) inference uses the batch `SpeakRequested` path, not the streaming queue.
- TTS actor still depends only on `bus`. No direct imports of `inference` or any other Sena crate.

---

## 15. Encryption Architecture

All sensitive persistent state is encrypted at rest. This is a **Phase 2 entry gate requirement** — no sensitive data is written to disk without encryption in place.

### 15.1 Scope

| File | Encrypted | Owner |
|---|---|---|
| Soul redb database | Yes | `crates/soul` |
| ech0 graph (redb) | Yes | `crates/memory` |
| ech0 vector index (hora) | Yes | `crates/memory` |
| Config file (TOML) | No — contains no sensitive data by design | `crates/runtime` |
| Logs | No — logs must never contain sensitive content (enforced, see §15.4) | `crates/runtime` |

### 15.2 Encryption Model

**Envelope encryption** with AES-256-GCM:

```
[Master Key]
    ↓ (used to derive)
[Data Encryption Key (DEK)] — unique per file
    ↓ (encrypts)
[File contents on disk]

[Master Key] is never stored on disk in plaintext.
```

### 15.3 Key Storage

Two modes, tried in order:

**Primary — OS Keychain:**

| OS | Mechanism | Crate |
|---|---|---|
| macOS | Keychain Services | `keyring` |
| Windows | Windows Credential Manager | `keyring` |
| Linux | Secret Service (libsecret / KWallet) | `keyring` |

The master key is stored as a `keyring` entry under service name `sena` on first boot. Retrieved on every subsequent boot.

**Fallback — User Passphrase:**

If the OS keychain is unavailable (CI, headless server, user preference), the user provides a passphrase. The master key is derived via **Argon2id** with stored salt. The salt is stored in plaintext next to the encrypted files — this is safe; the salt only prevents rainbow table attacks and is not secret.

```
Master Key = Argon2id(passphrase, salt, memory=64MB, iterations=3, parallelism=1)
```

**Hard rules:**
- The master key is never written to disk in any form.
- The DEK is never written to disk unencrypted.
- Passphrase is zeroed from memory immediately after key derivation using `zeroize`.
- Re-encryption on passphrase change: new DEK generated, all files re-encrypted, old DEK zeroed. This is an atomic operation — partial re-encryption is treated as corruption on next boot.

### 15.4 Log Sanitization

Logs must never contain:
- Memory node content
- Soul event data
- Clipboard text (even digests)
- File paths that could reveal user behavior

Structured log fields for sensitive subsystems use redacted markers: `[REDACTED]`. This is enforced by logging wrapper types in `crates/soul` and `crates/memory` — raw content types do not implement `Display` or `Debug` in ways that would expose content to the log sink.

### 15.5 Approved Crates

| Purpose | Crate |
|---|---|
| AES-256-GCM encryption | `aes-gcm` |
| Argon2id key derivation | `argon2` |
| OS keychain access | `keyring` |
| Secure memory zeroing | `zeroize` |
| Random nonce/salt generation | `rand` with `getrandom` backend |

---

## 16. Revision Policy

This document is changed by pull request only. Changes require:
1. Updated version number (semver patch for clarifications, minor for additions, major for breaking changes)
2. A summary of what changed and why
3. Reconciliation note if PRD.md is also affected

Any implementation that contradicts this document without a corresponding approved revision to this document is considered a defect.

---

## 17. Inference Architecture Extension: Streaming Pipeline

This section governs Sena's streaming inference pipeline. It specifies the data flow from token emission to ordered TTS playback and defines the hard rules for each component.

### 17.1 What is this?

Sena's inference pipeline has two modes:

| Mode | Source | Path | Output |
|---|---|---|---|
| **Streaming** | `UserVoice`, `UserText` | `process_streaming_inference_with_context` | `InferenceTokenGenerated` + `InferenceSentenceReady` + `InferenceStreamCompleted` |
| **Batch** | `ProactiveCTP`, `Iterative` | `process_single_inference_with_context` | `InferenceCompleted` (single event) |

The streaming path uses `InferenceBackend::stream()` from the `infer` crate, which returns a `std::sync::mpsc::Receiver<String>`. Tokens are bridged from the sync mpsc channel to a tokio channel via `spawn_blocking`.

### 17.2 Streaming pipeline

```
User input (voice or text)
  │
  ├─ InferenceRequested { source: UserVoice | UserText }
  │
  └─ InferenceActor: process_streaming_inference_with_context()
       ├─ Query memory (SINGLE_ROUND_MEMORY_TIMEOUT)
       ├─ Query soul (SINGLE_ROUND_MEMORY_TIMEOUT)
       ├─ Acquire vision frame (if model is vision-capable)
       ├─ Build enriched prompt
       ├─ Wrap with ChatTemplate
       ├─ backend.stream(params) → std::sync::mpsc::Receiver<String>
       │   (in spawn_blocking, bridged to tokio::sync::mpsc)
       │
       ├─ Per-token loop:
       │   ├─ Emit InferenceTokenGenerated
       │   ├─ Append to buffer + full_text
       │   └─ text::detect_sentence_boundary(buffer, max_buffer, max_sentence)
       │       ├─ Some(sentence, remainder) → Emit InferenceSentenceReady
       │       │                               Reset buffer to remainder
       │       └─ None → continue
       │
       ├─ Stream closed: flush non-empty buffer → final InferenceSentenceReady
       ├─ Emit InferenceStreamCompleted (full text, total tokens, total sentences)
       ├─ Write full_text to memory (NEVER partial)
       └─ Emit InferenceCompleted (backward compat for IPC/CLI)
```

### 17.3 Sentence boundary detection

Lives in `crates/text/src/sentence.rs`. Exported as `text::detect_sentence_boundary`.

**Function signature:**
```rust
pub fn detect_sentence_boundary(
    buffer: &str,
    max_buffer_chars: usize,
    max_sentence_chars: usize,
) -> Option<(String, String)>
```

**Boundary rules in priority order:**
1. **Hard boundary**: `.`, `?`, or `!` followed by whitespace or end-of-string
2. **Soft boundary**: `;` followed by whitespace
3. **Comma threshold**: `,` followed by whitespace, only when `buffer.len() > max_buffer_chars`
4. **Hard cap**: when `buffer.len() > max_sentence_chars`, split at nearest whitespace before threshold

**Hard rules:**
- Pure function: no state, no side effects, deterministic.
- Thresholds come from config: `inference.streaming.max_buffer_chars` (default 150), `inference.streaming.max_sentence_chars` (default 400).

### 17.4 InferenceSource

`InferenceSource` is the authoritative source-of-origin field on every `InferenceRequested` event. It replaces the fragile `request_id < 1000` convention.

| Variant | Meaning | Inference path |
|---|---|---|
| `UserVoice` | STT transcription triggered | Streaming |
| `UserText` | CLI text input | Streaming |
| `ProactiveCTP` | CTP-triggered proactive thought | Batch |
| `Iterative` | Multi-round reasoning chain | Batch |

**Hard rule:** No code path may infer whether an inference is proactive from `request_id` values. All proactive detection must use `InferenceSource::ProactiveCTP`.

### 17.5 Hard rules

- Streaming inference writes `full_text` to memory ONLY after `InferenceStreamCompleted` — never partial sentences.
- Batch inference continues to use `InferenceCompleted` as its sole response event.
- `InferenceSentenceReady` events are consumed only by the TTS actor (and optionally by the CLI for display — display-only, not persisted).
- `InferenceTokenGenerated` events are for display/debugging only. No actor persists tokens.
- Proactive (CTP) responses always use `SpeakRequested` for TTS — never the streaming `InferenceSentenceReady` path.
- The `infer` crate's `stream()` method is always called inside `spawn_blocking`. Never called in async context directly (it blocks).

### 17.6 Connections

| This subsystem | Connects to |
|---|---|
| Streaming inference output | TTS actor (via `InferenceSentenceReady`) |
| Stream completion | Memory actor (via `MemoryWriteRequest` — full text only) |
| Sentence boundary detection | `crates/text` (leaf node, no Sena deps) |
| Source routing | `InferenceSource` enum in `bus::events::inference` |
| Backend | `infer` crate (external, tag v0.1.0) |

