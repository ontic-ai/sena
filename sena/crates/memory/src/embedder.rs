//! Sena vector embedder — implements `ech0::Embedder`.
//!
//! BONES stub: returns a zero vector of the expected dimensionality.
//! The full implementation will route embed requests through the InferenceActor
//! via a directed mpsc channel (per architecture §12).

use async_trait::async_trait;
use ech0::error::{EchoError, ErrorCode};
use ech0::traits::Embedder;
use tracing::debug;

/// Dimensionality of Sena's embedding vectors.
///
/// Must match `StoreConfig.store.vector_dimensions` in the ech0 configuration.
/// Using 384 as a default (compatible with small nomic-embed models).
pub const EMBEDDING_DIMENSIONS: usize = 384;

/// Sena's embedding implementation.
///
/// Delegates to the InferenceActor for real embedding. The BONES stub returns a
/// zero vector — sufficient for structural integration tests but NOT for semantic
/// retrieval.
pub struct SenaEmbedder;

impl SenaEmbedder {
    /// Create a new Sena embedder.
    pub fn new() -> Self {
        Self
    }
}

impl Default for SenaEmbedder {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Embedder for SenaEmbedder {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EchoError> {
        debug!(text_len = text.len(), "SenaEmbedder: embed (stub — zero vector)");

        if text.is_empty() {
            return Err(EchoError {
                code: ErrorCode::InvalidInput,
                message: "cannot embed empty text".to_string(),
                context: None,
            });
        }

        // TODO M3: route to InferenceActor via directed mpsc channel
        Ok(vec![0.0f32; EMBEDDING_DIMENSIONS])
    }

    fn dimensions(&self) -> usize {
        EMBEDDING_DIMENSIONS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn embed_returns_correct_dimension() {
        let embedder = SenaEmbedder::new();
        let result = embedder.embed("test text").await.expect("embed failed");
        assert_eq!(result.len(), EMBEDDING_DIMENSIONS);
    }

    #[tokio::test]
    async fn embed_rejects_empty_text() {
        let embedder = SenaEmbedder::new();
        assert!(embedder.embed("").await.is_err());
    }

    #[test]
    fn dimensions_matches_constant() {
        let embedder = SenaEmbedder::new();
        assert_eq!(embedder.dimensions(), EMBEDDING_DIMENSIONS);
    }
}
