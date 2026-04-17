//! ech0-based memory backend.
//!
//! BONES stub: the `Echo0Backend` struct satisfies the `MemoryBackend` trait
//! interface without making any real ech0 calls. The full implementation will
//! be wired to an `ech0::Store` once the encryption layer and embedder are
//! integrated.

use crate::backend::MemoryBackend;
use crate::error::MemoryError;
use async_trait::async_trait;
use bus::events::{MemoryKind, ScoredChunk};
use bus::CausalId;
use tracing::{debug, warn};

/// ech0-based memory backend.
///
/// Owns an `ech0::Store` for semantic storage and retrieval.  
/// The BONES implementation is an in-memory stub that accepts ingest calls
/// but does not persist anything.
pub struct Echo0Backend {
    /// In-memory log of ingested chunks for diagnostic purposes.
    log: Vec<String>,
}

impl Echo0Backend {
    /// Create a new (stub) ech0 backend.
    pub fn new() -> Self {
        Self { log: Vec::new() }
    }
}

impl Default for Echo0Backend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MemoryBackend for Echo0Backend {
    async fn ingest(
        &mut self,
        text: &str,
        kind: MemoryKind,
        causal_id: CausalId,
    ) -> Result<(), MemoryError> {
        debug!(
            text_len = text.len(),
            ?kind,
            ?causal_id,
            "Echo0Backend: ingest (stub -- not persisted)"
        );
        self.log.push(text.to_string());
        Ok(())
    }

    async fn query(&self, query: &str, limit: usize) -> Result<Vec<ScoredChunk>, MemoryError> {
        warn!(
            query_len = query.len(),
            limit,
            "Echo0Backend: query (stub -- returns empty results)"
        );
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bus::events::MemoryKind;
    use bus::CausalId;

    #[tokio::test]
    async fn ingest_does_not_error() {
        let mut backend = Echo0Backend::new();
        let result = backend
            .ingest("hello world", MemoryKind::Episodic, CausalId::new())
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn query_returns_empty_vec() {
        let backend = Echo0Backend::new();
        let results = backend.query("anything", 10).await.expect("query failed");
        assert!(results.is_empty());
    }
}
