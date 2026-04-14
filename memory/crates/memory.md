# Crate: memory
Path: crates/memory/
Last updated: 2026-04-11
Last commit touching this crate: (see git log)

## Purpose
ech0 adapter crate — translates Sena's typed domain into ech0's API for memory ingestion and retrieval. Implements the Embedder and Extractor traits that ech0 requires. All memory logic (graph traversal, vector search, A-MEM linking, contradiction detection) is owned by ech0.

## Public API Surface
**Types:**
- `MemoryActor` — the memory orchestrator
- `SenaEmbedder` — implements ech0's Embedder trait
- `SenaExtractor` — implements ech0's Extractor trait
- `EncryptedStore` — encrypted ech0 store wrapper
- `MemoryError` — error enum
- `Redacted` — log-safe wrapper
- `WorkingMemory` — in-RAM per-cycle memory
- `InferenceExchange` — prompt/response pair

**Functions:**
- `handle_transparency_query()` — /memory command handler

**Modules:**
- `actor` — MemoryActor implementation
- `embedder` — SenaEmbedder (calls inference via mpsc)
- `extractor` — SenaExtractor (calls inference via mpsc)
- `encrypted_store` — encrypted ech0 wrapper
- `error` — error types
- `redacted` — safe logging
- `transparency_query` — /memory handler
- `working_memory` — ephemeral memory

## Bus Events Owned
Emits (defined in bus):
- `MemoryEvent::MemoryQueryResponse`
- `MemoryEvent::ContextMemoryQueryResponse`
- `MemoryEvent::MemoryConflictDetected`

Subscribes to:
- `InferenceEvent::InferenceCompleted` — ingest responses
- `MemoryEvent::MemoryWriteRequest`
- `MemoryEvent::MemoryQueryRequest`
- `MemoryEvent::ContextMemoryQueryRequest`

## Dependency Edges
Imports from Sena crates: bus, crypto, soul
Imported by Sena crates: runtime
Key external deps:
- ech0 (v0.1.2 git) — memory graph + vector store
- chrono — timestamps
- uuid — identifiers
- serde_json

## Background Loops Owned
- `memory_consolidation` — episodic → semantic promotion, deduplication

## Known Issues
None in production paths

## Notes
**ech0 trait implementations:**
- `SenaEmbedder`: sends `EmbedRequest` to inference → gets `EmbedResponse` with f32 vector
- `SenaExtractor`: sends `ExtractionRequest` to inference → gets facts

**Memory tiers:**
- Working memory: in-RAM, per-inference cycle, never persisted
- Episodic memory: ech0 graph nodes + edges (redb)
- Semantic memory: ech0 vector index (hora)

**Ingest path:**
InferenceResponse → store.ingest_text() → ech0 handles linking/contradiction

**Retrieval path (dual-routing):**
1. Level 1: graph traversal (coarse topic matching)
2. Level 2: ANN vector search (fine ranking)
3. Merge, dedupe, apply token budget

**Context queries:**
- 65% graph / 35% vector weight
- Lower importance threshold (0.10)
- Read-only (never triggers consolidation)

**Hard rules:**
- Raw clipboard text NEVER passed to ingest
- Raw keystroke data NEVER passed to ingest
- Working memory NEVER written to disk
- ConflictResolution::Overwrite NEVER silent (log to Soul first)
