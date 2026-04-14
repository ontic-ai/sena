# Architecture Digest
Source: docs/architecture.md (v0.6.0)
Last synced: 2026-04-11

## Crate Dependency Graph
```
cli
 ├── runtime      ← only dependency
 └── bus          ← event subscriptions

runtime           ← composition root: constructs ALL actors
 ├── bus
 ├── crypto
 ├── soul
 ├── platform
 ├── ctp
 ├── memory
 ├── inference
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
 (no Sena crate dependencies — leaf node)

platform
 └── bus

speech
 └── bus
```

### Import Legality Matrix
| Crate | May import | May NOT import |
|---|---|---|
| bus | (none — leaf) | all other Sena crates |
| crypto | (none — leaf) | all other Sena crates |
| text | (none — leaf) | all other Sena crates |
| cli | runtime, bus | soul, platform, ctp, memory, inference, prompt, speech, crypto |
| runtime | bus, crypto, soul, platform, ctp, memory, inference, speech | cli, prompt, text |
| ctp | bus, platform | runtime, soul, memory, inference, prompt, speech, crypto, cli |
| inference | bus, text | runtime, soul, platform, ctp, memory, prompt, speech, crypto, cli |
| memory | bus, crypto, soul | runtime, platform, ctp, inference, prompt, speech, cli |
| prompt | bus | runtime, soul, platform, ctp, memory, inference, speech, crypto, cli |
| soul | bus, crypto | runtime, platform, ctp, memory, inference, prompt, speech, cli |
| platform | bus | runtime, soul, ctp, memory, inference, prompt, speech, crypto, cli |
| speech | bus | runtime, soul, platform, ctp, memory, inference, prompt, crypto, cli |

## Hard Rules

### §1 Workspace Layout
- R1.1: Root Cargo.toml is virtual manifest. No `[package]` section. No `src/`.
- R1.2: All crates under `crates/`. No `sena-` prefix. Names are functional.
- R1.3: No `Makefile`. No shell scripts. All automation in `xtask/`.

### §2 Dependency Graph
- R2.1: `runtime` is composition root. Constructs ALL actor instances.
- R2.2: `cli` may only import `runtime` and `bus`. All others forbidden.
- R2.3: `crypto` has zero Sena dependencies. Like `bus`, it is leaf node.
- R2.4: `soul` has no knowledge of `ctp`, `inference`, `memory`, or `prompt`.
- R2.5: `speech` depends only on `bus`. Never imports inference/memory/soul.
- R2.6: `bus` has zero Sena dependencies. Bottom of graph.
- R2.7: Circular dependencies are build errors. Graph must remain DAG.
- R2.8: `cli` has no business logic. If logic appears there, it belongs elsewhere.

### §3 The Bus
- R3.1: No string-typed events. Every event is typed struct/enum.
- R3.2: Events are `Clone + Send + 'static`. No exceptions.
- R3.3: Events carry no logic. Methods on event types forbidden.
- R3.4: New events added to `events.rs` first, before any emit/subscribe code.
- R3.5: Events are immutable once sent. No actor modifies received events.
- R3.6: All events defined in `crates/bus/src/events/`.

### §3.3 Actor Trait
- R3.3.1: Every actor owns its own state. No shared mutable state.
- R3.3.2: Actors communicate exclusively via bus. Direct calls forbidden.
- R3.3.3: Panicking actor must not bring down others. Runtime catches panics.
- R3.3.4: Actors do not block async executor. Use `spawn_blocking`.

### §4 Runtime
- R4.1: Boot order is strict (1–12, documented in architecture.md §4.1).
- R4.2: Readiness gate: 30s timeout for all actors to emit `ActorReady`.
- R4.3: CLI is wrapper, not owner. All business logic in daemon actors.
- R4.4: No `process::exit()` outside runtime shutdown handler.
- R4.5: Shutdown timeout configurable. Default 5s per actor.
- R4.6: Soul always flushes before exit.

### §5 Platform Adapter
- R5.1: `PlatformAdapter` is only place OS-specific code lives.
- R5.2: Keystroke captures timing only. Characters NEVER captured/stored.
- R5.3: Clipboard content passed as digest in memory. Never verbatim.
- R5.4: File event scope configurable.

### §6 CTP
- R6.1: `ContextSnapshot` contains no raw keystroke characters. Build fails if char/String field added to KeystrokeCadence.
- R6.2: CTP never calls inference directly. Emits `ThoughtEventTriggered`.
- R6.3: Trigger gate must be tunable via config.
- R6.4: If Sena observes it, CTP must know about it (signal completeness).
- R6.5: Pattern detection is rule-based. Thresholds tunable via config.
- R6.6: User state is ephemeral — computed per tick, never persisted.
- R6.7: Task inference is synchronous and fast (<5ms).

### §7 Inference
- R7.1: Model weights not loaded at boot. Loaded on first request.
- R7.2: Inference runs in `spawn_blocking`. Never blocks async runtime.
- R7.3: Inference actor never reads memory/soul directly. Receives composed prompt.
- R7.4: `InferenceSource` replaces `request_id < 1000` detection convention.

### §8 Memory
- R8.1: `memory` never calls llama-cpp-rs directly. Goes through `inference`.
- R8.2: `inference` never calls ech0 directly. Only responds to embed/extract requests.
- R8.3: `Embedder` and `Extractor` implementations live exclusively in `crates/memory`.
- R8.4: Raw clipboard text never passed to `store.ingest_text()`.
- R8.5: Raw keystroke data never passed to `store.ingest_text()`.
- R8.6: Working memory never written to ech0 or disk. Ephemeral.
- R8.7: `ConflictResolution::Overwrite` never called silently. Logged to Soul first.
- R8.8: ech0 `Store` owned exclusively by memory actor.
- R8.9: Context queries never trigger consolidation or ingestion. Read-only.

### §9 Prompt Composition
- R9.1: No hardcoded strings in prompt assembly.
- R9.2: Token budget always respected via llama-cpp-rs tokenizer.
- R9.3: Prompt assembly has no side effects. Pure transformation.

### §10 SoulBox
- R10.1: Soul starts empty. Never pre-seeded.
- R10.2: Soul evolves through observation/interaction only.
- R10.3: Schema changes require migrations. Breaking migrations forbidden.
- R10.4: No other crate writes to Soul redb directly. mpsc channel only.
- R10.5: Event log is append-only. No system deletion.
- R10.6: Soul's internal types never exposed in pub APIs.
- R10.7: Soul does not perform inference. Stores and retrieves only.
- R10.8: All intelligence modules are `mod` (private). Only events are public.
- R10.9: Harvest cycle triggered by event count, not time interval.

### §11 Error Handling
- R11.1: No `unwrap()` or `expect()` outside tests.
- R11.2: Errors are typed. Each crate defines own Error enum.
- R11.3: `anyhow` permitted in `cli` only.
- R11.4: User-facing errors human-readable. Internal errors structured.
- R11.5: Sena never panics in production.

### §12 Configuration
- R12.1: Config format: TOML.
- R12.2: All tunable thresholds in config.
- R12.3: Config loaded once at boot, immutable for session.
- R12.4: No hardcoded values for tunables.

### §13 Testing
- R13.1: No test writes to real config/Soul/home directory. Use temp dirs.
- R13.2: No test hits remote endpoint.
- R13.3: `cargo test` must pass on all three platforms.

### §14 Speech
- R14.1: STT never captures or stores raw audio persistently.
- R14.2: TTS never sends text to external services.
- R14.3: Wakeword model must be <= 20MB and idle CPU < 1%.
- R14.4: Speech actors failing does not affect core CTP/inference/memory loop.
- R14.5: Never write partial sentence audio/text to Soul or memory.
- R14.6: Proactive inference uses batch path, not streaming queue.
- R14.7: TTS actor still depends only on bus. No direct inference import.

### §15 Encryption
- R15.1: All persistent sensitive state encrypted at rest.
- R15.2: Encrypted files: Soul redb, ech0 graph (redb), ech0 vector index (hora).
- R15.3: Config and logs not encrypted (no sensitive data by design).
- R15.4: Master key never written to disk.
- R15.5: DEK never written to disk unencrypted.
- R15.6: Passphrase zeroed immediately after Argon2 derivation.
- R15.7: Re-encryption on passphrase change is atomic.
- R15.8: Logs must never contain sensitive content.

## Bus Event Ownership
| Module | Events |
|---|---|
| `system` | ShutdownSignal, BootComplete, ActorReady, ActorFailed, ActorStopped, ShutdownComplete, ConfigSetRequested, ConfigReloaded, LoopControlRequested, LoopStatusChanged, CliAttachRequested |
| `platform` | WindowChanged, ClipboardChanged, FileEvent, KeystrokePattern |
| `platform_vision` | VisionFrameReady, VisionFrameRequested |
| `ctp` | ContextSnapshotReady, ThoughtEventTriggered, UserStateComputed, SignalPatternDetected, EnrichedTaskInferred |
| `inference` | InferenceRequested, InferenceCompleted, InferenceStatusUpdate, InferenceTokenGenerated, InferenceSentenceReady, InferenceStreamCompleted, InferenceSource |
| `memory` | MemoryWriteRequest, MemoryQueryRequest, MemoryQueryResponse, ContextMemoryQueryRequest, ContextMemoryQueryResponse |
| `soul` | SoulSummaryReady, SoulEventLogged, IdentitySignalDistilled, TemporalPatternDetected, PreferenceLearningUpdate, RichSummaryRequested, RichSummaryReady |
| `speech` | SpeakRequested, SpeechOutputCompleted, SpeechFailed, TranscriptionCompleted, TranscriptionWordReady, LowConfidenceTranscription, VoiceInputDetected, ListenModeRequested, ListenModeTranscription, ListenModeStopped, WakewordDetected, WakewordSuppressed, WakewordResumed |
| `download` | DownloadStarted, DownloadProgress, DownloadCompleted, DownloadFailed |
| `transparency` | TransparencyQueryRequested, TransparencyQueryCompleted |

## Registered Background Loops
| Loop name | Actor | Default | Description |
|---|---|---|---|
| `ctp` | CTPActor | enabled | Continuous thought processing |
| `memory_consolidation` | MemoryActor | enabled | Periodic memory consolidation |
| `platform_polling` | PlatformActor | enabled | Platform signal polling |
| `screen_capture` | PlatformActor | enabled | Screen capture for vision models |
| `speech` | SttActor / WakewordActor | enabled | Speech input loop |
| `vram_monitor` | Boot task | enabled | Real-time VRAM usage monitoring |

## Boot Sequence
1. Config load — read user config or create defaults
2. Encryption init — derive or retrieve master key
3. EventBus init — bus live before any actor starts
4. Soul init — schema loaded/migrated before writes
5. Core actors spawn — Soul, Platform, CTP, Memory, Inference
6. Platform adapter — OS signal collection begins
7. CTP actor — observation loop begins
8. Memory actor — loads indexes, prepares write queues
9. Inference actor — discovers models (no weight loading)
10. Prompt actor — ready to compose
11. Speech actors — STT, TTS spawned conditionally
12. System tray — tray icon in dedicated thread
13. (BootComplete emitted by supervisor after readiness gate)

## Actor Communication Contract
- Actors communicate ONLY via bus
- Direct function calls between actors FORBIDDEN
- Actor panics caught by runtime, emit `ActorFailed`
- Blocking work in `tokio::task::spawn_blocking`

## Encryption Rules
- AES-256-GCM envelope encryption
- Primary: OS keychain (keyring crate)
- Fallback: Argon2id from user passphrase
- Master key never on disk
- DEK unique per file
- Nonces fresh per encryption (rand + getrandom)
- All key types implement `ZeroizeOnDrop`
- Custom Debug impls that redact key content
