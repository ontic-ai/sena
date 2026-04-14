# Crate: inference
Path: crates/inference/
Last updated: 2026-04-11
Last commit touching this crate: (see git log)

## Purpose
Adapts the external ontic/infer crate to Sena's bus architecture. Provides model discovery, inference queue, streaming, and LLM backend management. Never loads model weights at boot — lazy loading on first request.

## Public API Surface
**Re-exports from infer:**
- `BackendType` — Metal/CUDA/CPU
- `ChatTemplate` — prompt formatting
- `ExtractionResult` — fact extraction output
- `InferError` — inference errors
- `InferenceParams` — generation parameters
- `MockBackend` — testing backend
- `MockConfig` — mock configuration
- `ModelRegistry` — model discovery
- `LlmBackend` (alias for `InferenceBackend`)
- `LlamaBackend` — llama-cpp-2 backend

**Local types:**
- `InferenceActor` — the inference orchestrator
- `InferenceQueue` — bounded priority queue
- `WorkKind` — work type classification
- `InferenceError` — local error type

**Functions:**
- `discover_models()` — scan for GGUF files
- `suppress_llama_logs()` — silence llama.cpp output

**Modules:**
- `actor` — InferenceActor implementation
- `discovery` — model discovery
- `error` — error types
- `queue` — inference queue
- `registry` — model registry utilities
- `identity_signals` (private) — extracts identity signals from responses
- `transparency_query` (private) — /explanation handler

## Bus Events Owned
Emits (defined in bus):
- `InferenceEvent::InferenceCompleted`
- `InferenceEvent::InferenceStatusUpdate`
- `InferenceEvent::InferenceTokenGenerated`
- `InferenceEvent::InferenceSentenceReady`
- `InferenceEvent::InferenceStreamCompleted`

Subscribes to:
- `InferenceEvent::InferenceRequested`
- `MemoryEvent::MemoryQueryResponse`
- `SoulEvent::SoulSummaryReady`
- `PlatformVisionEvent::VisionFrameReady`

## Dependency Edges
Imports from Sena crates: bus, text
Imported by Sena crates: runtime
Key external deps:
- infer (v0.1.1 git) — llama-cpp-2 wrapper
- tokio (spawn_blocking)
- serde_json
- uuid

## Background Loops Owned
None — request-driven only

## Known Issues
- TODO: Phase 7B — switch to RichSummaryRequested
- TODO: M6 — replace with memory::WorkingMemory for token budget

**CRITICAL: GGML conflict with whisper_rs_sys**
- Both llama_cpp_sys_2 and whisper_rs_sys link ggml.c
- Causes LNK2005 errors in Windows tests

## Notes
**Backend selection (auto-detected):**
1. Metal (macOS, Apple Silicon)
2. CUDA (Windows/Linux, NVIDIA)
3. CPU (fallback)

**Streaming vs Batch:**
- `UserVoice`, `UserText` → streaming path
- `ProactiveCTP`, `Iterative` → batch path

**Streaming pipeline:**
1. Per-token: emit `InferenceTokenGenerated`
2. Sentence boundary detected: emit `InferenceSentenceReady`
3. Stream complete: emit `InferenceStreamCompleted`
4. Write full text to memory (NEVER partial)

**Model weights:** loaded on first InferenceRequest, not at boot
