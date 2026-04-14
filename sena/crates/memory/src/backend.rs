//! Memory backend trait abstraction.

use crate::error::MemoryError;
use bus::events::{MemoryKind, ScoredChunk};
use bus::CausalId;

/// Trait for memory storage backends.
///
/// Implementations provide persistence, indexing, and retrieval of memory chunks.
/// The trait is designed to support multiple backend strategies (in-memory, disk-based,
/// vector-indexed, etc.) without changing the actor interface.
pub trait MemoryBackend: Send {
    /// Ingest a new memory chunk.
    ///
    /// # Arguments
    /// * `text` - The text content to store
    /// * `kind` - The kind of memory being stored
    /// * `causal_id` - The causal ID linking this memory to its source event chain
    ///
    /// # Errors
    /// Returns `MemoryError` if ingestion fails.
    fn ingest(
        &mut self,
        text: &str,
        kind: MemoryKind,
        causal_id: CausalId,
    ) -> Result<(), MemoryError>;

    /// Query the memory store using a semantic embedding.
    ///
    /// # Arguments
    /// * `embedding` - The query embedding vector
    /// * `limit` - Maximum number of results to return
    ///
    /// # Returns
    /// A list of scored memory chunks, ordered by relevance (highest score first).
    ///
    /// # Errors
    /// Returns `MemoryError` if the query fails or embedding is invalid.
    fn query(&self, embedding: &[f32], limit: usize) -> Result<Vec<ScoredChunk>, MemoryError>;
}

/// Stub implementation of MemoryBackend for testing and initial integration.
///
/// This implementation logs operations but does not persist or retrieve any data.
/// It serves as a placeholder until a real backend (e.g., ech0-based) is implemented.
pub struct StubBackend;

impl StubBackend {
    /// Create a new stub backend.
    pub fn new() -> Self {
        Self
    }
}

impl Default for StubBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryBackend for StubBackend {
    fn ingest(
        &mut self,
        text: &str,
        kind: MemoryKind,
        causal_id: CausalId,
    ) -> Result<(), MemoryError> {
        tracing::debug!(
            text_len = text.len(),
            ?kind,
            causal_id = causal_id.as_u64(),
            "stub backend: ingest called"
        );
        Ok(())
    }

    fn query(&self, embedding: &[f32], limit: usize) -> Result<Vec<ScoredChunk>, MemoryError> {
        let has_nonzero = embedding.iter().any(|&x| x != 0.0);
        tracing::debug!(
            embedding_len = embedding.len(),
            has_nonzero,
            limit,
            "stub backend: query called"
        );
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_backend_ingest_succeeds() {
        let mut backend = StubBackend::new();
        let result = backend.ingest("test text", MemoryKind::Episodic, CausalId::new());
        assert!(result.is_ok());
    }

    #[test]
    fn stub_backend_query_returns_empty() {
        let backend = StubBackend::new();
        let embedding = vec![0.1, 0.2, 0.3];
        let result = backend.query(&embedding, 10);
        assert!(result.is_ok());
        let chunks = result.unwrap();
        assert!(chunks.is_empty());
    }

    #[test]
    fn stub_backend_query_with_zero_embedding() {
        let backend = StubBackend::new();
        let embedding = vec![0.0, 0.0, 0.0];
        let result = backend.query(&embedding, 10);
        assert!(result.is_ok());
    }
}
