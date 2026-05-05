//! Memory subsystem: ingestion, consolidation, and retrieval.
//!
//! The Memory subsystem manages:
//! - Memory ingestion from various sources (platform events, inference, Soul)
//! - Semantic indexing and embedding-based retrieval via ech0
//! - Working memory to long-term consolidation
//! - Encrypted storage of all persistent memory state
//!
//! This is the BONES rewrite implementation using ech0 as the backend.

pub mod actor;
pub mod backend;
pub mod echo0_backend;
pub mod embedder;
pub mod encrypted_store;
pub mod error;
pub mod extractor;
pub mod types;
pub mod working_memory;

pub use actor::{BackupConfig, MemoryActor};
pub use backend::{MemoryBackend, MemoryStats, StubBackend};
pub use echo0_backend::{Echo0Backend, PersistentMemoryStore};
pub use embedder::{EMBEDDING_DIMENSIONS, SenaEmbedder};
pub use error::MemoryError;
pub use extractor::SenaExtractor;
pub use types::{MemoryKind, ScoredChunk};
pub use working_memory::{WorkingMemory, WorkingMemoryChunk};
