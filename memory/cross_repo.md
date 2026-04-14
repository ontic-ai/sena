# Cross-Repo Boundary: ontic/infer
Last updated: 2026-04-11

## What Sena Depends On
```toml
infer = { git = "https://github.com/ontic-ai/infer", tag = "v0.1.1", features = ["llama"] }
```
- Git dependency, not crates.io
- Tag: v0.1.1
- Feature: `["llama"]` (enables llama-cpp-2 backend)

## Interface Surface
Types/traits imported from ontic/infer:
- `InferenceBackend` (re-exported as `LlmBackend` for backward compat)
- `LlamaBackend` — concrete llama-cpp-2 implementation
- `MockBackend` — for testing
- `BackendType` — enum for backend selection
- `ChatTemplate` — prompt formatting
- `ExtractionResult` — for fact extraction
- `InferError` — error type
- `InferenceParams` — inference configuration
- `MockConfig` — mock backend config
- `ModelRegistry` — model discovery

Usage in Sena:
```rust
// crates/inference/src/lib.rs
pub use infer::{
    BackendType, ChatTemplate, ExtractionResult, InferError, InferenceParams, MockBackend,
    MockConfig, ModelRegistry,
};
pub use infer::InferenceBackend as LlmBackend;
pub use infer::LlamaBackend;
```

## Dependencies via infer
- `llama-cpp-2` (transitively brings `llama_cpp_sys_2` which links ggml)
- `serde`, `thiserror`

## Known Pending Changes
No explicit TODO referencing infer API changes found in crates/inference/

## Last Interface Change
Commit a302d5e — cli: add WakewordSuppressed/Resumed verbose handlers (§10.1 compliance)
(This commit didn't change infer interface)

Prior infer-related commits:
- Phase 5.5.1 changed from direct llama-cpp-2 to infer crate
- M5.5.1 removed crates/inference/src/backend.rs, llama_backend.rs, mock_backend.rs, chat_template.rs, manifest.rs

## Critical Issue
**GGML Symbol Conflict:**
- `infer` → `llama-cpp-2` → `llama_cpp_sys_2` → links `ggml.c`
- `speech` → `whisper-rs` → `whisper_rs_sys` → links `ggml.c`
- Both libraries compile their own copy of ggml.c with conflicting symbols

This causes linker errors when building tests that depend on both crates.

# Cross-Repo Boundary: kura120/ech0
Last updated: 2026-04-11

## What Sena Depends On
```toml
ech0 = { git = "https://github.com/kura120/ech0", tag = "v0.1.2" }
```
- Git dependency, not crates.io
- Tag: v0.1.2
- Full features enabled

## Interface Surface
The `memory` crate implements ech0's traits:
- `Embedder` trait — `SenaEmbedder` in crates/memory/src/embedder.rs
- `Extractor` trait — `SenaExtractor` in crates/memory/src/extractor.rs

Key types:
- `Store` — main interface for memory operations
- `SearchOptions` — query configuration
- `IngestResult` — ingest operation results
- `ConflictReport` — contradiction detection
- `Node`, `Edge` — graph types
- `ScoredNode` — search results

## Storage Files
- ech0 graph: redb database, encrypted via crypto crate
- ech0 vector index: hora index, encrypted via crypto crate

## Known Pending Changes
None found
