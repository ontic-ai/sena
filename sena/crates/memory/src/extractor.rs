//! Sena graph extractor — implements `ech0::Extractor`.
//!
//! BONES stub: returns an empty extraction result.
//! The full implementation will route extract requests through the InferenceActor
//! via a directed mpsc channel (per architecture §12).

use async_trait::async_trait;
use ech0::error::{EchoError, ErrorCode};
use ech0::traits::{ExtractionResult, Extractor};
use tracing::debug;

/// Sena's knowledge graph extraction implementation.
///
/// Delegates to the InferenceActor for real NLP-based extraction. The BONES stub
/// returns an empty result — sufficient for structural integration tests but NOT
/// for building a useful knowledge graph.
pub struct SenaExtractor;

impl SenaExtractor {
    /// Create a new Sena extractor.
    pub fn new() -> Self {
        Self
    }
}

impl Default for SenaExtractor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Extractor for SenaExtractor {
    async fn extract(&self, text: &str) -> Result<ExtractionResult, EchoError> {
        debug!(
            text_len = text.len(),
            "SenaExtractor: extract (stub — empty graph fragment)"
        );

        if text.is_empty() {
            return Err(EchoError {
                code: ErrorCode::InvalidInput,
                message: "cannot extract from empty text".to_string(),
                context: None,
            });
        }

        // TODO M3: route to InferenceActor via directed mpsc channel
        Ok(ExtractionResult {
            nodes: Vec::new(),
            edges: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn extract_returns_empty_graph() {
        let extractor = SenaExtractor::new();
        let result = extractor
            .extract("Alice works at ACME.")
            .await
            .expect("extract failed");
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn extract_rejects_empty_text() {
        let extractor = SenaExtractor::new();
        assert!(extractor.extract("").await.is_err());
    }
}
