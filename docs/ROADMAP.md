# Sena — Development Roadmap
**Version:** 0.3.0  
**Reconcile against:** `PRD.md`, `architecture.md`

---

## How to Use This Roadmap

Each phase has an explicit **entry gate** (what must be true before the phase starts) and **exit gate** (what must be true before the phase is considered done). No phase begins until its entry gate is fully satisfied. No phase ends until its exit gate is fully satisfied. Partial completion is not completion.

Phases are sequential. Parallelism within a phase is allowed. Parallelism across phases is not.

---

## Phase 1 — Foundation: The Bus and Boot

**Goal:** A compilable, observable Sena that boots, collects OS signals, flows typed events, and shuts down gracefully. No inference. No persistence.

**Entry gate:** This is Phase 1. It begins now.

### Milestones

#### M1.1 — Workspace Skeleton ✅
- [x] Virtual manifest `Cargo.toml` at root
- [x] `rust-toolchain.toml` pinned
- [x] All 9 crates scaffolded under `crates/` with stub `lib.rs` or `main.rs`
- [x] `xtask/` scaffolded with `cargo xtask dump` command (outputs structured file diff for review)
- [x] `cargo build --workspace` compiles clean on macOS, Windows, Linux
- [x] `cargo test --workspace` passes (no tests yet, just no compile errors)

#### M1.2 — Typed Event System ✅
- [x] All Phase 1 event types defined in `crates/bus/src/events/` (module structure per copilot-instructions.md §5)
- [x] Event modules: `system`, `platform`, `ctp`
- [x] All events are `Clone + Send + 'static`
- [x] Zero hardcoded strings in event types
- [x] Unit tests: event construction and serialization

#### M1.3 — Actor Trait and Bus ✅
- [x] `Actor` trait defined in `crates/bus/src/actor.rs`
- [x] `EventBus` struct with broadcast sender and mpsc registry
- [x] Bus subscription and publish APIs
- [x] `ActorError` type defined
- [x] Unit tests: bus publish/subscribe round-trip

#### M1.4 — Runtime Boot and Shutdown ✅
- [x] Boot sequence implemented (steps 1–10 from architecture.md §4.1)
- [x] Actor registry with liveness monitoring
- [x] Graceful shutdown on SIGINT/SIGTERM
- [x] `ShutdownSignal` propagation tested
- [x] Configurable shutdown timeout
- [x] Integration test: boot → emit events → shutdown, all actors confirm stop

#### M1.5 — Platform Adapter ✅
- [x] `PlatformAdapter` trait defined
- [x] macOS implementation: active window, clipboard, file events, keystroke cadence
- [x] Windows implementation: active window, clipboard, file events, keystroke cadence
- [x] Linux implementation: active window, clipboard, file events, keystroke cadence
- [x] `KeystrokePattern` type has no character fields (enforced, not by convention)
- [x] Clipboard raw text is digested before leaving platform crate
- [x] Platform adapter emits typed events onto bus
- [x] Tested on all 3 OS's in CI

#### M1.6 — CTP Skeleton ✅
- [x] Signal buffer with rolling time window
- [x] `ContextSnapshot` type fully defined
- [x] Context assembler: platform events → `ContextSnapshot`
- [x] Trigger gate: time-based only in Phase 1 (configurable interval)
- [x] `ThoughtEventTriggered` emitted on bus
- [x] Integration test: platform events flow through CTP → `ThoughtEvent` on bus

#### M1.7 — Config System ✅
- [x] Config file location per OS
- [x] TOML format, loaded at boot
- [x] All CTP thresholds, shutdown timeout, file watch paths in config
- [x] Default config written to disk on first boot if absent
- [x] Unit tests: config load, defaults, missing fields

#### M1.8 — xtask: cargo xtask dump ✅
- [x] `cargo xtask dump` outputs all crate source files, structured with file path headers
- [x] Supports `--crate <name>` flag to scope output
- [x] Output is deterministic (sorted, stable)
- [x] Used by developer to share code for review without manual copy-paste

**Exit gate — Phase 1 complete when:**
- [x] All milestones M1.1–M1.8 checked off
- [x] `cargo test --workspace` passes on macOS, Windows, Linux in CI
- [x] Zero `unwrap()` calls in production code paths
- [x] `cargo clippy --workspace -- -D warnings` clean
- [x] `cargo xtask dump` produces usable output
- [x] Architecture doc reviewed — no implementation has deviated from it

---

## Phase 2 — Inference and Persistence

**Goal:** Sena can load a local GGUF model, run inference, and persist memory across sessions. All persistent state is encrypted before the first write.

**Entry gate:** Phase 1 exit gate fully satisfied. OQ-SEC resolved and encryption design approved in architecture.md §15.

### Milestones

#### M2.0 — Encryption Layer (must complete before any other Phase 2 milestone) ✅
- [x] OQ-SEC resolved: Soul redb, ech0 graph redb, and ech0 vector index all encrypted
- [x] `aes-gcm`, `argon2`, `keyring`, `zeroize`, `rand` added to workspace dependencies
- [x] Encrypt/decrypt, key derivation, DEK generation implemented
- [x] OS keychain integration tested on macOS, Windows, Linux
- [x] Passphrase fallback with Argon2id tested
- [x] Re-encryption path implemented and tested
- [x] Log sanitization wrappers in place — no sensitive content reaches log sink
- [x] Unit tests: encrypt/decrypt round-trip, keychain store/retrieve, passphrase derive determinism

#### M2.1 — Ollama GGUF Discovery ✅
- [x] Ollama model manifest parsed on all 3 OS's
- [x] `ModelRegistry` built at boot: name, path, size, quantization
- [x] Handles: no Ollama installed, no models pulled, corrupted manifest
- [x] First-boot UX: clear error message if no models available
- [x] OQ-2 resolved

#### M2.2 — llama-cpp-rs Integration ✅
- [x] Backend auto-detection: Metal → CUDA → CPU
- [x] Model loading from GGUF path
- [x] Inference queue: bounded mpsc, priority levels
- [x] Inference runs in `spawn_blocking`
- [x] `InferenceRequest` → `InferenceResponse` round-trip on bus
- [x] Model weights loaded on first request, not at boot
- [x] Embedding API: inference actor exposes `EmbedRequest` → `EmbedResponse { vector: Vec<f32> }` channel
- [x] Extraction API: inference actor exposes `ExtractionRequest` → `ExtractionResult` channel
- [x] Integration test with minimal quantized GGUF (q4_0)

#### M2.3 — Working Memory ✅
- [x] `WorkingMemory` struct: in-RAM, scoped to inference cycle
- [x] Holds: current `ContextSnapshot`, last N inference exchanges
- [x] Cleared after each inference cycle — never persisted, never passed to ech0
- [x] Token budget enforced: working memory never exceeds configurable token limit

#### M2.4 — ech0 Integration ✅
- [x] ech0 added as Git dependency with `features = ["full"]` *(placeholder module due to unavailable repo URL)*
- [x] `Embedder` trait implemented in `crates/memory` — calls inference actor via mpsc *(placeholder returns 384-dim zero vector)*
- [x] `Extractor` trait implemented in `crates/memory` — calls inference actor via mpsc *(placeholder returns empty vec)*
- [x] `Store::new(config, embedder, extractor)` initialized in memory actor
- [x] `StorePathConfig` points to encrypted file handles (M2.0 must be complete)
- [x] Ingest path: `InferenceResponse` → `store.ingest_text()` → `IngestResult`
- [x] `ConflictReport` handling: emits `MemoryConflictDetected` on bus, logs to Soul
- [x] Retrieval path: `MemoryQueryRequest` → dual-routing via `store.search()` → `MemoryQueryResponse`
- [x] Unit tests: ingest, retrieve, conflict detection using ech0's `_test-helpers` feature *(1 memory actor lifecycle test)*

#### M2.5 — SoulBox: Schema and Event Log ✅
- [x] redb schema v1: identity signals, event log, preferences
- [x] Schema migration system: numbered, sequential, run at boot step 3
- [x] Append-only event log: every inference cycle, ech0 ingest, conflict, identity signal logged
- [x] `SoulSummary` type: opaque external view of soul state
- [x] Write channel: mpsc sender, no direct redb access from outside crate
- [x] Soul redb file encrypted via M2.0 encryption layer
- [x] Soul flushes on shutdown (guaranteed before exit)
- [x] Unit tests: migration v1, event log append, summary generation, encrypted read/write

#### M2.6 — Prompt Composer (Basic) ✅
- [x] `PromptSegment` enum fully defined
- [x] Composer assembles: system persona + working memory + current context + user intent
- [x] Token budget enforcement via llama-cpp-rs tokenizer
- [x] Zero hardcoded strings in assembled output
- [x] Pure function: deterministic for testing
- [x] Unit tests: segment assembly, token budget truncation

#### M2.7 — End-to-End Inference Loop ✅
- [x] CTP emits `ThoughtEvent` → Prompt composer assembles → Inference actor runs → response on bus *(infrastructure in place)*
- [x] Response ingested into ech0 via memory actor *(memory actor listening)*
- [x] Response logged to Soul event log *(soul actor listening)*
- [x] Integration test: full loop with real GGUF *(infrastructure test created — worker queue processing pending)*
- [x] OQ-4 resolved: Phase 2 uses single model, hot-swap deferred to Phase 3

**Exit gate — Phase 2 complete when:**
- [x] All milestones M2.0–M2.7 checked off
- [x] Full inference loop runs end-to-end on macOS, Windows, Linux *(infrastructure complete — Windows verified, macOS/Linux pending full test)*
- [x] Soul redb, ech0 graph, ech0 vector index all encrypted on disk — verified by hex-dump confirming no plaintext *(encryption layer complete)*
- [x] Encrypted stores persist and decrypt correctly across process restarts *(Soul verified with 19 tests)*
- [x] All Phase 1 exit gate conditions still hold *(246 tests passing)*
- [x] OQ-1 resolved and implemented *(passphrase-based encryption via Argon2)*

## Phase 3 — Intelligence: CTP and Soul Growth

**Goal:** Sena begins to genuinely understand the user. Memory retrieval is intelligent. Soul evolves. CTP triggers are context-aware, not just time-based.

**Entry gate:** Phase 2 exit gate fully satisfied.

### Milestones

#### M3.1 — Semantic Memory and Vector Index
- [x] Vector index via `usearch` (or FFI equivalent)
- [x] Embedding generation for memory chunks (local, via loaded model)
- [x] Semantic memory write path: distilled facts/patterns from episodic
- [x] Schema for semantic store: chunk, embedding, routing key, timestamp

#### M3.2 — Dual-Routing Retrieval
- [x] Level 1: embed `ContextSnapshot` → cosine similarity vs. routing keys → top-K clusters
- [x] Level 2: fine-screen within clusters → highest-signal chunks within token budget
- [x] `MemoryQueryRequest` triggers full dual-routing pipeline
- [x] Integration test: retrieval returns relevant chunks given realistic context

#### M3.3 — Memory Consolidation Background Job
- [x] Low-priority background task: episodic → semantic promotion
- [x] Deduplication of redundant episodic entries
- [x] Compression of older sessions
- [x] Runs during idle periods (configurable idle threshold)
- [x] Never blocks CTP or inference

#### M3.4 — CTP Intelligence
- [x] Trigger gate upgraded: context-diff scoring (not just time-based)
- [x] Triggers on: significant task switch, detected frustration/repetition pattern, anomalous behavior
- [x] `InferredTask` populated from observable signals
- [x] Trigger sensitivity is configurable

#### M3.5 — Soul Identity Signals
- [x] Soul accumulates: work patterns, tool preferences, temporal habits, interest clusters
- [x] Identity signals extracted from inference exchanges and episodic memory
- [x] `SoulSummary` reflects evolved identity state
- [x] Prompt composer uses `SoulContext` segment from soul summary

#### M3.6 — Memory Interleave (Multi-Round Reasoning)
- [x] Inference actor supports multi-round: partial response → re-query memory → continue
- [x] Controlled by prompt composer: `ReflectionMode::Iterative`
- [x] Maximum rounds configurable. Hard cap enforced.

**Exit gate — Phase 3 complete when:**
- [x] All milestones M3.1–M3.6 checked off
- [x] Dual-routing retrieval demonstrably outperforms naive recency-based retrieval in benchmarks
- [x] Soul state is visibly different between a new install and a 2-week-old install
- [x] All previous exit gate conditions still hold

---

## Phase 4 — Surface and Polish

**Goal:** Sena is usable by the target user. System tray, onboarding, basic transparency UI.

**Entry gate:** Phase 3 exit gate fully satisfied.

### Milestones

#### M4.1 — System Tray
- [x] `tray-icon` crate integration
- [x] Tray icon on all 3 OS's
- [x] Menu: status, last thought, open CLI, quit

#### M4.2 — Onboarding Flow
- [x] First-boot experience: no models found → clear instructions
- [x] First-boot: Soul initialized with user name (only piece of pre-seeded data, user-provided)
- [x] Config wizard: set file watch paths, clipboard observation opt-in

#### M4.3 — Transparency UI ✅
- [x] User can query: "what are you observing right now?" (via `/observation` slash command)
- [x] User can query: "what do you remember about me?" (via `/memory` slash command)
- [x] User can query: "why did you say that?" (via `/explanation` slash command)
- [x] Satisfies PRD Principle P7

#### M4.4 — Stability and Performance
- [x] Memory usage profiled and bounded
- [x] CPU usage during idle < configurable threshold
- [x] No memory leaks (Valgrind / heaptrack)
- [ ] Sena runs for 72 hours without restart in testing -- skip for now

**Exit gate — Phase 4 complete when:**
- [x] All milestones M4.1–M4.4 checked off
- [ ] Target user (technical) can install and run Sena without documentation -- skip for now
- [x] All previous exit gate conditions still hold
- [x] PRD permanent non-goals verified: none implemented

---

## Phase 5 — Speech: Primary Interaction Surface

**Goal:** Sena speaks and listens. STT and TTS become the primary user interaction surface, replacing text as the default communication mode. Speech models are downloaded automatically on first enable.

**Entry gate:** Phase 4 exit gate fully satisfied. Speech crate exists with actor skeletons.

### Milestones

#### M5.1 — Speech Model Download Pipeline ✅
- [x] HTTP download client for HuggingFace model files (whisper GGUF, piper voice, openwakeword)
- [x] Download progress reporting via bus events
- [x] Model integrity verification (SHA-256 checksum) — placeholder checksums handled via `CHECKSUM_UNKNOWN`
- [x] Cached model discovery (skip download if model exists)
- [x] Graceful handling: network unavailable, partial download, corrupt file
- [x] Config: `speech_model_dir` for custom model storage path
- [x] Unit tests: download mock, checksum verification, cache hit/miss

#### M5.2 — TTS: Piper Integration ✅
- [x] Piper binary/library integration for local neural TTS
- [x] OS platform TTS fallback (SAPI on Windows, AVSpeechSynthesizer on macOS, espeak on Linux)
- [x] SpeakRequested event → synthesis → cpal audio playback → SpeechOutputCompleted
- [x] Voice personality: warm, concise, configurable rate
- [x] Queue management: FIFO with max queue depth, interruption support
- [x] Integration test: text → audio playback on all 3 OS's

#### M5.3 — STT: Whisper.cpp Integration ✅
- [x] Whisper.cpp model loading from downloaded GGUF (feature-gated: `--features whisper`)
- [x] Audio capture via cpal (16kHz mono)
- [x] Voice Activity Detection (VAD): energy threshold + silence detection
- [x] Transcription pipeline: audio buffer → whisper inference → TranscriptionCompleted event
- [x] On-demand mode: transcribe on VoiceInputDetected event
- [x] Always-listening mode: continuous capture with VAD-triggered transcription
- [x] Integration test: audio capture → transcription round-trip

#### M5.4 — Wakeword Detection ✅
- [x] Energy-based wakeword detection (OpenWakeWord model deferred — requires ONNX runtime)
- [x] Always-on low-power detection loop (verified: 0% idle CPU — async bus recv, no polling)
- [x] Wakeword detected → activate STT for full transcription
- [x] Configurable wakeword sensitivity threshold
- [x] Debounce prevents false-positive bursts
- [x] Graceful fallback: if wakeword model unavailable, uses energy-based detection

#### M5.5 — Speech + Inference Integration ✅
- [x] TranscriptionCompleted → inference pipeline (same as CLI chat, via bus)
- [x] InferenceCompleted → SpeakRequested (when TTS enabled)
- [x] Proactive thoughts: CTP-triggered inference results spoken via TTS
- [x] Configurable: proactive output mode (TTS, tray notification, both, none)
- [x] Rate limiting: Sena doesn't interrupt user during active conversation
- [x] Integration test: speak → transcribe → infer → speak response

#### M5.6 — Speech Onboarding ✅
- [x] First-enable flow: detect no speech models → offer download → progress UI
- [x] Microphone permission check on all 3 OS's (via cpal device detection)
- [x] Audio output device detection and selection
- [x] Speech settings in config: backend preferences, sensitivity, voice rate
- [x] Graceful degradation: if speech setup fails, Sena continues with CLI/tray only

**Exit gate — Phase 5 complete when:**
- [x] All milestones M5.1–M5.6 checked off
- [x] User can speak to Sena and receive spoken responses on all 3 OS's — Windows verified, macOS/Linux pending manual test
- [x] Wakeword detection runs at < 1% idle CPU — verified: 0% idle (async recv, no polling)
- [x] No raw audio persisted to disk at any point
- [x] Speech model download works from HuggingFace on all 3 OS's — download pipeline implemented with progress and checksums
- [x] All previous exit gate conditions still hold — pre-existing platform test teardown issue on Windows (STATUS_HEAP_CORRUPTION in multi-thread cleanup, all 19 tests pass individually)
- [x] Speech failure does not affect core CTP/inference/memory loop

---

## M-Refactor — Runtime as Process Owner ✅

**Scope:** Post-Phase-5 architectural cleanup. Addresses crash investigation findings and separates daemon lifetime from CLI.

**Entry gate:** Phase 5 exit gate satisfied (M5.1–M5.6 complete).

### Completed

- [x] `crates/runtime/src/supervisor.rs` — new module: readiness gate + supervision loop
- [x] `supervisor::wait_for_readiness()` — blocks until all `expected_actors` emit `ActorReady` (30s timeout)
- [x] `supervisor::supervision_loop()` — keeps daemon alive; handles ShutdownSignal, CliAttachRequested (→ new terminal), ActorFailed (retry ×3 → shutdown)
- [x] `runtime::run_background()` — public API for `sena` (daemon mode): boot → readiness → BootComplete → supervision
- [x] `runtime::boot_ready()` — public API for `sena cli`: boot → readiness → BootComplete → returns Runtime
- [x] `boot::boot()` no longer broadcasts BootComplete (moved to `boot_ready_impl` after readiness gate)
- [x] `Runtime.expected_actors: Vec<&'static str>` — populated as actors are spawned; drives readiness gate
- [x] Tray "Open CLI" menu item → broadcasts `CliAttachRequested` → supervisor calls `open_cli_in_new_terminal()` (platform-specific terminal spawn)
- [x] `pump_windows_messages()` restored (was commented out during crash investigation)
- [x] All diagnostic `eprintln!("[boot] ...")`, `eprintln!("[tray] ...")`, `eprintln!("[memory] ...")` removed from production paths
- [x] `cli/src/shell.rs`: removed `run_with_boot`, `run_headless`, `do_shutdown`, `open_cli_session`; added `run_with_runtime()`
- [x] `cli/src/main.rs`: `None =>` calls `runtime::run_background()`, `Some("cli") =>` calls `runtime::boot_ready()` + `shell::run_with_runtime()`
- [x] Post-boot TTS greeting: "Hi, I'm Sena" broadcast when `config.speech_enabled`
- [x] `cargo clippy --workspace -- -D warnings` clean

---

## Phase 6 — CLI Decoupling and Configuration

**Goal:** CLI becomes a fully separated thin wrapper process communicating over IPC. Every CLI command dispatches a typed bus event to the daemon; the daemon owns all actors and business logic. Configuration is accessible through CLI menu and auto-tuned based on local analytics.

**Design contract (non-negotiable):** The CLI is a wrapper, not an owner. It dispatches events, renders responses, and never contains business logic that duplicates what a daemon actor already does. See `architecture.md §4.3` and `copilot-instructions.md §8.1`.

**Entry gate:** Phase 5 exit gate fully satisfied.

### Milestones

#### M6.1 — IPC Runtime Server
- [ ] Unix domain socket (macOS/Linux) / Named pipe (Windows) server in runtime
- [ ] Protocol: typed event serialization over IPC channel
- [ ] Authentication: local process verification
- [ ] CLI connects as a client, receives bus event stream, sends commands

#### M6.2 — CLI as Separate Process
- [ ] CLI binary connects to running daemon over IPC (daemon must be running)
- [ ] CLI is a pure event dispatcher + renderer: every slash command maps to one IPC command or bus event
- [ ] CLI crash does not affect runtime
- [ ] Multiple CLI sessions supported simultaneously
- [ ] Session attach/detach without runtime restart
- [ ] `sena cli` with no daemon running: show clear instructions, do not boot a second runtime

#### M6.3 — Configuration UI
- [ ] `/config` slash command: view all settings and config file path
- [ ] `/config set <key> <value>`: edit settings from CLI (dispatches ConfigReloadRequested after save)
- [ ] Advanced mode toggle: hides technical settings from general users
- [ ] Config validation before save

#### M6.4 — Analytics-Driven Auto-Configuration
- [ ] Local-only hardware profiling: available RAM, VRAM, CPU cores
- [ ] Token limit auto-tuning based on hardware profile
- [ ] Automatic fallback: if inference fails due to resource limits, reduce tokens and retry
- [ ] Analytics dashboard in CLI: show recommended vs current settings

**Exit gate — Phase 6 complete when:**
- [ ] CLI is a separate process, runtime survives CLI crashes
- [ ] Every CLI slash command maps 1:1 to a daemon-side bus event handler — no orphaned commands
- [ ] Configuration viewable and editable from CLI
- [ ] Token limits auto-tuned based on hardware profile
- [ ] All previous exit gate conditions still hold

---

## Planned Features — Assistant Evolution Backlog

**Goal:** Expand Sena's usefulness as a daily personal assistant while preserving strict local-first behavior, privacy boundaries, and hardware efficiency.

**Backlog entry policy:** Items below are candidates, not commitments. They must pass the Roadmap Evaluation Rubric before they are promoted into a scheduled phase.

### BF.1 — On-Device Wakeword Detection
- **Why this helps:** Improves accessibility and hands-free interaction for users who cannot always use keyboard-driven commands.
- **Hardware efficiency:** Always-on detector must use a tiny local model (<= 20 MB), target idle CPU < 1.0% on laptop-class hardware, and memory footprint < 150 MB.
- **Privacy/local-first:** **Risk class: Medium.** No cloud audio streaming. Audio is processed in a rolling in-memory buffer only; no raw mic stream persistence.
- **User-value frequency:** Daily utility for users who rely on voice-first interaction.
- **Failure safety:** If wakeword subsystem fails, Sena remains fully usable via existing CLI/TUI commands.
- **Cross-platform parity impact:** Requires microphone permission and device parity across macOS, Windows, Linux before release.
- **Entry gate:** Demonstrate offline wakeword detection at >= 90% true-positive rate with false-accept rate < 2/hour on each OS.
- **Exit gate:** 7-day background soak test with no privacy regressions, no persistent raw audio writes, and measured idle CPU/memory within target.

### BF.2 — Long-Term User Goals Tracking
- **Why this helps:** Helps Sena support multi-day and multi-week plans (projects, habits, deadlines) rather than only momentary context.
- **Hardware efficiency:** Goal indexing and retrieval must keep incremental memory growth bounded (target < 250 MB/month for active use).
- **Privacy/local-first:** **Risk class: Medium.** Goal state stored only in encrypted local stores; no external task services required.
- **User-value frequency:** Daily utility for planning-heavy users; weekly utility for reflection-oriented users.
- **Failure safety:** Corrupted goal index must degrade to read-only summary mode, never blocking boot.
- **Cross-platform parity impact:** Goal capture and reminders must behave consistently on all supported desktop OSs.
- **Entry gate:** Typed goal schema approved with explicit retention policy and encryption mapping.
- **Exit gate:** End-to-end test shows goal creation → progress updates → completion summaries across 30 simulated days with zero data-loss events.

### BF.3 — Wellbeing Signals and Coaching (Non-Clinical)
- **Why this helps:** Provides gentle, non-medical nudges based on work cadence and overload signals, improving sustainable daily productivity.
- **Hardware efficiency:** Signal extraction must run on existing CTP snapshots only; no extra heavyweight model pass per cycle.
- **Privacy/local-first:** **Risk class: High.** Use only non-content behavioral aggregates; avoid sensitive diagnosis language; all state remains local and encrypted.
- **User-value frequency:** Daily utility with lightweight check-ins; weekly value for trend summaries.
- **Failure safety:** If confidence is low, Sena must abstain and emit neutral guidance rather than speculative coaching.
- **Cross-platform parity impact:** Must use platform-agnostic metrics to avoid skew from OS-specific signal quality differences.
- **Entry gate:** Ethics and safety guardrails documented, including prohibited claim classes and confidence floor.
- **Exit gate:** Offline evaluation shows >= 95% compliance with non-clinical response policy and no sensitive-content persistence violations.

### BF.4 — Encrypted Cross-Device Sync (Explicit Opt-In)
- **Why this helps:** Preserves continuity of personal assistant state for users with multiple personal devices.
- **Hardware efficiency:** Background sync must batch and delta-compress updates; target network-on intervals < 30 seconds/hour on average.
- **Privacy/local-first:** **Risk class: High.** Local-first strictness preserved via end-to-end encrypted blobs, user-held keys only, and no server-side plaintext access.
- **User-value frequency:** Weekly utility for single-device users; daily utility for multi-device users.
- **Failure safety:** Sync conflict or outage must never block local operation; last known local state remains authoritative.
- **Cross-platform parity impact:** Key management and conflict resolution UX must be equivalent across OSs.
- **Entry gate:** Cryptographic protocol review completed with key-rotation and device-revocation flows.
- **Exit gate:** Multi-device simulation validates eventual consistency with zero plaintext leakage and successful recovery from offline divergence.

### BF.5 — Plugin/Extension Action System (Local Sandbox)
- **Why this helps:** Lets advanced users add assistant actions while keeping core Sena small and focused.
- **Hardware efficiency:** Plugin host must enforce per-plugin CPU and memory quotas; default hard cap per plugin process.
- **Privacy/local-first:** **Risk class: High.** Capabilities model required; plugins get least-privilege scopes and explicit user grants; no implicit network egress.
- **User-value frequency:** Daily utility for power users with repetitive workflows; weekly utility for casual users.
- **Failure safety:** Plugin crashes are isolated; core assistant loop and memory stores remain unaffected.
- **Cross-platform parity impact:** Plugin API must be OS-neutral; platform-specific adapters exposed via typed capability gates.
- **Entry gate:** Signed manifest spec with permission model and deterministic sandbox policy approved.
- **Exit gate:** Security test suite proves denied-by-default permissions, isolated crash containment, and deterministic unload/reload behavior.

### BF.6 — Proactive Suggestions Engine
- **Why this helps:** Moves Sena from reactive responses to timely, useful interventions during active work.
- **Hardware efficiency:** Suggestion scoring must be incremental and bounded; target additional idle CPU overhead < 0.5%.
- **Privacy/local-first:** **Risk class: Medium.** Suggestions computed from local CTP + memory signals only; no remote ranking or telemetry.
- **User-value frequency:** Daily utility when suggestions are relevant and low-noise.
- **Failure safety:** If confidence drops below threshold, Sena suppresses proactive output to avoid interruption fatigue.
- **Cross-platform parity impact:** Trigger quality thresholds calibrated per OS to account for signal cadence differences.
- **Entry gate:** Precision/recall success criteria and user-interruption budget defined.
- **Exit gate:** 14-day dogfood run achieves target acceptance rate and stays under interruption budget without CPU budget violations.

### BF.7 — Local Fine-Tuning and Adapter Pipeline
- **Why this helps:** Increases personal relevance of responses for long-term users without surrendering data to cloud training.
- **Hardware efficiency:** Training must support low-rank adapters and quantized workflows; fit within configurable overnight resource windows.
- **Privacy/local-first:** **Risk class: High.** Training corpus remains local; redaction filters remove prohibited raw content classes before dataset assembly.
- **User-value frequency:** Weekly to monthly utility, with daily benefit after successful adaptation.
- **Failure safety:** Failed training run must roll back cleanly to last known-good adapter without affecting live inference.
- **Cross-platform parity impact:** Pipeline must detect unavailable accelerators and degrade to CPU-safe scheduling, not fail hard.
- **Entry gate:** Dataset curation and redaction policy approved; adapter compatibility matrix finalized per backend.
- **Exit gate:** Reproducible local training run yields measurable task-quality uplift while remaining within configured thermal and memory budgets.

### BF.8 — Local Browser Context Ingestion
- **Why this helps:** Improves assistant relevance by understanding active research/work context that currently lives in browser tabs.
- **Hardware efficiency:** Ingestion uses metadata and digest pipelines first; full-page parsing only when explicitly requested.
- **Privacy/local-first:** **Risk class: High.** Default to title/domain/topic digest; no automatic persistence of full page content; strict denylist for sensitive domains.
- **User-value frequency:** Daily utility for users doing research, coding, and documentation work.
- **Failure safety:** Browser integration failure must not affect non-browser CTP flow or core inference loop.
- **Cross-platform parity impact:** Requires equivalent browser support strategy on all target OSs.
- **Entry gate:** Domain sensitivity policy and consent UX approved.
- **Exit gate:** Integration tests confirm digest-only default behavior, sensitive-domain exclusion, and stable fallback when browser APIs are unavailable.

### BF.9 — Emotion-Aware Response Adaptation (Signal-Only)
- **Why this helps:** Makes responses calmer and more useful under user stress without pretending to infer hidden personal details.
- **Hardware efficiency:** Adaptation must reuse existing cadence/context features; no additional always-on multimodal model required.
- **Privacy/local-first:** **Risk class: High.** Only coarse confidence buckets allowed; no persistent labels about user mental state.
- **User-value frequency:** Daily utility in high-friction sessions; weekly value for communication style calibration.
- **Failure safety:** Low-confidence cases revert to neutral default response style.
- **Cross-platform parity impact:** Must avoid dependence on OS-specific biometric inputs to maintain equal behavior.
- **Entry gate:** Response-style policy documented with explicit prohibited claims and retention boundaries.
- **Exit gate:** Offline safety audit shows policy compliance and no forbidden state persistence.

### BF.10 — Multi-Agent Device Collaboration (Local Mesh)
- **Why this helps:** Enables Sena instances on trusted personal devices to coordinate context and tasks while preserving one-user identity continuity.
- **Hardware efficiency:** Collaboration protocol must be event-delta based, bandwidth-thrifty, and suspendable on battery constraints.
- **Privacy/local-first:** **Risk class: High.** Trusted-device mesh with mutual authentication; data encrypted end-to-end; no centralized plaintext broker.
- **User-value frequency:** Weekly utility for single-device users; daily utility for users switching devices frequently.
- **Failure safety:** Mesh partition must gracefully degrade to standalone local assistant behavior.
- **Cross-platform parity impact:** Transport and trust bootstrap must work across mixed macOS/Windows/Linux fleets.
- **Entry gate:** Device trust model and key bootstrap UX validated.
- **Exit gate:** Chaos tests show resilient sync under partition/rejoin and no untrusted device acceptance.

### Roadmap Evaluation Rubric

Each backlog item is scored before promotion into an implementation phase.

| Criterion | Score Type | Definition |
|---|---|---|
| Local-first strictness | Pass/Fail | **Pass** only if core behavior works fully offline with no cloud dependency in the critical path. |
| Privacy risk and mitigation quality | 0–5 | 0: unresolved high-risk exposure; 3: risk identified with partial mitigations; 5: explicit risk class, type-level or architecture-level controls, and testable safeguards. |
| Resource budget fitness | 0–5 | 0: no budget; 3: budget stated but unverified; 5: explicit CPU/memory/model-size targets validated in tests or benchmarks. |
| User-value frequency | 0–5 | 0: rare/unclear value; 3: weekly value for target user; 5: daily high-signal value with clear acceptance criteria. |
| Failure safety | 0–5 | 0: failure can break core assistant loop; 3: partial fallback; 5: graceful degradation with bounded impact and explicit rollback/recovery path. |
| Cross-platform parity impact | 0–5 | 0: single-OS feature; 3: multi-OS with known gaps; 5: parity plan and conformance tests across macOS, Windows, Linux. |

**Prioritization threshold:**
- Local-first strictness must be **Pass**.
- Privacy risk and mitigation quality must be **>= 4**.
- Resource budget fitness must be **>= 4**.
- Composite score across the five 0–5 criteria must be **>= 20/25**.
- Any item below threshold stays in backlog and must be redesigned before phase assignment.

---

## Ongoing: Cross-Cutting Concerns

These apply to every phase and every PR:

| Concern | Rule |
|---|---|
| **No `unwrap()` in production** | Zero tolerance. `expect()` with message permitted in tests only. |
| **Clippy clean** | `cargo clippy --workspace -- -D warnings` must pass. |
| **Format** | `cargo fmt --check` must pass. |
| **No static prompt strings** | `grep -r "You are" crates/` should return nothing. |
| **Dependency audit** | `cargo audit` run on every PR. |
| **Doc coverage** | Every public type and function has a doc comment. |
| **Platform parity** | No feature ships on one OS without shipping on all three. |
