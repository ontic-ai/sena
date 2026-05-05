//! Memory backend trait abstraction.

use crate::error::MemoryError;
use async_trait::async_trait;
use bus::CausalId;
use bus::events::{MemoryKind, ScoredChunk};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy)]
pub struct MemoryStats {
    pub working_memory_chunks: usize,
    pub long_term_memory_nodes: usize,
}

/// Trait for memory storage backends.
///
/// Implementations provide persistence, indexing, and retrieval of memory chunks.
/// The trait is designed to support multiple backend strategies (in-memory, disk-based,
/// vector-indexed, etc.) without changing the actor interface.
#[async_trait]
pub trait MemoryBackend: Send + Sync {
    /// Ingest a new memory chunk.
    ///
    /// # Arguments
    /// * `text` - The text content to store
    /// * `kind` - The kind of memory being stored
    /// * `causal_id` - The causal ID linking this memory to its source event chain
    ///
    /// # Errors
    /// Returns `MemoryError` if ingestion fails.
    async fn ingest(
        &mut self,
        text: &str,
        kind: MemoryKind,
        causal_id: CausalId,
    ) -> Result<(), MemoryError>;

    /// Query the memory store using a semantic query string.
    ///
    /// # Arguments
    /// * `query` - The query string (will be embedded internally)
    /// * `limit` - Maximum number of results to return
    ///
    /// # Returns
    /// A list of scored memory chunks, ordered by relevance (highest score first).
    ///
    /// # Errors
    /// Returns `MemoryError` if the query fails.
    async fn query(&self, query: &str, limit: usize) -> Result<Vec<ScoredChunk>, MemoryError>;

    /// Return current backend statistics.
    async fn stats(&self) -> Result<MemoryStats, MemoryError>;

    /// Perform periodic background consolidation/maintenance.
    ///
    /// This may include tasks like:
    /// - Decaying importance scores over time
    /// - Pruning low-importance nodes
    /// - Optimizing indices
    ///
    /// # Returns
    /// The number of nodes affected by consolidation operations.
    ///
    /// # Errors
    /// Returns `MemoryError` if consolidation fails.
    async fn consolidate(&mut self) -> Result<usize, MemoryError>;

    /// Export the current persistent memory snapshot to a JSON file.
    async fn export_json(&self, path: PathBuf) -> Result<(), MemoryError>;
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

#[async_trait]
impl MemoryBackend for StubBackend {
    async fn ingest(
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

    async fn query(&self, query: &str, limit: usize) -> Result<Vec<ScoredChunk>, MemoryError> {
        tracing::debug!(query_len = query.len(), limit, "stub backend: query called");
        Ok(Vec::new())
    }

    async fn stats(&self) -> Result<MemoryStats, MemoryError> {
        Ok(MemoryStats {
            working_memory_chunks: 0,
            long_term_memory_nodes: 0,
        })
    }

    async fn consolidate(&mut self) -> Result<usize, MemoryError> {
        tracing::debug!("stub backend: consolidate called");
        // Stub returns 0 nodes affected
        Ok(0)
    }

    async fn export_json(&self, path: PathBuf) -> Result<(), MemoryError> {
        let parent = path
            .parent()
            .ok_or_else(|| MemoryError::BackendError("backup path has no parent".to_string()))?;
        std::fs::create_dir_all(parent)
            .map_err(|e| MemoryError::BackendError(format!("failed to create backup dir: {e}")))?;
        std::fs::write(path, "[]")
            .map_err(|e| MemoryError::BackendError(format!("failed to write backup file: {e}")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stub_backend_ingest_succeeds() {
        let mut backend = StubBackend::new();
        let result = backend
            .ingest("test text", MemoryKind::Episodic, CausalId::new())
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn stub_backend_query_returns_empty() {
        let backend = StubBackend::new();
        let result = backend.query("test query", 10).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }
}
