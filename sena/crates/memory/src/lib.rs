//! Memory subsystem: ingestion, consolidation, and retrieval.
//!
//! The Memory subsystem manages:
//! - Memory ingestion from various sources (platform events, inference, Soul)
//! - Semantic indexing and embedding-based retrieval
//! - Working memory to long-term consolidation
//! - Backend abstraction for storage strategies
//!
//! This is a stub implementation that defines the trait abstraction and actor
//! skeleton. Concrete backend implementations (e.g., ech0-based) will follow.

pub mod actor;
pub mod backend;
pub mod error;
pub mod types;

pub use actor::MemoryActor;
pub use backend::{MemoryBackend, StubBackend};
pub use error::MemoryError;
pub use types::{MemoryKind, ScoredChunk};
