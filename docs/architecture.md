# Sena — Architecture
**Version:** 0.4.0  
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
│   ├── runtime/            ← boot sequence, actor registry, shutdown
│   ├── platform/           ← OS adapter trait + per-OS implementations
│   ├── ctp/                ← continuous thought processing loop
│   ├── inference/          ← llama-cpp-rs wrapper, model manager, queue
│   ├── memory/             ← tiered memory, dual-routing, consolidation
│   ├── prompt/             ← dynamic prompt composition engine
│   ├── soul/               ← SoulBox: identity, schema, event log
│   ├── speech/             ← STT, TTS, wakeword — primary interaction surface
│   └── cli/                ← binary entrypoint — thin shell only
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
- Crates are published to crates.io only if explicitly decided. Default: workspace-internal.
- No `Makefile`. No shell scripts in root. All automation lives in `xtask/`.

---

## 2. Dependency Graph

This is the law. Arrows mean "may depend on." Absence of an arrow means the dependency is **forbidden.**

```
cli
 ├── runtime      ← only dependency (runtime re-exports discover_models, ollama_models_dir)
 └── bus          ← event subscriptions and slash command dispatch

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
 └── bus

memory
 ├── bus
 ├── crypto
 └── soul

prompt
 ├── memory
 ├── ctp
 └── inference

soul
 ├── bus
 └── crypto

platform
 └── bus

speech
 └── bus
```

**Hard rules:**
- `runtime` is the composition root. It constructs all concrete actor instances (soul, platform, ctp, memory, inference, speech) inside `boot()`. CLI never constructs actors.
- `cli` may only import `runtime` and `bus`. All other crate imports in `cli` are forbidden. The `runtime` crate re-exports `discover_models`, `ollama_models_dir`, etc. so CLI can use these without importing `inference` or `platform` directly.
- `crypto` has zero dependencies on any other Sena crate. Like `bus`, it is a leaf node in the graph. It provides encryption primitives consumed by `runtime`, `soul`, and `memory`.
- `soul` has no knowledge of `ctp`, `inference`, `memory`, or `prompt`. It only knows `bus` and `crypto`. Other crates emit events; soul absorbs them. Soul's internals are never reached into from outside.
- `speech` depends only on `bus`. It receives events and emits events. It never imports `inference`, `memory`, or `soul`.
- `bus` has zero dependencies on any other Sena crate. It is the bottom of the graph.
- `cli` is the developer-facing tool surface. It has no business logic and no actor construction. If business logic appears in `cli`, it belongs in another crate.
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
pub mod inference { ... }  // InferenceRequest, InferenceResponse
pub mod memory { ... }     // MemoryWriteRequest, MemoryQueryRequest, MemoryQueryResponse
pub mod soul { ... }       // SoulEventLogged, IdentitySignalEmitted
```

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

The runtime lives in `crates/runtime`. It owns the boot sequence, the actor registry, and the shutdown protocol.

### 4.1 Boot Sequence

Order is strict and non-negotiable:

```
1. Config load           — read user config from disk (or create defaults)
2. Encryption init       — derive or retrieve master key; must complete before any store opens
3. EventBus init         — bus is live before any actor starts
4. Soul init             — SoulBox schema loaded/migrated before anything writes to it
5. Core actors spawn     — bus, runtime internal actors
6. Platform adapter      — OS signal collection begins
7. CTP actor             — begins observation loop
8. Memory actor          — loads indexes, prepares write queues
9. Inference actor       — discovers models, does NOT load weights yet
10. Prompt actor         — ready to compose, idle until ThoughtEvent
11. BootComplete event   — emitted on bus. Actors waiting on this may now activate.
```

If any step from 1–4 fails, Sena exits with a clear error. Steps 5–10 failing emit `ActorFailed` and Sena continues in degraded mode.

### 4.2 Shutdown Protocol

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

### 4.3 Actor Registry

The registry maps actor names to their `JoinHandle`. The runtime uses this to monitor liveness and restart failed actors within policy.

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
    pub active_app: AppContext,
    pub recent_files: Vec<FileEvent>,
    pub clipboard_digest: Option<String>,   // digest/summary, not raw content
    pub keystroke_cadence: KeystrokeCadence,
    pub session_duration: Duration,
    pub inferred_task: Option<TaskHint>,
    pub timestamp: Instant,
}
```

**Hard rules:**
- `ContextSnapshot` contains no raw keystroke characters. Build fails if a char/String field is added to `KeystrokeCadence`.
- CTP never calls the inference layer directly. It emits `ThoughtEventTriggered` on the bus.
- The trigger gate must be tunable without code changes. Thresholds live in config.

---

## 7. Inference

Lives in `crates/inference`. Wraps llama-cpp-rs.

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
| **Semantic memory** | ech0 vector index | `usearch` index file on disk |

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

Both ech0 storage files (`redb` graph, `usearch` index) are encrypted. See §15 for the full encryption architecture. The `memory` actor is responsible for providing ech0's `StorePathConfig` with paths that point to encrypted file handles — ech0 itself has no knowledge of encryption.

**Hard rules:**
- Raw clipboard text is never passed to `store.ingest_text()`.
- Raw keystroke data is never passed to `store.ingest_text()`.
- Working memory (`Vec<MemoryChunk>`) is never written to ech0 or disk. It is ephemeral.
- `ConflictResolution::Overwrite` is never called silently. Any overwrite is logged to Soul first.
- The ech0 `Store` instance is owned exclusively by the memory actor. No other actor holds a reference to it.

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
- **STT Actor:** Captures microphone audio, detects speech, transcribes via Whisper.cpp
- **TTS Actor:** Receives text, synthesizes speech via Piper (preferred) or OS platform TTS

Both actors communicate exclusively via the bus. They never call each other directly.

### 14.2 STT Pipeline

```
Microphone → cpal audio capture → Voice Activity Detection → Whisper.cpp transcription → TranscriptionCompleted event on bus
```

Modes:
- **Wakeword mode (default):** Always listening for wakeword ("Sena") via dedicated tiny model (OpenWakeWord, ~5MB). After wakeword detected, captures speech until silence, then transcribes.
- **Push-to-talk mode:** STT activates only on explicit user action (hotkey or tray button).

### 14.3 TTS Pipeline

```
SpeakRequested event → Piper synthesis (or OS fallback) → cpal audio playback → SpeechOutputCompleted event
```

Voice personality is warm and concise, derived from Soul state. TTS is the default output for Sena's thoughts and responses.

### 14.4 Model Management

Speech models are downloaded from HuggingFace on first enable:
- Whisper GGUF (~150MB for small model)
- Piper voice model (~60MB)  
- OpenWakeWord model (~5MB)

This is the **only** network exception in Sena's local-first architecture. Downloads are:
- User-consented (explicit enable in config or onboarding)
- Model weights only (no user data transmitted)
- Cached locally after first download

### 14.5 Hard Rules

- STT never captures or stores raw audio persistently. Audio is processed in a rolling in-memory buffer only.
- TTS never sends text to external services. All synthesis is local.
- Wakeword detection model must be <= 20MB and idle CPU < 1%.
- Speech actors failing does not affect core CTP/inference/memory loop.
- All speech functionality degrades gracefully: if no model available, speech is disabled and CLI/tray remain functional.

---

## 15. Encryption Architecture

All sensitive persistent state is encrypted at rest. This is a **Phase 2 entry gate requirement** — no sensitive data is written to disk without encryption in place.

### 15.1 Scope

| File | Encrypted | Owner |
|---|---|---|
| Soul redb database | Yes | `crates/soul` |
| ech0 graph (redb) | Yes | `crates/memory` |
| ech0 vector index (usearch) | Yes | `crates/memory` |
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
