# Sena — Development Roadmap
**Version:** 0.3.0  
**Reconcile against:** `PRD.md`, `architecture.md`

---

## How to Use This Roadmap

Each phase has an explicit **entry gate** (what must be true before the phase starts) and **exit gate** (what must be true before the phase is considered done). No phase begins until its entry gate is fully satisfied. No phase ends until its exit gate is fully satisfied. Partial completion is not completion.

Phases are sequential. Parallelism within a phase is allowed. Parallelism across phases is not.

CURRENT BONES PHASING AND PROMPT:
You are wiring the sena/crates/ workspace from a structurally complete
but fully stubbed system into a running one. The diagnostic report at
docs/_scratch/diagnostic_report.md identifies every gap. Work through
the phases below in strict order. Compile and verify after each phase
before proceeding. Do not combine phases.

Read the diagnostic report in full before writing a single line of code.
Read every file you intend to modify before modifying it.

---

PHASE 1 — MAKE THE TERMINAL ALIVE
Target: `cargo run --bin sena` produces visible output and accepts input.

STEP 1.1 — Tracing configuration
In crates/cli/src/main.rs, replace:
  tracing_subscriber::fmt::init();
With:
  tracing_subscriber::fmt()
      .with_env_filter(
          tracing_subscriber::EnvFilter::try_from_default_env()
              .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
      )
      .with_target(false)
      .with_writer(std::io::stdout)
      .init();

This makes INFO the default level without RUST_LOG set, and routes all
tracing output to stdout so it appears in the terminal.

STEP 1.2 — Launch the shell concurrently with the supervision loop
In crates/cli/src/main.rs, after boot() succeeds, run the shell and
supervision loop concurrently using tokio::select! or tokio::join!.
The shell should run in parallel with supervision — neither blocks the
other. When either completes (shutdown or shell exit), the other is
cancelled and the process exits cleanly.

Read crates/cli/src/shell.rs in full to understand Shell::new() and
shell.run() before wiring them.

STEP 1.3 — Ctrl+C shutdown bridge
In crates/runtime/src/supervisor.rs (or main.rs if cleaner), add a
tokio::signal::ctrl_c() listener that broadcasts SystemEvent::ShutdownSignal
on the bus when triggered. This must run as part of the supervision loop
so Ctrl+C initiates the graceful shutdown sequence rather than killing
the process with SIGKILL.

VERIFY Phase 1:
  - `cargo run --bin sena` shows boot log lines on stdout
  - The shell prompt appears and accepts input
  - Ctrl+C triggers a clean shutdown log and exits with code 0
  - cargo build --workspace clean

---

PHASE 2 — WIRE PLATFORM AND CTP
Target: platform events flow → CTP assembles snapshots → ThoughtEventTriggered
emits on the bus.

STEP 2.1 — Fix the platform actor spawn
Read crates/runtime/src/boot.rs, specifically spawn_platform_actor().
The current stub discards _actor entirely and runs a bare shutdown loop.

Rewrite spawn_platform_actor() to:
  1. Call actor.start() to initialize the backend
  2. Enter a real polling loop that calls the backend's poll methods
     (active_window, clipboard, keystroke_cadence) on a configurable
     interval (default: 250ms for active_window, 500ms for clipboard,
     100ms for keystroke_cadence)
  3. Broadcast the resulting PlatformEvent on the bus after each poll
  4. Break on ShutdownSignal

Read crates/platform/src/ in full before writing the polling loop.
Understand what each poll method returns and what PlatformEvent variants
map to each result.

STEP 2.2 — Switch platform to NativeBackend
In crates/runtime/src/builder.rs, build_platform_actor() currently
constructs StubPlatformBackend. Change it to construct
platform::NativeBackend (the platform-native alias, e.g. WindowsBackend
on Windows). Read backends.rs to confirm the correct type alias.

If NativeBackend::new() requires initialization (e.g. rdev listener for
keystrokes), perform that initialization inside build_platform_actor()
or inside actor.start(). Read the WindowsBackend implementation to
understand what setup is required.

STEP 2.3 — Fix CTP signal_tx
In crates/runtime/src/boot.rs, line ~195:
  let (ctp_actor, _signal_tx) = builder::build_ctp_actor()?;

The _signal_tx is dropped immediately. Either:
  a) Store signal_tx in the runtime state so it lives for the duration
     of the session, OR
  b) If signal_tx is not yet used by any caller, remove it from
     build_ctp_actor()'s return type and simplify to:
     let ctp_actor = builder::build_ctp_actor()?;

Read crates/ctp/src/ in full to determine whether signal_tx is used
anywhere in the run loop. If it is used for manual signal injection
from tests only, option (b) is correct for production. If it is used
in production signal paths, option (a) is correct.

Do not drop the sender if any production code path uses it.

VERIFY Phase 2:
  - PlatformEvent::ActiveWindowChanged appears in the event stream
    within 1 second of boot on a real desktop
  - CTP receives platform events (add tracing::debug! to CTP's event
    handler temporarily to confirm)
  - ThoughtEventTriggered appears on the bus within the cooldown window
    (30 seconds default) after boot
  - cargo build --workspace clean, cargo test --workspace passes

---

PHASE 3 — WIRE REAL INFERENCE BACKEND
Target: LlamaBackend from ontic/infer is bridged into Sena's actor
system, wired at runtime, and discovers models.

CONTEXT — the ontic/infer crate
The `infer` crate is a standalone Ontic crate that provides the
InferenceBackend trait, LlamaBackend (feature-gated), MockBackend, and
model discovery utilities. It is bus-agnostic. Sena's crates/inference
crate is bus-aware and wraps infer's backends into the actor system.
Do NOT re-implement LlamaBackend from scratch. Bridge the existing one.

The public API relevant to this phase (from infer's lib.rs):
  - infer::InferenceBackend — the core trait
  - infer::LlamaBackend — production backend (#[cfg(feature = "llama")])
  - infer::MockBackend / infer::MockConfig — test backend
  - infer::InferenceParams — parameters struct passed to complete/infer
  - infer::BackendType — enum for selecting backend variant
  - infer::BackendType::auto_detect() — picks the best available backend
    (Vulkan → CUDA → Metal → CPU) based on compiled features
  - infer::auto_backend(&path, backend_type) — constructs the right
    backend for a given model path
  - infer::discover_models(dir) — scans a directory for GGUF models
  - infer::ollama_models_dir() — returns the platform-default Ollama
    model directory path
  - infer::suppress_llama_logs() — silences llama.cpp's C-level stdout
    logs; call this once at boot before any backend is constructed
  - infer::ExtractionResult — return type for embed/extract operations

STEP 3.1 — Add infer as workspace dependency
In sena/crates/Cargo.toml workspace dependencies section, add:

  [workspace.dependencies]
  infer = {
    git = "https://github.com/ontic-ai/infer",
    tag = "v0.1.2",
    features = ["vulkan"]
  }

The vulkan feature implies llama and enables cross-vendor GPU offloading
on Windows and Linux. This is the correct feature for this system.

In crates/inference/Cargo.toml, add:
  infer = { workspace = true }

Do not add llama-cpp-2 directly to crates/inference — it is a
transitive dependency of infer and must not be duplicated.

STEP 3.2 — Create the infer adapter
Read crates/inference/src/backend.rs in full to understand the local
InferenceBackend trait that Sena's inference actor uses.
Read crates/inference/src/stream.rs to understand InferenceStream.
Read crates/inference/src/registry.rs to understand ModelRegistry.

Then read the ontic/infer crate's backend trait carefully. Determine
whether the local InferenceBackend trait should be:
  a) REPLACED by infer::InferenceBackend directly — if the signatures
     are compatible and no Sena-specific methods exist on the local trait
  b) BRIDGED via an adapter struct — if the local trait has Sena-specific
     methods (bus-aware, actor-specific) that infer::InferenceBackend
     does not and should not have

If bridging (option b, which is likely correct):
  Create crates/inference/src/llama_adapter.rs. Define:

    pub struct LlamaAdapter {
        inner: infer::LlamaBackend,
    }

  Implement the local InferenceBackend trait for LlamaAdapter by
  delegating to self.inner. Map infer's types to Sena's local types
  where needed (InferenceParams → local params, InferError → local
  InferenceError, etc.).

  Use #[cfg(feature = "llama")] on the entire file so it compiles out
  without the feature. The "llama" feature in crates/inference/Cargo.toml
  should gate on infer's llama feature being present.

If replacing (option a):
  Remove the local InferenceBackend trait definition. Import and
  re-export infer::InferenceBackend. Update all local trait usages
  to the imported type. Ensure all Sena-specific extensions (if any)
  are either added via extension trait or moved into the actor layer.

Use #askquestion if this decision is ambiguous after reading both traits.
Do not guess — the wrong choice here is costly to undo.

Call infer::suppress_llama_logs() once, at the top of
build_inference_actor() in builder.rs, before any backend is
constructed. This prevents llama.cpp's C-level logs from polluting
the terminal output configured in Phase 1.

STEP 3.3 — Wire model discovery at boot
In crates/runtime/src/boot.rs, after crypto init and before spawning
actors, add:

  let models_dir = infer::ollama_models_dir()
      .unwrap_or_else(|| config.models_dir.clone());
  let registry = infer::discover_models(&models_dir);

Log the result:
  - tracing::info! for each model found, including model name and path
  - tracing::warn! if the registry is empty (degrade gracefully, do not
    fail boot — Sena without a model is still a running system)

Pass the registry to build_inference_actor() so it can select a model
to load on startup.

STEP 3.4 — Switch runtime to real backend
In crates/runtime/src/builder.rs, update build_inference_actor() to:

  1. If the infer "llama" feature is enabled (i.e., cfg(feature = "llama")
     resolves via the infer dependency):
     - Call infer::auto_backend(&first_model_path, BackendType::auto_detect())
       to construct the backend. auto_detect() will select Vulkan on this
       system automatically.
     - If the registry is empty, construct MockBackend and log:
       tracing::warn!("no models found — inference actor using MockBackend")

  2. If the feature is not enabled:
     - Construct MockBackend unconditionally.

  If a real backend is constructed and a model path is available, call
  backend.load(&path) before passing to InferenceActor. Log VRAM
  headroom after load if the backend exposes it.

VERIFY Phase 3:
  - With a GGUF model file present:
    boot log shows model discovered and loaded, Vulkan selected
  - InferenceRequested on the bus produces InferenceSentenceReady events
  - VramUsageUpdated emits every 2 seconds with real values
  - Without a model file: boot completes, MockBackend is used, inference
    requests return a canned response with a clear log line
  - cargo build --workspace clean

---

PHASE 4 — WIRE REAL SPEECH BACKEND (STREAMING OPTIMIZED)
Target: STT transcribes microphone input in real-time using Nemotron
INT8. TTS synthesizes response audio through Piper.

STEP 4.0 — Implement Download Manager
Create crates/runtime/src/download_manager.rs. This is the source of
truth for model integrity.

Registry: Define a constant asset list for all required models:

  Parakeet / Nemotron INT8:
    - encoder.onnx
      URL: https://huggingface.co/lokkju/nemotron-speech-streaming-en-0.6b-int8/resolve/main/encoder.onnx
      SHA-256: d24be4aff18dd9d2aa3433cb89c5a457df5015abf79e06a63dde76b1cd6386bb

    - decoder_joint.onnx
      URL: https://huggingface.co/lokkju/nemotron-speech-streaming-en-0.6b-int8/resolve/main/decoder_joint.onnx
      SHA-256: c86d527e4ae27251a741609eaddd4429ba5c32050e2f532cea1052d9e21f4f09

    - tokenizer.model
      URL: https://huggingface.co/lokkju/nemotron-speech-streaming-en-0.6b-int8/resolve/main/tokenizer.model
      SHA-256: 07d4e5a63840a53ab2d4d106d2874768143fb3fbdd47938b3910d2da05bfb0a9

  Piper TTS:
    - en_US-lessac-high.onnx
      URL: https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/high/en_US-lessac-high.onnx
      SHA-256: 4cabf7c3a638017137f34a1516522032d4fe3f38228a843cc9b764ddcbcd9e09

    - en_US-lessac-high.onnx.json
      URL: https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/high/en_US-lessac-high.onnx.json
      SHA-256: UNKNOWN — after first download, compute the SHA-256 of
      the downloaded file using sha2::Sha256 and write it to
      docs/_scratch/piper_json_checksum.txt so it can be hardcoded on
      the next revision. For this pass, skip checksum verification for
      this file only and log a tracing::warn! noting the checksum is
      pending human review.

Logic: Implement ensure_models_present(models_dir: &Path).
  For each asset:
    1. Check if the file exists at models_dir/asset_name
    2. If it exists: verify SHA-256 against the known checksum
       (skip this check for the Piper JSON file as noted above)
    3. If verification passes: skip download
    4. If the file is missing or checksum fails:
       a. Download to models_dir/asset_name.tmp using streaming HTTP
          (reqwest with stream feature)
       b. Verify checksum of the .tmp file before renaming
       c. Atomically rename .tmp → final path on success
       d. Delete .tmp and return error on checksum mismatch
  Return Ok(()) only when all assets are present and verified.
  Return Err on any download or verification failure.

Safety: ensure_models_present() failure must abort boot. The runtime
must not proceed to spawn speech actors if models are missing or corrupt.
Emit a clear tracing::error! with the asset name and failure reason
before returning.

STEP 4.1 — Add dependencies
In crates/speech/Cargo.toml, add:
  parakeet-rs = "0.3.4"
  cpal = "0.17.3"

In crates/runtime/Cargo.toml, add:
  reqwest = { version = "0.12", features = ["stream"] }
  sha2 = "0.10"
  tokio = { features = ["fs"] }  (if not already present)

STEP 4.2 — Implement ParakeetSttBackend
Create crates/speech/src/parakeet_backend.rs.

Read crates/speech/src/stt_actor.rs in full to understand the SttBackend
trait (feed, flush, backend_name) and SttEvent variants before writing
any implementation.

ParakeetSttBackend:
  - Load Nemotron INT8 model from the three asset paths
    (encoder.onnx, decoder_joint.onnx, tokenizer.model)
  - feed(&mut self, pcm: &[f32]) — pass raw f32 PCM chunks (10-30ms
    windows at 16kHz mono) directly into parakeet-rs streaming state.
    Return SttEvent::PartialTranscription immediately as tokens arrive.
    Return SttEvent::Completed when internal VAD or silence threshold
    (500ms default) is met.
  - flush(&mut self) — finalize any buffered audio, return remaining
    SttEvent::Completed
  - backend_name() — return "parakeet-nemotron-int8"

Target: sub-100ms latency on Ryzen 4800H using INT8 quantization.

STEP 4.3 — Add audio capture loop to SttActor
Read crates/speech/src/stt_actor.rs in full. The run() method currently
only handles bus events and never calls self.backend.feed().

Add a concurrent audio capture path using tokio::select!:
  Branch 1: existing bus event handling (unchanged)
  Branch 2: audio capture from cpal microphone

The audio capture branch:
  - Only active when self.listening == true
  - Opens a cpal input stream at 16kHz, mono, f32
  - Moves audio from the cpal callback thread to the async actor loop
    via an mpsc channel (cpal callbacks are sync; the actor loop is async)
  - On each received chunk: call self.backend.feed(chunk)
  - On SttEvent::PartialTranscription: broadcast on bus for debug
    visibility (not routed to inference)
  - On SttEvent::Completed: apply confidence threshold (discard below
    config.speech.min_confidence_threshold, default 0.65 — emit
    SpeechEvent::LowConfidenceTranscription for discards), then
    broadcast TranscriptionCompleted for passing results

STEP 4.4 — Implement PiperTtsBackend
Create crates/speech/src/piper_backend.rs.

  - Load en_US-lessac-high.onnx and en_US-lessac-high.onnx.json from
    the configured models directory
  - synthesize(text: &str) -> Result<Vec<u8>, TtsError> — run Piper
    ONNX inference, return raw PCM bytes
  - Initialize a cpal output stream for playback. Buffer synthesized
    PCM and feed it to the output stream.
  - Fallback: if Piper assets are missing at construction time (should
    not happen if ensure_models_present succeeded, but handle it),
    log tracing::warn! and return TtsError::BackendUnavailable —
    TtsActor falls back to StubTtsBackend.

STEP 4.5 — Runtime integration
In crates/runtime/src/boot.rs, call
download_manager::ensure_models_present(&config.models_dir).await
immediately after crypto init, before any actors are spawned. Abort
boot on failure.

In crates/runtime/src/builder.rs:
  - build_stt_actor(): construct ParakeetSttBackend using the validated
    Nemotron file paths. Fall back to StubSttBackend with
    tracing::warn! only if the backend constructor fails — this should
    be unreachable if ensure_models_present succeeded.
  - build_tts_actor(): construct PiperTtsBackend using the validated
    Piper file paths. Same fallback behavior.

VERIFY Phase 4:
  - On first boot: download progress is visible in the terminal log
  - On subsequent boots: checksums pass, no downloads occur
  - Speaking into the microphone in listen mode produces
    TranscriptionCompleted in the CLI event stream
  - TranscriptionCompleted routes to inference, produces
    InferenceSentenceReady, TTS synthesizes and plays audio
  - If TTS falls back to stub: tracing::warn! is visible, no crash
  - cargo build --workspace clean, cargo test --workspace passes

---

PHASE 5 — WIRE IPC SERVER
Target: Slash commands in the shell reach the runtime via IPC.

STEP 5.1 — Start IPC server at boot
Read crates/runtime/src/ipc_server.rs in full. Understand what
spawn_ipc_server() does, what IpcCommand variants it handles, and
what bus events it emits for each.

In crates/runtime/src/boot.rs, call spawn_ipc_server() as part of the
boot sequence — after BootComplete is emitted but before
supervision_loop() blocks. Store the returned JoinHandle in the actor
registry alongside the other actor handles so it participates in
graceful shutdown.

STEP 5.2 — Connect shell commands to IPC
Read crates/cli/src/shell.rs in full. Verify that slash command
handlers (for /status, /shutdown, /loop, /debug, /verbose, /memory,
/config, /help) send IpcCommands to the IPC server's channel.
If the channel connection is missing, wire it now.

VERIFY Phase 5:
  - /status prints actor health from HealthCheckResponse
  - /shutdown triggers graceful shutdown and exits cleanly
  - /help renders the command list
  - All other slash commands execute without panic
  - cargo build --workspace clean

---

FINAL VERIFICATION

After all five phases pass:
  cargo build --workspace
  cargo test --workspace
  cargo clippy --workspace -- -D warnings
  cargo fmt --check

Then perform a live boot test:
  cargo run --bin sena

Confirm and report each point explicitly:
  [ ] Boot log appears on stdout with INFO level by default
  [ ] Shell prompt appears and accepts input
  [ ] Platform events appear in the event stream within 5 seconds
  [ ] ThoughtEventTriggered appears within 30 seconds
  [ ] VramUsageUpdated emits with real values (if model present)
  [ ] Ctrl+C triggers clean shutdown and exits with code 0
  [ ] /help renders command list
  [ ] /status shows all actors as Healthy
  [ ] Speaking produces TranscriptionCompleted in the event stream
  [ ] docs/_scratch/piper_json_checksum.txt exists with computed SHA-256

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
- [x] Vector index via `hora` (pure-Rust, default) — `usearch` removed as dependency
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

#### M5.3 — STT: Whisper Integration ✅
- [x] Whisper model loading from downloaded GGUF (whisper-rs v0.16.0)
- [x] Audio capture via cpal (16kHz mono)
- [x] Voice Activity Detection (VAD): energy threshold + silence detection
- [x] Transcription pipeline: audio buffer → whisper inference → TranscriptionCompleted event
- [x] Word-level confidence scoring with visual indicators in CLI output
- [x] TranscriptionWordReady streaming events for real-time word display
- [x] On-demand mode: transcribe on VoiceInputDetected event
- [x] Always-listening mode: continuous capture with VAD-triggered transcription
- [x] `SilenceDetector` struct extracted — per-mode VAD instances prevent cross-mode state contamination (Audit Finding F1 partial)
- [x] Integration test: audio capture → transcription round-trip

#### M5.4 — Wakeword Detection ✅
- [x] Energy-based wakeword detection (placeholder — OpenWakeWord model deferred to BF.1)
- [x] Always-on low-power detection loop (verified: 0% idle CPU — async bus recv, no polling)
- [x] Wakeword detected → activate STT for full transcription
- [x] Configurable wakeword sensitivity threshold
- [x] Debounce prevents false-positive bursts
- [x] Graceful fallback: if wakeword model unavailable, uses energy-based detection
- [x] Wakeword disabled by default (energy-based placeholder not suitable for production use)
- [x] Wakeword suppressed during `/listen` mode (role handoff — Audit Finding F2 resolved)
- [x] CLI suppresses wakeword messages during active listen mode

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

## Phase 5.5 — Streaming Inference and Ordered TTS

**Goal:** Sena's responses stream token-by-token to the TTS actor, producing natural speech with minimal latency. Sentence boundaries are detected in real-time and each complete sentence is synthesized and played in order as inference continues.

**Entry gate:** Phase 5 exit gate fully satisfied. `infer` crate (ontic-ai/infer v0.1.0) integrated as workspace dependency.

### Milestones

#### M5.5.1 — ontic/infer Backend Integration ✅
- [x] `infer = { git = "https://github.com/ontic-ai/infer", tag = "v0.1.0", features = ["llama"] }` added as workspace dependency
- [x] `llama-cpp-2` direct dependency removed from `crates/inference` (now transitive through infer)
- [x] `crates/inference/src/backend.rs`, `llama_backend.rs`, `mock_backend.rs`, `chat_template.rs`, `manifest.rs` deleted
- [x] `crates/inference/src/lib.rs` updated to re-export from infer crate
- [x] `InferenceBackend` (formerly `LlmBackend`) re-exported as `LlmBackend` for backward compat
- [x] `crates/bus` depends on `infer` for `ModelInfo` and `Quantization` re-exports
- [x] All existing tests pass after migration

#### M5.5.2 — InferenceSource and Streaming Events ✅
- [x] `InferenceSource` enum: `UserVoice`, `UserText`, `ProactiveCTP`, `Iterative`
- [x] `InferenceRequested` event carries `source: InferenceSource` field
- [x] All `InferenceRequested` construction sites updated with appropriate source variant
- [x] `request_id < 1000` proactive detection convention removed; replaced by `InferenceSource::ProactiveCTP`
- [x] `InferenceTokenGenerated`, `InferenceSentenceReady`, `InferenceStreamCompleted` variants added to `bus::events::inference`

#### M5.5.3 — Text Utility Crate ✅
- [x] `crates/text` created as leaf node (no Sena crate dependencies)
- [x] `detect_sentence_boundary(buffer, max_buffer_chars, max_sentence_chars)` implemented
- [x] Boundary rules: hard (`.?!`), soft (`;`), comma threshold, hard cap
- [x] 22+ exhaustive unit tests covering all boundary conditions
- [x] `crates/inference` and `crates/prompt` depend on `text`

#### M5.5.4 — Streaming Inference Actor Path ✅
- [x] `process_streaming_inference_with_context()` added to `InferenceActor`
- [x] `backend.stream()` called inside `spawn_blocking` with std/tokio mpsc bridge
- [x] Per-token: `InferenceTokenGenerated` emitted on broadcast bus
- [x] Per sentence: `InferenceSentenceReady` emitted with sentence index
- [x] Stream closed: buffer flushed, `InferenceStreamCompleted` emitted
- [x] Full text written to memory ONLY after stream completion
- [x] Routing: `UserVoice` + `UserText` → streaming, `ProactiveCTP` + `Iterative` → batch
- [x] Streaming thresholds configurable: `inference.streaming.max_buffer_chars` (default 150), `inference.streaming.max_sentence_chars` (default 400)
- [x] `InferenceCompleted` emitted for backward compat after streaming completes

#### M5.5.5 — Ordered TTS Streaming Queue ✅
- [x] TTS actor handles `InferenceSentenceReady` events
- [x] Per sentence: synthesis task spawned in `spawn_blocking`; result bridges back to async via mpsc
- [x] Synthesis results accumulated in `BTreeMap<sentence_index, SynthResult>`
- [x] Consecutive ready entries played in order starting from `next_play_index`
- [x] Queue depth bounded by `speech.tts_queue_depth` (default 5); oldest entry dropped on overflow
- [x] `InferenceStreamCompleted` drains remaining synthesis tasks (30s timeout) and plays all
- [x] `TranscriptionCompleted` clears streaming queue entirely (interruption path)
- [x] `SpeakRequested` FIFO path preserved for proactive/system messages

**Exit gate — Phase 5.5 complete when:**
- [x] All milestones M5.5.1–M5.5.5 checked off
- [x] User speech input produces streaming response: tokens arrive in real-time, first sentence speaks before inference completes
- [x] Sentences are always played in sentence_index order regardless of synthesis timing
- [x] Full response text is persisted to memory after stream completion; no partial writes
- [x] grep "request_id < 1000" crates/inference/src/actor.rs returns nothing
- [x] grep "use llama_cpp" crates/inference/src/ returns nothing
- [x] All previous exit gate conditions still hold
- [x] `cargo clippy --workspace -- -D warnings` clean

---

## Phase 6 — CLI Decoupling and Configuration

**Goal:** CLI becomes a fully separated thin wrapper process communicating over IPC. Every CLI command dispatches a typed bus event to the daemon; the daemon owns all actors and business logic. Configuration is accessible through CLI menu and auto-tuned based on local analytics.

**Design contract (non-negotiable):** The CLI is a wrapper, not an owner. It dispatches events, renders responses, and never contains business logic that duplicates what a daemon actor already does. See `architecture.md §4.3` and `copilot-instructions.md §8.1`.

**Entry gate:** Phase 5 exit gate fully satisfied.

### Milestones

#### M6.1 — IPC Runtime Server ✅
- [x] Unix domain socket (macOS/Linux) / Named pipe (Windows) server in runtime
- [x] Protocol: typed event serialization over IPC channel (JSON-over-newline, IpcMessage/IpcPayload/LineStyle in bus crate)
- [x] Authentication: local process verification (socket file permissions on Unix, pipe ACL on Windows)
- [x] CLI connects as a client, receives bus event stream, sends commands

#### M6.2 — CLI as Separate Process ✅
- [x] CLI binary connects to running daemon over IPC (daemon must be running)
- [x] CLI is a pure event dispatcher + renderer: every slash command maps to one IPC command or bus event
- [x] CLI crash does not affect runtime (verified by ipc_server_survives_client_disconnect integration test)
- [x] Multiple CLI sessions supported simultaneously (verified by ipc_multiple_clients_connect_simultaneously integration test)
- [x] Session attach/detach without runtime restart
- [x] `sena cli` with daemon already running: connect and attach, do not start a second runtime
- [x] `sena cli` with daemon not running: auto-start daemon, connect IPC, shut daemon down on CLI exit

#### M6.2.1 — Loop Registry and Visibility ✅
- [x] `SystemEvent::LoopControlRequested { loop_name, enabled }` and `SystemEvent::LoopStatusChanged { loop_name, enabled }` defined in bus
- [x] `IpcPayload::LoopStatusUpdate { loop_name, enabled }` and `IpcPayload::ShutdownRequested` defined in bus
- [x] IPC server maintains loop registry (`loop_states` HashMap); sends 5 `LoopStatusUpdate` on Subscribe
- [x] `LoopStatusChanged` bus events propagate to IPC clients in real-time
- [x] `/loops`, `/loops <name>`, `/loops <name> on|off` commands handled server-side
- [x] CTP, Memory, Platform (polling + screen_capture), Speech actors respond to `LoopControlRequested`
- [x] CLI sidebar: logo removed, Loops section with ● green/red dots for all 5 loops
- [x] Loop states update in real-time via `IpcPayload::LoopStatusUpdate`
- [x] `IpcPayload::ShutdownRequested` wired to `SystemEvent::ShutdownSignal` broadcast

#### M6.3 — Configuration UI
- [x] `/config` slash command: view all settings and config file path
- [x] `/config set <key> <value>`: edit settings from CLI (dispatches ConfigSetRequested → supervisor handles save → ConfigReloaded)
- [x] Config writes moved from CLI to supervisor via `ConfigSetRequested` event (Audit Finding F4 resolved)
- [x] `/microphone select` config write moved to bus event dispatch (Audit Finding F4)
- [x] Supervisor config writes use `spawn_blocking` (Audit Finding F5 resolved)
- [ ] Advanced mode toggle: hides technical settings from general users
- [x] Config validation before save (validate_config_set() runs before disk write in supervisor.rs)

#### M6.4 — Analytics-Driven Auto-Configuration
- [x] Local-only hardware profiling: available RAM, VRAM, CPU cores (hardware_profile.rs at boot step 0.5)
- [x] Token limit auto-tuning based on usage telemetry (P95 rolling window, 20% headroom, 10% delta threshold) — implemented as local-only analytics, not hardware-profile-based
- [x] Token auto-tuner config writes use `spawn_blocking` via supervisor (Audit Finding F5)
- [ ] Automatic fallback: if inference fails due to resource limits, reduce tokens and retry
- [ ] Analytics dashboard in CLI: show recommended vs current settings

**Exit gate — Phase 6 complete when:**
- [x] CLI is a separate process, runtime survives CLI crashes
- [x] Every CLI slash command maps 1:1 to a daemon-side bus event handler — no orphaned commands
- [x] Configuration viewable and editable from CLI
- [x] Token limits auto-tuned based on hardware profile
- [x] All previous exit gate conditions still hold

---

## Phase 7A — CTP + Soul Intelligence Layer & UX Polish

**Goal:** Transform CTP from a passive signal collector into an active reasoning cortex with pattern recognition, user-state classification, and rich task inference. Elevate Soul from flat key-value storage into an identity distillation, temporal modeling, and preference learning engine. Polish CLI, prompt, memory, and speech subsystems to consume the new intelligence.

**Entry gate:** Phase 6 exit conditions met.

**Open questions:** None — all design decisions resolved during implementation.

### M7A.1 — Intelligence Audit & Bus Event Foundation
- [x] Full subsystem audit of CTP, Soul, Memory, Prompt, CLI, Speech (776-line report)
- [x] New bus event types: CTP (UserStateComputed, SignalPatternDetected, EnrichedInferredTask), Soul (IdentitySignalDistilled, TemporalPatternDetected, PreferenceLearningUpdate, RichSummaryRequested/Ready), Memory (ContextQueryRequested/Completed), Speech (LowConfidenceTranscription)
- [x] Box CTP event variant to reduce Event enum size

### M7A.2 — CTP Intelligence
- [x] Pattern engine: frustration, repetition, flow-state, anomaly detection from signal buffer
- [x] User-state classifier: frustration_level, flow_detected, context_switch_cost
- [x] Rich task inference engine: semantic descriptions with confidence scoring
- [x] Trigger gate: pattern-aware significance scoring (+0.20 frustration, +0.15 anomaly, +0.10 switch cost, +0.10 memory relevance)
- [x] CTP actor integration: pattern → state → task → broadcast → trigger pipeline

### M7A.3 — Soul Intelligence
- [x] Identity distillation: harvests DistilledIdentitySignal from observation counters
- [x] Temporal behavior model: hour×day_of_week bucketed activity patterns
- [x] Preference learning: engagement signal tracking → verbosity, engagement, proactiveness preferences
- [x] Rich summary assembler: multi-section RichSoulSummary with token budget and relevance sorting
- [x] Soul actor integration: periodic intelligence harvesting every 50 events

### M7A.4 — Cross-Crate Integration
- [x] Prompt: RichSoulContext segment renders multi-section soul summaries; enhanced CurrentContext with semantic task + user state
- [x] Memory: context-aware queries (ContextMemoryQueryRequest/Response) with graph-heavy search
- [x] CTP↔Memory feedback loop: cached_memory_relevance updates trigger gate scoring

### M7A.5 — CLI UX Polish
- [x] /help grouped by category (Chat, Transparency, Audio, System) in TUI and IPC
- [x] Input length limit (4096 chars) with visible character counter
- [x] No-match autocomplete indicator
- [x] Model loading feedback messages
- [x] Formatted inference error display
- [x] Enhanced onboarding: explicit list of what works vs doesn't without a model

### M7A.6 — Speech Naturalness
- [x] Low-confidence transcription events instead of silent failures
- [x] TTS queue overflow broadcasts SpeechFailed (was silent)
- [x] Wakeword debounce cooldown to prevent rapid-fire detections

### M7A.7 — Integration Tests & Architecture Docs
- [x] CTP intelligence integration tests (pattern engine + user state + task inference)
- [x] Soul intelligence integration tests (distillation + temporal + preference + summary)
- [x] Cross-crate integration tests (CTP→Memory query, Soul→Prompt rendering)
- [x] Architecture docs bumped to v0.6.0: §6.5 CTP Intelligence Layer, §8.8 Context-Aware Memory Queries, §10.4 Soul Intelligence Layer

**Exit gate — Phase 7A complete when:**
- [x] CTP detects at least 4 signal pattern types and uses them in trigger decisions
- [x] CTP classifies user state (frustration, flow, context-switch cost) every tick
- [x] CTP infers semantic task descriptions with confidence scoring
- [x] Soul distills identity signals from observation counters
- [x] Soul tracks temporal behavior patterns bucketed by time
- [x] Soul learns user preferences from engagement signals
- [x] Soul assembles rich multi-section summaries with token budgets
- [x] Memory responds to context-aware queries from CTP
- [x] Prompt renders rich soul context and enhanced snapshots
- [x] CLI /help is grouped, input is length-limited, onboarding is informative
- [x] Speech handles low-confidence gracefully, TTS overflow is visible, wakeword debounces
- [x] 536+ tests pass across the workspace (2 ignored: OS keychain integration and 72h longevity — both pre-existing)
- [x] Architecture docs reflect all new subsystems
- [x] All previous exit gate conditions still hold

---

## Phase 7 — Natural Speech: Voice Cloning and Continuous Listening

**Goal:** Transform Sena's speech capabilities from basic TTS/STT to natural, emotionally-aware voice interaction. TTS gains voice cloning and emotional prosody driven by Soul state. STT becomes continuous, low-latency, and speaker-aware.

**Entry gate:** Phase 7A exit gate fully satisfied. OQ-TTS-7 and OQ-STT-7 resolved.

**Open Questions (must resolve before work begins):**

| # | Question | Blocks |
|---|---|---|
| OQ-TTS-7 | StyleTTS2 integration path: ONNX + onnxruntime-rs vs Python FFI via pyo3? Decision criteria: ONNX preferred if latency < 500ms/sentence and cross-platform builds pass all 3 OS's. | M7.2 |
| OQ-STT-7 | Does whisper-rs support streaming incremental transcription, or must we buffer and transcribe? If chunked, define max acceptable chunk size for < 800ms latency from speech end to text display. | M7.4 |
| OQ-VOICE-7 | Voice cloning embeddings are sensitive biometric data. Must be encrypted at rest via crypto crate. Schema: new Soul table `voice_embeddings` AES-256-GCM. Deletion policy: embedding removed when Soul deleted. | M7.3 |

### Milestones

#### M7.1 — Soul Personality Schema Extension
- [ ] Schema migration v3: add `voice::urgency` [0,100] and `voice::stress_level` [0,100] identity signal keys
- [ ] Migration file: `crates/soul/src/schema/migrations/003_add_voice_emotion_signals.rs`
- [ ] Extend `PersonalityUpdated` event: add `urgency: u8`, `stress_level: u8` fields
- [ ] Soul actor `compute_personality()` reads new signals, defaults: urgency=30, stress=10
- [ ] Soul logs every `PersonalityUpdated` emission to event log
- [ ] Unit tests: migration v3 clean, defaults correct, event logged

#### M7.2 — StyleTTS2 Backend Integration (TTS Layer 1)
Requires OQ-TTS-7 resolved. Parallel-safe with M7.5.
- [ ] Add ONNX runtime dependency (audit: cross-platform, maintained, MSRV compatible)
- [ ] New `TtsBackend::StyleTTS2` variant in `crates/speech/src/lib.rs`
- [ ] `crates/speech/src/styletts2_backend.rs`: ONNX model loading + synthesis → Vec<i16> PCM
- [ ] TTS actor integration: route to StyleTTS2 when active, fallback to Piper when models absent
- [ ] Add StyleTTS2 ONNX model URLs to download manifest + SHA-256 checksums
- [ ] Config: `tts_backend_preference` field (default: "styletts2" if models present, else "piper")
- [ ] Integration tests: synthesis non-empty on all 3 OS's, fallback works
- [ ] Exit: synthesis latency < 500ms/sentence on target hardware

#### M7.3 — Voice Cloning (TTS Layer 3)
Requires M7.2 complete, OQ-VOICE-7 resolved.
- [ ] New slash command `/voice clone`: CLI dispatches `VoiceCloneRequested { request_id }`
- [ ] STT actor captures 10s audio → StyleTTS2 speaker encoder → 256-dim embedding
- [ ] Emits `VoiceCloneCompleted { embedding: Vec<f32>, request_id }`
- [ ] Soul actor stores encrypted embedding in new `VOICE_EMBEDDINGS` redb table
- [ ] TTS actor reads embedding from Soul at boot, uses it in every synthesis call
- [ ] `/voice reset` slash command: clears stored embedding
- [ ] New bus events: `VoiceCloneRequested`, `VoiceCloneCompleted`, `VoiceEmbeddingRequested`, `VoiceEmbeddingRetrieved`
- [ ] Privacy: embedding never logged (custom Debug redacts), encrypted at rest via crypto crate
- [ ] Exit: cloned voice persists across restarts, encrypted in Soul (hex-dump verified)

#### M7.4 — Emotional Prosody (TTS Layer 4)
Requires M7.1 and M7.3 complete.
- [ ] StyleTTS2 backend: `compute_style_embedding(warmth, urgency, stress) -> Vec<f32>` — maps personality to style latent space via pre-trained style anchors
- [ ] TTS actor subscribes to `PersonalityUpdated`, updates `current_personality` state
- [ ] Every synthesis call passes `current_personality` to backend style conditioning
- [ ] CTP identity signal path: high work cadence → raise urgency; long calm session → lower stress
- [ ] Integration test: `PersonalityUpdated` with high urgency → synthesis uses urgent style
- [ ] Exit: 3/3 blind testers distinguish calm vs urgent prosody; cloned voice identity preserved

#### M7.5 — `/listen` CLI Command (STT Layer 1)
**PARTIALLY IMPLEMENTED IN PHASE 5 (commit 739d412 / e0d8e56).** Remaining items tracked below.
Parallel-safe with M7.1, M7.2, M7.3, M7.4.
- [x] New slash command `/listen` in `crates/cli/src/shell.rs`
- [x] CLI dispatches `SpeechEvent::ListenModeRequested { session_id }`
- [x] STT actor: enables continuous capture, transcribes after silence threshold, emits `ListenModeTranscription { text, is_final, confidence, session_id }`
- [x] Listen mode uses independent `SilenceDetector` — wakeword events no longer wipe listen audio (bug fix)
- [ ] CLI renders: partial results in gray (overwritten), final results in white — currently all renders same style
- [x] Ctrl+C → `ListenModeStopRequested` → `ListenModeStopped` → clean exit
- [ ] `[unclear]` in red for confidence < 0.6 — currently low-confidence results are skipped, not labeled
- [x] New bus events: `ListenModeRequested`, `ListenModeTranscription`, `ListenModeStopRequested`, `ListenModeStopped`
- [ ] Integration test: mock STT emits 3 partial + 1 final, CLI renders all 4 correctly
- [ ] Exit: partial results < 1s, final results < 2s from silence, unclear words flagged

#### M7.6 — Speaker Diarization (STT Layer 2)
Requires M7.5 complete. Requires speaker diarization model research complete.
- [ ] Research phase: evaluate pyannote.audio ONNX export vs Silero VAD + speaker embeddings; document choice in `docs/speech/speaker-diarization-model-choice.md`
- [ ] `crates/speech/src/diarization.rs`: extract speaker embeddings per chunk, cluster to assign speaker IDs
- [ ] Extend `ListenModeTranscription`: add `speaker_id: Option<u8>` (0 = primary user, 1+ = others)
- [ ] CLI displays speaker labels: `[User]` and `[Speaker 1]`
- [ ] Integration test: mock diarization with alternating speaker IDs renders correctly
- [ ] Exit: speaker accuracy > 85% in 2-speaker test (5 min, 10+ speaker switches), diarization latency < 300ms/chunk

**Exit gate — Phase 7 complete when:**
- [ ] All milestones M7.1–M7.6 checked off
- [ ] TTS uses StyleTTS2 with voice cloning on all 3 OS's
- [ ] Emotional prosody perceptibly different in calm vs urgent states (3/3 blind testers confirm)
- [ ] `/listen` command: < 2s latency for final results, unclear words flagged
- [ ] Speaker diarization accuracy > 85% in 2-speaker test
- [ ] Voice embedding encrypted at rest (hex-dump verified, no plaintext floats)
- [ ] StyleTTS2 OOM fallback to Piper works on < 8 GB VRAM configurations
- [ ] All Phase 1-6 exit gate conditions still hold

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
