# Inference Crate — API Surface Audit (Q7.1)

**Status:** Post-Phase-5 architecture audit finding  
**Date:** 2026-04-02

---

## Summary

The `crates/inference` crate has a **split API surface**: a Sena-bus-coupled layer and a
generic, extractable layer. This document maps each public export and classifies it.

---

## Public Exports (`crates/inference/src/lib.rs`)

| Export | Bus-coupled? | Notes |
|--------|-------------|-------|
| `InferenceActor` | **YES** | Imports `bus::` types extensively. Accepts `Arc<EventBus>`, emits typed bus events. Not extractable without removing bus coupling. |
| `InferenceQueue` / `WorkKind` | **YES** | `WorkKind` carries `tokio::sync::oneshot::Sender` for response routing and is integrated with the bus event flow. |
| `LlamaBackend` | No | Wraps llama-cpp-rs. Depends only on `InferenceParams` and `InferenceError`. Fully extractable. |
| `MockBackend` | No | Test double for `LlmBackend`. No bus imports. Fully extractable. |
| `LlmBackend` (trait) | No | Trait with `load_model`, `infer`, `embed`, `extract`. Generic — no Sena coupling. |
| `BackendType` | No | Enum: `Metal`, `Cuda`, `Cpu`. No bus imports. Extractable. |
| `InferenceParams` | No | Plain struct: `max_tokens`, `ctx_size`, `temperature`. No bus imports. Extractable. |
| `InferenceError` | No | `thiserror` enum. No bus coupling. Extractable. |
| `discover_models` | No | Reads Ollama manifest path, returns `ModelRegistry`. No bus coupling. Extractable. |
| `ModelRegistry` | No | List of available GGUF models with metadata. No bus coupling. Extractable. |

---

## Coupling Analysis

### Coupled layer (Sena-specific — NOT extractable)

`InferenceActor` and `InferenceQueue`/`WorkKind` are tightly coupled to `bus::` event
types (`InferenceEvent`, `SoulEvent`, `CTPEvent`, `SpeechEvent`, `MemoryEvent`,
`TransparencyEvent`). Extracting these would require either:

1. Replacing all bus event emission with a generic callback/trait system, OR
2. Keeping `InferenceActor` as a Sena-specific wrapper around a generic engine.

Option 2 is strongly preferred and is the intended long-term architecture.

### Generic layer (fully extractable)

`LlmBackend`, `LlamaBackend`, `MockBackend`, `BackendType`, `InferenceParams`,
`InferenceError`, `discover_models`, `ModelRegistry` have zero coupling to the bus or
any Sena-specific type. These could be extracted into a standalone `llm-engine` crate
with no changes.

---

## Recommended Future Action (Phase 6+)

If `crates/inference` is ever published to crates.io or extracted for reuse outside Sena:

1. Create a new crate `llm-engine` containing the generic layer.
2. `crates/inference` becomes a thin Sena adapter that implements `LlmBackend` via the
   `llm-engine` crate and wires it to the bus via `InferenceActor`.
3. Bus coupling is isolated to the adapter layer, which is not published.

**This is NOT a current-phase action.** The current structure is correct for Phase 1–5.
No refactoring is warranted until an actual extraction use case arises.

---

## Hard Constraint

The `InferenceActor` must never be imported outside of `crates/runtime` (composition root).
The `CTPEvent`, `SpeechEvent`, `SoulEvent`, `MemoryEvent` imports in `inference/actor.rs`
are expected and correct — inference responds to these broadcast events.
