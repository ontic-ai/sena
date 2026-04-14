# PRD Digest
Source: docs/PRD.md (v0.3.0)
Last synced: 2026-04-11

## Current Phase
Phase 7 — Natural Speech: Voice Cloning and Continuous Listening

## Core Principles
| # | Principle | Summary |
|---|---|---|
| P1 | Local-first, always | All processing on-device. No user data leaves machine. Exception: speech model downloads from HuggingFace. |
| P2 | No abstraction tax | Direct control over stack. Ollama for discovery only, not inference. |
| P3 | Continuous, not reactive | Always running/observing/building context. Not a command-response system. |
| P4 | One user, deeply | One user per machine. Non-extractable personalization. |
| P5 | Identity evolves, never resets | SoulBox accumulates. No "clear history." Deletion is deliberate user action. |
| P6 | Fail silent, recover gracefully | Never crash host OS. Isolate failures. Log and recover without interrupting user. |
| P7 | Earn trust through transparency | User can always query what Sena observes, remembers, and reasoning. |
| P8 | Local does not mean unprotected | All persistent state encrypted at rest. |
| P9 | Speech-first interaction | STT/TTS is primary surface. CLI exists for dev/debug/accessibility. |

## Open Questions (OQs)

### Resolved OQs
| # | Question | Resolution |
|---|---|---|
| OQ-1 | Privacy model for clipboard observation? | Digest only in episodic/semantic. Full text in working memory (ephemeral). |
| OQ-2 | First-boot with no Ollama models? | Inference actor emits `ModelDiscoveryFailed` with actionable message. Degraded mode. |
| OQ-3 | SoulBox deletion UX? | Deferred to Phase 6. Export-then-delete approach chosen. |
| OQ-4 | Multi-model strategy? | Phase 2: single model. Hot-swap deferred to Phase 3. |
| OQ-5 | Minimum VRAM/RAM? | CPU: 8GB RAM. GPU: 4GB VRAM. Recommended: 16GB RAM / 8GB VRAM. |
| OQ-SEC | Encryption scope? | All three stores encrypted (Soul redb, ech0 graph, ech0 vector). Re-encryption via new DEK. |
| OQ-6 | ech0 trait implementations? | `memory` owns Embedder/Extractor. Calls `inference` via mpsc. |

### Phase 7 Open Questions
| # | Question | Blocks |
|---|---|---|
| OQ-TTS-7 | StyleTTS2 integration: ONNX vs pyo3? | M7.2 |
| OQ-STT-7 | whisper-rs streaming support? Max chunk size for <800ms latency? | M7.4 |
| OQ-VOICE-7 | Voice embedding encryption and deletion policy? | M7.3 |

## Permanent Non-Goals
These will NEVER be in scope:
- Browser extension or web-based interface
- Cloud sync or remote access
- Mobile companion app
- Social/sharing features
- Monetization layer

## What Sena IS
- Always-on background process that boots with OS
- Observer of user's computing environment
- Thinker with Continuous Thought Processing (CTP)
- Memory system accumulating episodic/semantic knowledge
- Reasoning engine via local LLMs (llama-cpp-rs)
- Dynamic prompt system (zero static prompts)
- Personalization engine (SoulBox)
- Speech-first ambient interface (STT/TTS primary surface)
- Cross-platform native app (macOS, Windows, Linux)

## What Sena is NOT
- Not a chatbot
- Not a text-first interface
- Not a cloud service
- Not an Ollama wrapper
- Not a plugin system (Phase 1)
- Not a surveillance tool
- Not a general-purpose AI assistant
- Not stateless
- Not multi-user
- Not a replacement for human connection

## Target User (Phase 1)
Technically proficient individual (developer, researcher, power user):
- Runs modern macOS, Windows, or Linux
- Has models via Ollama
- Comfortable with background processes
- Values privacy and local control

## Glossary
| Term | Definition |
|---|---|
| CTP | Continuous Thought Processing — observation and context-assembly loop |
| SoulBox | Identity/personalization engine. redb backend, encrypted. |
| ThoughtEvent | Typed event emitted when context warrants inference |
| ContextSnapshot | Structured capture of user's computing context |
| GGUF | Model file format for llama.cpp |
| Actor | Isolated async Tokio task, owns state, bus-only communication |
| Bus | Central event routing system |
| ech0 | Memory library — hybrid graph (redb) + vector (hora) |
| Working Memory | In-context, in-RAM. Inference cycle lifetime only. |
| Episodic Memory | Timestamped nodes in ech0's graph store |
| Semantic Memory | Long-term distilled knowledge in ech0's vector index |
| A-MEM | Adaptive Memory — ech0's background linking pass |
| Dual-Routing | L1 graph traversal + L2 ANN vector search |
| Embedder | ech0 trait for vector embeddings |
| Extractor | ech0 trait for fact extraction |
| Envelope Encryption | AES-256-GCM with OS keychain or Argon2 fallback |
