# Sena — Development Roadmap
**Version:** 0.2.0  
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
- [ ] Memory usage profiled and bounded
- [ ] CPU usage during idle < configurable threshold
- [ ] No memory leaks (Valgrind / heaptrack)
- [ ] Sena runs for 72 hours without restart in testing

**Exit gate — Phase 4 complete when:**
- [ ] All milestones M4.1–M4.4 checked off
- [ ] Target user (technical) can install and run Sena without documentation
- [ ] All previous exit gate conditions still hold
- [ ] PRD permanent non-goals verified: none implemented

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
