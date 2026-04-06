# Sena — GitHub Copilot Agent Instructions
# .github/copilot-instructions.md
#
# These instructions govern ALL Copilot-assisted code generation in this repository.
# They are project-specific, non-negotiable, and take precedence over Copilot defaults.
# Read the full file before generating any code.

---

## 0. Before You Write Anything

Read these files first. Do not generate code without understanding them:

1. `docs/PRD.md` — what Sena is and is not
2. `docs/architecture.md` — every structural rule
3. `docs/ROADMAP.md` — what phase we are in and what is in scope

If you are asked to implement something not in the current phase, say so. Do not implement it.

---

## 1. Project Identity

- **Language:** Rust, exclusively. No Python, no shell scripts, no JavaScript.
- **Async runtime:** `tokio` with the `full` feature. No `async-std`. No mixing runtimes.
- **Edition:** Rust 2021.
- **MSRV:** Defined in `rust-toolchain.toml`. Do not use features newer than the pinned toolchain.

---

## 2. Workspace Rules

- The root `Cargo.toml` is a virtual manifest. It has no `[package]`. Never add one.
- All crates live under `crates/`. No exceptions.
- Crate names have no `sena-` prefix. Names are functional: `bus`, `runtime`, `soul`.
- The only binary crate is `crates/cli`. All other crates are `lib`.
- `xtask/` is a standalone Cargo package, not part of the workspace members list that produces production artifacts.
- Never add a dependency to the workspace without confirming it is the correct, maintained crate for the job.

---

## 3. Dependency Rules

Before adding any dependency, ask:

1. Is there a `std` solution that is good enough?
2. Is this crate actively maintained (commit in last 6 months)?
3. Does this crate have a `no_std` option if we might need it later?
4. Does this crate compile on all three target platforms (macOS, Windows, Linux)?

**Approved core dependencies (do not substitute without explicit approval):**

| Purpose | Crate |
|---|---|
| Async runtime | `tokio` |
| Error handling (lib crates) | `thiserror` |
| Error handling (cli only) | `anyhow` |
| Config parsing | `toml` + `serde` |
| Embedded database (Soul) | `redb` |
| Memory graph + vector store | `ech0` (git dependency) |
| OS signals (file watch) | `notify` |
| OS signals (clipboard) | `arboard` |
| OS signals (keystroke timing) | `rdev` |
| Interactive TUI layout (CLI) | `ratatui` |
| System tray (Phase 4) | `tray-icon` |
| LLM inference | `llama-cpp-rs` |
| Async trait | `async-trait` |
| Serialization | `serde` with `derive` feature |
| Encryption | `aes-gcm` |
| Key derivation | `argon2` |
| OS keychain | `keyring` |
| Secure memory zeroing | `zeroize` |
| Random nonce/salt | `rand` |

**Banned:**

| Crate | Reason |
|---|---|
| `reqwest` / any HTTP client | No network calls in Phase 1–3 |
| `openai` / any cloud AI SDK | Violates P1 (local-first) |
| `rusqlite` / `sqlite` / `sqlx` | Replaced by `redb` (Soul) and `ech0` (memory). SQLite is not in this stack. |
| `lazy_static` | Use `std::sync::OnceLock` or `once_cell` |
| `failure` | Superseded by `thiserror` |
| Any crate that calls `process::exit` | Shutdown is the runtime's job |

---

## 4. Code Style Rules

### 4.1 Error Handling

```rust
// CORRECT — typed error in lib crate
#[derive(Debug, thiserror::Error)]
pub enum BusError {
    #[error("channel closed: {0}")]
    ChannelClosed(String),
}

// CORRECT — ? operator propagation
pub fn subscribe(&self, event_type: EventType) -> Result<Receiver, BusError> {
    self.registry.get(event_type).ok_or_else(|| BusError::ChannelClosed(...))
}

// WRONG — never do this in production code
pub fn subscribe(&self, event_type: EventType) -> Receiver {
    self.registry.get(event_type).unwrap() // FORBIDDEN
}

// WRONG — anyhow in a lib crate
use anyhow::Result; // FORBIDDEN outside of crates/cli
```

### 4.2 No `unwrap()` Policy

- `unwrap()` is forbidden in all production code paths.
- `expect("reason")` is permitted in tests only, with a descriptive reason string.
- If you find yourself wanting to `unwrap()`, the function signature is wrong. Return a `Result`.

### 4.3 Async Code

```rust
// CORRECT — blocking work goes to spawn_blocking
pub async fn run_inference(&self, prompt: String) -> Result<String, InferenceError> {
    tokio::task::spawn_blocking(move || {
        // llama-cpp-rs call here — this blocks, and that's fine
    }).await.map_err(|e| InferenceError::TaskPanicked(e.to_string()))?
}

// WRONG — blocking inside async without spawn_blocking
pub async fn run_inference(&self, prompt: String) -> Result<String, InferenceError> {
    std::thread::sleep(Duration::from_secs(10)); // NEVER
    llama_blocking_call(prompt) // NEVER directly in async fn
}
```

### 4.4 No Static Strings in Prompts

```rust
// WRONG — absolutely forbidden
let prompt = "You are Sena, a helpful assistant...".to_string();

// CORRECT — composed from typed segments
let prompt = self.composer.assemble(&[
    PromptSegment::SystemPersona(soul.persona_state()),
    PromptSegment::MemoryContext(memory_chunks),
    PromptSegment::CurrentContext(snapshot),
])?;
```

`grep -r "You are" crates/` should return nothing. Ever.

### 4.5 Actor Communication

```rust
// CORRECT — actor sends event on bus
self.bus.broadcast(Event::Ctp(CTPEvent::ThoughtEventTriggered(thought))).await?;

// WRONG — actor calls another actor's function directly
self.inference_actor.run(prompt).await; // FORBIDDEN. Actors are isolated.
```

### 4.6 Types Over Primitives

```rust
// WRONG — stringly typed
pub struct Event {
    pub kind: String,
    pub data: String,
}

// CORRECT — typed
pub enum PlatformEvent {
    WindowChanged(WindowContext),
    ClipboardChanged(ClipboardDigest),
    FileEvent(FileEvent),
    KeystrokePattern(KeystrokeCadence),
}
```

### 4.7 Privacy-Critical Types

`KeystrokeCadence` must never contain a field of type `char`, `String`, `Vec<char>`, or `Vec<u8>` that could represent character content. If you add such a field, you are introducing a critical privacy violation.

```rust
// CORRECT
pub struct KeystrokeCadence {
    pub events_per_minute: f64,
    pub burst_detected: bool,
    pub idle_duration: Duration,
}

// WRONG — captures character content
pub struct KeystrokeCadence {
    pub keys: Vec<char>, // FORBIDDEN. Privacy violation.
}
```

---

## 5. Module and File Structure

- One concept per file. Do not put `EventBus` and `Actor` in the same file.
- `mod.rs` is forbidden. Use named modules (`bus.rs`, `actor.rs`, `events.rs`).
- Public API surface is minimal. Default to `pub(crate)`. Promote to `pub` only when another crate needs it.
- `events.rs` in `crates/bus` is the **single source of truth** for all event types. No event type is defined anywhere else.

```
crates/bus/src/
├── lib.rs          ← re-exports only. No logic.
├── bus.rs          ← EventBus struct
├── actor.rs        ← Actor trait, ActorError
└── events/
    ├── mod.rs      ← EXCEPTION: mod.rs permitted here as a re-export hub
    ├── system.rs
    ├── platform.rs
    ├── ctp.rs
    ├── inference.rs
    ├── memory.rs
    └── soul.rs
```

---

## 6. Testing Rules

- Every public function has at least one test.
- Tests live in a `#[cfg(test)]` module at the bottom of the file they test, OR in `tests/` for integration.
- No test may write to the user's real config directory, Soul database, or home directory. Use `tempfile::tempdir()`.
- No test may make a network call.
- No test may depend on a real GGUF model file. Inference tests use a fixture minimal model or a mock.
- Test names describe behavior, not implementation: `test_bus_delivers_event_to_subscriber`, not `test_bus_1`.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn bus_delivers_broadcast_to_all_subscribers() {
        // arrange
        let bus = EventBus::new();
        let mut rx1 = bus.subscribe_broadcast();
        let mut rx2 = bus.subscribe_broadcast();

        // act
        bus.broadcast(Event::System(SystemEvent::BootComplete)).await.unwrap();

        // assert
        assert!(rx1.recv().await.is_ok());
        assert!(rx2.recv().await.is_ok());
    }
}
```

---

## 7. Platform-Specific Code

- All platform-specific code lives in `crates/platform`. Nowhere else.
- Platform guards use `#[cfg(target_os = "macos")]`, `#[cfg(target_os = "windows")]`, `#[cfg(target_os = "linux")]`.
- Every platform branch must be covered. No `#[cfg(not(target_os = "windows"))]` that silently covers macOS and Linux as a group.
- If a platform implementation is not yet done, it must be a stub that returns an appropriate error — never a silent no-op.

```rust
// CORRECT — explicit stub
#[cfg(target_os = "linux")]
pub fn active_window(&self) -> Option<WindowContext> {
    // TODO M1.5: implement via x11rb
    None
}

// WRONG — silent no-op that looks like it might work
#[cfg(not(target_os = "windows"))]
pub fn active_window(&self) -> Option<WindowContext> {
    None
}
```

---

## 8. Dependency Direction Enforcement

If you are in crate X and want to import from crate Y, check the dependency graph in `docs/architecture.md §2` first.

**Quick reference — forbidden imports:**

| In crate | May NOT import |
|---|---|
| `bus` | Any other Sena crate |
| `soul` | `ctp`, `inference`, `memory`, `prompt`, `platform` |
| `platform` | `ctp`, `inference`, `memory`, `prompt`, `soul` |
| `cli` | Direct business logic of any kind |
| Any crate | `anyhow` (except `cli`) |

If the architecture graph does not have an arrow for the import you want, the architecture must be revised — not violated. Raise it before writing the code.

---

## 8.1 CLI Design Principle — Wrapper, Not Owner

The CLI is a **thin wrapper** over the daemon's capabilities. It does not own business logic.

**What the CLI IS:**
- A TUI surface for the user to manually trigger daemon operations
- A window into the bus event stream (observation, status, transparency)
- A command dispatcher: sends typed bus events to request work (inference, STT, model swap, config)

**What the CLI is NOT:**
- An owner of actors — CLI never constructs Soul, Memory, Inference, Platform, CTP, or Speech actors
- A re-implementor of business logic — if the daemon already does it, the CLI requests it via bus
- An independent inference pipeline — CLI requests inference by broadcasting events, waits for response events

**Hard rules:**
- Every CLI command maps to exactly one bus event or IPC command dispatched to the daemon. No CLI logic that duplicates what an actor already does.
- When the daemon is running, CLI connects to it (Phase 6 IPC). It does NOT boot a second runtime.
- In CLI-only mode (no daemon), CLI boots the full runtime as the owner — this path is transitional. Target state is always IPC-attached.
- New slash commands added to the CLI must have a corresponding daemon-side event handler. No orphaned CLI commands.
- CLI renders bus events. It does not compute them.

```rust
// CORRECT — CLI dispatches a bus event, waits for response
self.bus.broadcast(Event::Speech(SpeechEvent::TranscribeRequested { ... })).await?;

// WRONG — CLI runs inference itself
let result = llama_model.infer(prompt); // FORBIDDEN — inference belongs to the actor

// WRONG — CLI constructs actors
let actor = InferenceActor::new(...); // FORBIDDEN — runtime owns actor construction
```

---

## 9. SoulBox-Specific Rules

- Never write SQL directly in a file outside of `crates/soul/`.
- Never access Soul's SQLite connection from outside `crates/soul/`.
- All writes to Soul go through Soul's mpsc write channel.
- All reads from Soul come back as typed events on the bus (`SoulSummary`, `SoulEventLogged`).
- Schema changes require a new numbered migration file in `crates/soul/src/schema/migrations/`. No in-place schema modification.

---

## 10. What to Do When You Are Unsure

1. Read `docs/architecture.md` again, specifically the relevant section.
2. If still unsure, implement the minimal thing that satisfies the current milestone and leave a `// TODO: <question>` comment with a specific question.
3. Do NOT make an assumption that results in a cross-crate dependency violation, a privacy type violation, or a static prompt string. These are the three categories of mistake that cause the most damage.
4. Surface the ambiguity in the PR description.

### 10.1 Feature Completeness

All implemented features MUST be fully integrated and plugged in. Coded ≠ integrated.

**Requirements:**
- Events emitted must have handlers that respond to them — every broadcast must have a subscriber
- Every newly emitted event must be traceable to at least one concrete handler path; do not ship fire-and-forget events
- Actors must use the features they implement — no orphaned implementations
- No dead code in production paths — if it's coded, it must be reachable
- After implementing any feature, verify the end-to-end flow works
- Before marking work complete: **trace the code path from trigger to user-visible effect or persistent state change**
- Before marking work complete, write down the trigger -> bus flow -> handler -> observable effect chain for each user-facing change

**Examples of incomplete integration:**
- Adding a `MemoryThresholdExceeded` event but no shell handler displays it to the user
- Implementing a consolidation algorithm but never calling it from the actor run loop
- Creating a prompt segment type that no prompt composer uses
- Broadcasting an event that no actor subscribes to
- Creating a menu item that emits an event but no handler responds to it
- Adding a config field that is never read by any actor

**Completion verification checklist:**
1. Identify the trigger point (user action, timer, bus event)
2. Trace the event flow through the bus
3. Verify handler exists and is subscribed
4. Confirm the handler's output is observable (UI update, log, persistent state change)
5. Test the full path end-to-end before reporting complete

---

## 11. Commit and PR Rules

- Commits are atomic: one logical change per commit.
- Commit messages: `<crate>: <imperative verb> <what>` — e.g. `bus: add broadcast subscription API`
- PRs must reference the milestone they are closing: `Closes M1.3`
- PRs must pass: `cargo build --workspace`, `cargo test --workspace`, `cargo clippy --workspace -- -D warnings`, `cargo fmt --check`
- PRs do not contain code for future phases. Scope is the current milestone only.

### 11.1 Persistence and Milestone Commit Enforcement

- Agent behavior is persistent by default: do not stop at partial implementation.
- Every completed milestone must be committed before moving to the next milestone.
- If a milestone spans multiple logical units, commit each unit as it becomes clean and verified.
- Do not leave completed milestone work only in the working tree.
- Before reporting milestone completion, push all milestone commits to the configured remote.
- If push fails (auth/network/protection), report the exact failure and stop claiming completion until resolved.

---

## 12. ech0 Integration Rules

ech0 is the memory system. The `memory` crate is an adapter over it. These rules are absolute.

- `ech0::Store` is owned exclusively by the memory actor. No other code holds a reference.
- `Embedder` and `Extractor` traits are implemented in `crates/memory` only. Not in `crates/inference`.
- The `Extractor` and `Embedder` implementations call `inference` via directed mpsc channel — they do not import llama-cpp-rs directly.
- `store.ingest_text()` is never called with raw clipboard text or any keystroke data.
- `ConflictResolution::Overwrite` is never called without a preceding Soul log write. This is non-negotiable.
- Working memory (`Vec<MemoryChunk>`) is never passed to `store.ingest_text()`. It is ephemeral and in-RAM only.
- ech0's `_test-helpers` feature is used in tests. Real embedding calls are never made in unit tests.

```rust
// CORRECT — memory actor owns the store
struct MemoryActor {
    store: Store<SenaEmbedder, SenaExtractor>,
    // ...
}

// WRONG — store leaked outside memory actor
pub fn get_store() -> &'static Store<...> { ... } // FORBIDDEN
```

---

## 13. Encryption Rules

All sensitive persistent state is encrypted. These rules have zero tolerance.

- Soul redb, ech0 graph redb, and ech0 vector index are all encrypted. No exceptions.
- Encryption is initialized at boot step 2 (before Soul init at step 3). Any code that opens a store before encryption is initialized is a critical bug.
- Master key and DEK are never written to disk in any form.
- Master key and DEK are never passed to a log macro.
- All key types implement `ZeroizeOnDrop`. No key type uses `#[derive(Debug)]` — they have a custom impl that redacts.
- Nonces are generated fresh per encryption call using `rand`. Never hardcoded, never sequential.
- Passphrase (when used) is `Zeroize`d immediately after Argon2 derivation.

```rust
// CORRECT — key type that cannot leak
#[derive(ZeroizeOnDrop)]
struct MasterKey([u8; 32]);

impl fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "MasterKey([REDACTED])")
    }
}

// WRONG — key that can appear in logs
#[derive(Debug, Clone)]  // FORBIDDEN on key types
struct MasterKey(Vec<u8>);
```

---

## 14. Debugging Protocol — Human-based Inspection

Since the agent cannot interact with Sena directly when debugging runtime behaviour, the following protocol is mandatory for any runtime debugging session:

### 14.1 Before Initiating a Test

1. **Build Sena** with the latest changes: `cargo build --bin sena`
2. **Run Sena** in the appropriate mode (background or CLI)
3. **Send test instructions** to the user specifying:
   - What to do / what input to provide to Sena
   - The expected observable behaviour (output, log lines, no crash, etc.)
   - The exact log file path to share if behaviour is unexpected
4. **Ask a question** using the `vscode_askQuestions` tool (do NOT send another chat message — this keeps the conversation in-session) asking whether the expected behaviour was met. Request the user describe what happened.

### 14.2 Human-based Inspection Question Format

```
Did Sena behave as expected?
Expected: <description>
If not, please paste: <relevant log excerpt or terminal output>
```

### 14.3 Log File Location

- Windows:  `%APPDATA%\sena\sena.YYYY-MM-DD.log`
- macOS:    `~/Library/Application Support/sena/sena.YYYY-MM-DD.log`
- Linux:    `~/.config/sena/sena.YYYY-MM-DD.log`

### 14.4 Dev vs Production Logging

- **Always persisted**: `INFO` level and above, written to the rotating log file regardless of build profile.
- **Dev-only stderr**: In `debug` builds (`cfg!(debug_assertions)`) OR when `SENA_LOG_STDERR=1` is set, logs are also emitted to stderr so they appear in the terminal.
- **Level override**: `SENA_LOG` env var overrides the default level (e.g. `SENA_LOG=debug sena`).
- Keys, passphrases, and DEKs must NEVER appear in any log at any level.

---

## 15. Decision-Making and Autonomy Rules

The agent (Copilot) must follow these meta-rules when making implementation decisions:

### 15.1 Quality First, Then Performance

Implement for correctness and quality first. Optimise for performance second. A correct, well-integrated implementation that is slow is better than a fast, broken one. Performance is a refinement pass, not a design goal.

### 15.2 Choose the Best Option, Not the Easiest

When multiple implementation paths exist:
- **Easiest ≠ best.** A shortcut that creates governance debt is not acceptable.
- **Hardest ≠ best.** Over-engineering a simple concern is equally wrong.
- The right choice depends on the subsystem's complexity requirements. An OS-level personal assistant with a novel CTP system demands high-complexity solutions in some areas and simple ones in others. Use judgment, not defaults.

### 15.3 Ask, Don't Infer

When a decision falls outside the architecture or governance documents, **do not silently infer** what the developer would prefer. Use the `vscode_askQuestions` tool to present:
- The identified gap
- 2–4 concrete options (with trade-offs)
- A recommended default (with reasoning)

This applies to: dependency choices, event routing design, new bus event types, responsibility assignment between crates, and any architectural concern not explicitly covered.

### 15.4 Governance Before Implementation

Before writing code for a new feature or fixing a finding from an audit:
1. Check that the relevant architecture/governance documents match what you're about to implement.
2. If they don't, **update the document first**, then implement. Architecture drift is harder to fix than code bugs.
3. If the document update requires a design decision, ask (per §15.3).

### 15.5 Plug-and-Play Resilience

Every subsystem must degrade gracefully when disabled or when its dependencies are unavailable:
- Turning off speech must not crash CTP, inference, or memory.
- Turning off CTP must not crash speech or inference.
- Disabling any actor must produce a clear log message and a bus event that dependent actors can observe.
- During development, systematically test on/off combinations to discover governance gaps that are invisible in code review.

### 15.6 Autonomy Over Manual

Sena is designed as an autonomous system. Manual CLI interaction is a **development convenience**, not the product's primary mode. Implementation decisions must always favor the autonomous path:
- CTP and proactive inference are the primary loops. CLI commands are secondary.
- Any feature built for CLI must also work (or degrade gracefully) in daemon-only mode.
- "Works in CLI" is not "works." "Works in background daemon without CLI" is "works."

---

## 16. CTP Governance

CTP (Continuous Thought Processing) is Sena's most architecturally novel subsystem. It is not a simple polling loop — it is the observation and reasoning cortex.

### 16.1 Respect CTP's Complexity

CTP is not a cron job. It is a context-aware, multi-signal processing pipeline:
1. **Signal ingestion**: any observable signal from any sensor
2. **Context assembly**: multi-modal snapshot construction
3. **Trigger gating**: intelligent decision on when to think
4. **Thought emission**: structured ThoughtEvent with full context

Every signal type Sena can observe must eventually flow through CTP's signal buffer. CTP is the only subsystem that decides whether Sena should think proactively. Other actors may request inference (e.g., user chat), but proactive thought is CTP's exclusive domain.

### 16.2 CTP Signal Completeness

**If Sena observes it, CTP must know about it.**

This is a general principle, not a finite checklist. The signal types CTP ingests will grow as Sena gains new sensors and observation capabilities. The current implementation status of specific signals is tracked in `docs/architecture.md §6.3`. What matters here is the rule: **no observation may bypass CTP's signal buffer.** A signal that does not reach CTP is a context gap, and context gaps make CTP's trigger decisions less intelligent — which makes Sena less useful.

### 16.3 CTP Is Not Downplayable

In planning and prioritisation, CTP improvements must not be consistently deferred in favor of surface-level features. CTP is the product's core differentiator. A polished CLI with a weak CTP is a chatbot, not an ambient intelligence.

---

## 17. Background Loop Registry

Every Sena background processing loop MUST be registered in the IPC server's loop registry so the CLI can display and toggle it. This is a **mandatory** requirement — unregistered loops are invisible to the user and cannot be controlled.

### 17.1 Loop Registration Rules

- Every background loop in any actor must have a canonical name (lowercase, underscore-separated).
- Every loop must be registered in `crates/runtime/src/ipc_server.rs` with its name, description, and default enabled state.
- Every loop must respond to `SystemEvent::LoopControlRequested { loop_name, enabled }` by pausing or resuming accordingly.
- When a loop's state changes (started or stopped for any reason), the actor must broadcast `SystemEvent::LoopStatusChanged { loop_name, enabled }`.
- When a new loop is created in any actor, its registration in the IPC server loop registry is NOT optional — it must be added in the same commit.

### 17.2 Current Registered Loops

| Loop name | Actor | Default | Description |
|---|---|---|---|
| `ctp` | `CTPActor` | enabled | Continuous thought processing — signal ingestion and proactive inference trigger |
| `memory_consolidation` | `MemoryActor` | enabled | Periodic memory consolidation — moves working memory to long-term store |
| `platform_polling` | `PlatformActor` | enabled | Platform signal polling — active window, clipboard, keystroke cadence |
| `screen_capture` | `PlatformActor` | enabled | Screen capture for vision-capable models — periodic screenshot acquisition |
| `speech` | `SttActor` / `WakewordActor` | enabled | Speech input loop — wakeword detection and/or continuous STT capture |

### 17.3 CLI Display Contract

The CLI sidebar shows all registered loops with a colored status indicator:
- Green dot (●) = loop enabled and running
- Red dot (●) = loop disabled

The `/loops` command lists all loops with their current state.  
The `/loops <name>` command toggles a single loop by name.  
The `/loops <name> on|off` command explicitly enables or disables a loop.

New loops added in Phase 7+ must be added to the table in §17.2 AND to the IPC server registry before the implementing PR is closed.

