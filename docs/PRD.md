# Sena — Product Requirements Document
**Version:** 0.3.0  
**Status:** Living Document — all architectural decisions must be reconciled against this file first  
**Owner:** Core Team

---

## 1. Vision

Sena is a local-first, OS-native personal AI system that runs continuously in the background of a user's computer. It is not a chatbot. It is not a productivity tool. It is an ambient intelligence that observes, understands, reflects, and grows — uniquely calibrated to a single user over time.

Sena's goal is to become an extension of the user's cognition: anticipating needs, surfacing relevant context, and evolving its understanding of the user without ever leaving their machine.

---

## 2. Core Principles

These are non-negotiable. Any feature, subsystem, or implementation decision that violates a principle must be rejected or redesigned.

| # | Principle | What It Means |
|---|---|---|
| P1 | **Local-first, always** | All inference, memory, and processing runs on-device. No user data ever leaves the machine unless the user explicitly opts in to a future cloud feature. **Exception:** Sena may download speech models (Whisper GGUF, Piper voice, OpenWakeWord) from HuggingFace on first enable, and check for model updates periodically. These are controlled, user-consented downloads of model weights only — no user data is transmitted. |
| P2 | **No abstraction tax** | Sena never pays for a layer of abstraction it doesn't control. This is why Ollama is used only for model discovery, not inference. llama-cpp-rs gives us the metal. |
| P3 | **Continuous, not reactive** | Sena is always running, always observing, always building context. It does not wait to be summoned. It is not a command-response system at its core. |
| P4 | **One user, deeply** | Sena is not multi-tenant. It is built for one user on one machine. All personalization, identity, and memory is singular and non-extractable. |
| P5 | **Identity evolves, never resets** | SoulBox accumulates. There is no "clear history." Sena's understanding of a user is treated as irreplaceable state. Deletion is a deliberate, destructive, user-initiated act. |
| P6 | **Fail silent, recover gracefully** | Sena must never crash the host OS or interrupt user work. Any subsystem failure must be isolated, logged, and recovered from without user interruption. |
| P7 | **Earn trust through transparency** | Sena must always be able to tell the user exactly what it is observing, what it remembers, and why it said what it said. No black boxes from the user's perspective. |
| P8 | **Local does not mean unprotected** | Being local-first is a privacy layer, not a security guarantee. All persistent sensitive state (SoulBox, ech0 graph, vector index) must be encrypted at rest. The local boundary is the first line of defense, not the only one. |
| P9 | **Speech-first interaction** | Sena's primary communication surface is speech (STT for input, TTS for output). Text-based interfaces exist for development, debugging, and accessibility. Sena is not a chatbot; it is a listener and speaker. |

---

## 3. What Sena Is

- An **always-on background process** that boots with the OS
- An **observer** of the user's computing environment (active window, clipboard, file events, keystroke patterns — never content)
- A **thinker** that continuously processes context through Continuous Thought Processing (CTP)
- A **memory system** that accumulates episodic and semantic knowledge about the user over time
- A **reasoning engine** powered by locally-run LLMs via llama-cpp-rs
- A **dynamic prompt system** that composes inference inputs at runtime — no static prompts exist anywhere in the codebase
- A **personalization engine** (SoulBox) that stores, evolves, and protects the user's identity model
- A **speech-first ambient interface** that listens via STT and speaks via TTS — the user talks to Sena naturally, and Sena responds vocally with a warm, concise, Soul-driven personality
- A **cross-platform native application** targeting macOS, Windows, and Linux from a single Rust codebase

---

## 4. What Sena Is NOT

This section is as important as section 3. These are hard boundaries.

| NOT | Why |
|---|---|
| **Not a chatbot** | Sena does not exist to answer questions on demand. Conversational interaction is a surface, not the product. |
| **Not a text-first chatbot** | The CLI exists for development and open-source transparency. Speech is the primary surface for general users. |
| **Not a cloud service** | No telemetry, no sync, no remote inference. Ever, by default. |
| **Not an Ollama wrapper** | Ollama is a model store. Sena extracts GGUFs and runs them directly. Ollama's inference server is never started or depended upon at runtime. |
| **Not a plugin system (Phase 1)** | No third-party extensions in Phase 1. The architecture must be stable before it is extensible. |
| **Not a surveillance tool** | Sena observes patterns, never content. Keystrokes are pattern-only (cadence, not characters). Clipboard content is observed for context signals, not stored verbatim. |
| **Not a general-purpose AI assistant** | Sena is not trying to be ChatGPT. It is a personal, ambient system. Sena proactively surfaces insight — it does not wait for prompts. |
| **Not stateless** | Every interaction, every observation, every inference cycle is state. There is no "session." There is only the continuous stream of Sena's experience with this user. |
| **Not multi-user** | One instance of Sena = one user. Running Sena for multiple users on one machine is not a supported use case. |
| **Not a replacement for human connection** | Sena is a cognitive tool. It must never position itself as a social or emotional substitute. |

---

## 5. Users

**Phase 1 Target User:** A technically proficient individual (developer, researcher, power user) who:
- Runs a modern macOS, Windows, or Linux machine
- Has at least one model pulled via Ollama
- Is comfortable running a background process
- Values privacy and local control over convenience

**Phase 1 does NOT target:** Non-technical users, enterprise deployments, mobile, or users without a GPU (CPU inference is supported but performance expectations must be set).

---

## 6. Subsystems Overview

Each subsystem has its own dedicated `docs/subsystems/` document. This section is index-only.

| Subsystem | Crate | Purpose |
|---|---|---|
| **Bus** | `crates/bus` | Typed event bus, actor trait, message channels |
| **Runtime** | `crates/runtime` | Boot sequence, actor registry, shutdown orchestration |
| **Platform** | `crates/platform` | OS adapter trait + per-OS signal collection |
| **CTP** | `crates/ctp` | Continuous Thought Processing — context assembly and thought triggering |
| **Inference** | `crates/inference` | llama-cpp-rs wrapper, model manager, inference queue |
| **Memory** | `crates/memory` | ech0 adapter — translates Sena events into ech0 ingestion/retrieval. ech0 owns graph (redb) + vector (usearch) storage. |
| **Prompt** | `crates/prompt` | Dynamic prompt composition — zero static strings |
| **Soul** | `crates/soul` | SoulBox: identity schema, event log, personalization state |
| **Speech** | `crates/speech` | Local STT (Whisper) and TTS (Piper/platform) — Sena's primary user-facing interaction surface |
| **CLI** | `crates/cli` | Thin binary entrypoint — wires runtime, zero business logic |
| **xtask** | `xtask/` | Build automation, dev tooling, `cargo xtask dump` |

---

## 7. Phases

### Phase 1 — Foundation
**Goal:** A compilable, runnable Sena skeleton with a working bus, actor runtime, and the ability to boot, observe OS signals, and shut down gracefully. No inference, no memory persistence yet.

**Done when:**
- [ ] Workspace compiles clean on macOS, Windows, Linux
- [ ] Bus boots, actors spawn, typed events flow
- [ ] Platform adapter collects: active window, clipboard, file events, keystroke patterns
- [ ] CTP assembles `ContextSnapshot` and emits `ThoughtEvent`
- [ ] Graceful shutdown propagates to all actors
- [ ] `cargo xtask dump` produces a reviewable file diff

### Phase 2 — Inference & Memory
**Goal:** Sena can load a GGUF model, run inference, and persist memory across sessions. All persistent state is encrypted before Phase 2 begins.

**Entry gate (additional):** Encryption design finalized and OQ-SEC resolved. No Phase 2 code writes sensitive state to disk without encryption in place.

**Done when:**
- [ ] Ollama GGUF discovery working on all 3 OS's
- [ ] llama-cpp-rs loading with Metal / CUDA / CPU backend auto-detection
- [ ] Working memory (in-context) functional
- [ ] ech0 integrated: episodic and semantic memory via hybrid graph + vector store
- [ ] ech0 `Embedder` and `Extractor` traits implemented against llama-cpp-rs
- [ ] SoulBox (redb) and ech0 stores encrypted at rest (AES-256-GCM, OS keychain + passphrase fallback)
- [ ] Prompt composer assembles dynamic prompts from live context
- [ ] SoulBox schema initialized on first boot

### Phase 3 — CTP Intelligence & Soul Growth
**Goal:** Sena begins to actually understand the user. Memory dual-routing active. Soul evolving.

**Done when:**
- [ ] Semantic memory with vector index operational
- [ ] Dual-routing retrieval functional
- [ ] SoulBox accumulating user identity signals
- [ ] CTP trigger logic is intelligent (not just time-based)
- [ ] Memory consolidation background job running

### Phase 4 — Surface & Polish
**Goal:** Sena is usable by the target user. System tray, basic UI, onboarding.

**Status:** Complete.

### Phase 5 — Speech: Primary Interaction Surface
**Goal:** Sena speaks and listens. STT and TTS become the primary interaction surface. See `ROADMAP.md` for detailed milestones.

---

## 8. Non-Goals (Permanent)

These will never be in scope regardless of phase:

- Browser extension or web-based interface
- Cloud sync or remote access
- Mobile companion app
- Social/sharing features
- Monetization layer

---

## 9. Open Questions

These are unresolved and must be decided before the relevant phase begins. Do not implement anything that depends on these without first resolving them here.

| # | Question | Blocks | Status |
|---|---|---|---|
| OQ-1 | What is the exact privacy model for clipboard observation? Digest only, or full text in working memory? | Phase 1 | Resolved: Digest only in episodic/semantic memory. Full text permitted in working memory (ephemeral, in-RAM, never persisted). |
| OQ-2 | How does Sena handle first-boot when no Ollama models are present? | Phase 2 | **Resolved (M2.1):** Inference actor emits `ModelDiscoveryFailed` event on bus with a user-actionable reason string. Sena continues in degraded mode (no inference). Error messages: (1) Ollama not installed → "Install from ollama.ai and pull a model." (2) No models pulled → "Run: ollama pull \<model\>." (3) Manifest corrupted → "Re-pull models." |
| OQ-3 | What is the SoulBox deletion UX? Hard delete vs. export-then-delete? | Phase 3 | |
| OQ-4 | Multi-model strategy: one model always loaded, or hot-swap by task type? | Phase 2 | Resolved (M2.7): Phase 2 uses single model. Hot-swap deferred to Phase 3. |
| OQ-5 | What is the minimum VRAM/RAM requirement for a supported experience? | Phase 2 | |
| OQ-SEC | **Encryption:** Which files are encrypted — SoulBox (redb), ech0 graph (redb), ech0 vector index (usearch)? All three, or only SoulBox? What is the re-encryption migration path when a user changes their passphrase? | Phase 2 entry gate | **Resolved (M2.0):** All three stores encrypted. Re-encryption via new DEK, atomic re-encrypt of all files. |
| OQ-6 | ech0 `Embedder` and `Extractor` trait implementations: does the `memory` crate own these, or does `inference` expose them and `memory` consumes them? (Dependency direction must not be violated.) | Phase 2 | Resolved: `memory` crate owns implementations. Calls `inference` via directed mpsc channel for actual embedding/extraction. Per architecture.md §8.3. |

---

## 10. Glossary

| Term | Definition |
|---|---|
| **CTP** | Continuous Thought Processing. Sena's observation and context-assembly loop. |
| **SoulBox** | The identity and personalization engine. Sena-specific, non-extractable. Backed by redb, encrypted at rest. |
| **ThoughtEvent** | A typed event emitted by CTP when context is rich enough to warrant inference. |
| **ContextSnapshot** | A structured, typed capture of the user's current computing context at a moment in time. |
| **GGUF** | The model file format used by llama.cpp and llama-cpp-rs. |
| **Actor** | An isolated async Tokio task that owns its own state and communicates only via typed channels. |
| **Bus** | The central event routing system. Actors subscribe to event types; the bus routes without knowing who's listening. |
| **ech0** | The memory library powering Sena's episodic and semantic memory. Hybrid knowledge graph (redb) + vector index (usearch). Pure Rust, embedded, no network. |
| **Working Memory** | In-context, in-RAM memory. Lives only for the duration of an inference cycle. Not owned by ech0. |
| **Episodic Memory** | Timestamped, session-attributed memory nodes in ech0's graph store. Subject to A-MEM linking, contradiction detection, and importance decay. |
| **Semantic Memory** | Long-term distilled knowledge in ech0's vector index. Retrieved via approximate nearest-neighbor search. |
| **A-MEM** | Adaptive Memory — ech0's background linking pass. Every new ingest triggers dynamic re-linking across the graph. |
| **Dual-Routing** | Sena's retrieval strategy: Level 1 coarse graph traversal via ech0, Level 2 fine ANN vector search via ech0. Inspired by MSA. |
| **Embedder** | ech0 trait. Implemented by Sena's `memory` (or `inference`) crate to generate vector embeddings via llama-cpp-rs. |
| **Extractor** | ech0 trait. Implemented by Sena to extract structured facts from raw text before ingestion. |
| **Envelope Encryption** | Sena's at-rest encryption model. AES-256-GCM data encryption, key stored in OS keychain (primary) or derived from user passphrase via Argon2 (fallback). |
