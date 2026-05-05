//! Sena vector embedder — implements `ech0::Embedder`.
//!
//! Embeddings are requested from the inference actor through a directed mpsc
//! channel and returned over a oneshot response.

use async_trait::async_trait;
use ech0::error::{EchoError, ErrorCode};
use ech0::traits::Embedder;
use inference::EmbedRequest;
use tokio::sync::{mpsc, oneshot};
use tracing::debug;

/// Dimensionality of Sena's embedding vectors.
///
/// Must match `StoreConfig.store.vector_dimensions` in the ech0 configuration.
/// Using 384 as a default (compatible with small nomic-embed models).
pub const EMBEDDING_DIMENSIONS: usize = 384;

/// Sena's embedding implementation.
///
/// Delegates to the InferenceActor for real embeddings over a directed channel.
#[derive(Clone)]
pub struct SenaEmbedder {
    embed_tx: mpsc::Sender<EmbedRequest>,
}

impl SenaEmbedder {
    /// Create a new Sena embedder.
    pub fn new(embed_tx: mpsc::Sender<EmbedRequest>) -> Self {
        Self { embed_tx }
    }

    /// Create an embedder whose channel is disconnected.
    ///
    /// This is only suitable for tests or temporary fallback wiring. Calls to
    /// `embed()` will fail instead of returning a fake vector.
    pub fn disconnected() -> Self {
        let (embed_tx, embed_rx) = mpsc::channel(1);
        drop(embed_rx);
        Self { embed_tx }
    }
}

impl Default for SenaEmbedder {
    fn default() -> Self {
        Self::disconnected()
    }
}

#[async_trait]
impl Embedder for SenaEmbedder {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EchoError> {
        debug!(text_len = text.len(), "SenaEmbedder: embed request queued");

        if text.is_empty() {
            return Err(EchoError {
                code: ErrorCode::InvalidInput,
                message: "cannot embed empty text".to_string(),
                context: None,
            });
        }

        let (response_tx, response_rx) = oneshot::channel();
        self.embed_tx
            .send(EmbedRequest {
                text: text.to_string(),
                response_tx,
            })
            .await
            .map_err(|_| EchoError {
                code: ErrorCode::EmbedderFailure,
                message: "embedding channel closed".to_string(),
                context: None,
            })?;

        let vector = response_rx
            .await
            .map_err(|_| EchoError {
                code: ErrorCode::EmbedderFailure,
                message: "embedding response channel closed".to_string(),
                context: None,
            })?
            .map_err(|message| EchoError {
                code: ErrorCode::EmbedderFailure,
                message,
                context: None,
            })?;

        if vector.len() != EMBEDDING_DIMENSIONS {
            return Err(EchoError {
                code: ErrorCode::EmbedderFailure,
                message: format!(
                    "embedding dimension mismatch: expected {}, got {}",
                    EMBEDDING_DIMENSIONS,
                    vector.len()
                ),
                context: None,
            });
        }

        Ok(vector)
    }

    fn dimensions(&self) -> usize {
        EMBEDDING_DIMENSIONS
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::{mpsc, oneshot};

    #[tokio::test]
    async fn embed_returns_correct_dimension() {
        let (embed_tx, mut embed_rx) = mpsc::channel::<EmbedRequest>(1);
        tokio::spawn(async move {
            if let Some(request) = embed_rx.recv().await {
                let _ = request
                    .response_tx
                    .send(Ok(vec![1.0; EMBEDDING_DIMENSIONS]));
            }
        });

        let embedder = SenaEmbedder::new(embed_tx);
        let result = embedder.embed("test text").await.expect("embed failed");
        assert_eq!(result.len(), EMBEDDING_DIMENSIONS);
    }

    #[tokio::test]
    async fn embed_rejects_empty_text() {
        let embedder = SenaEmbedder::disconnected();
        assert!(embedder.embed("").await.is_err());
    }

    #[tokio::test]
    async fn embed_fails_when_channel_is_closed() {
        let embedder = SenaEmbedder::disconnected();
        assert!(embedder.embed("test text").await.is_err());
    }

    #[test]
    fn dimensions_matches_constant() {
        let embedder = SenaEmbedder::disconnected();
        assert_eq!(embedder.dimensions(), EMBEDDING_DIMENSIONS);
    }

    #[test]
    fn embed_request_constructs() {
        let (response_tx, _response_rx) = oneshot::channel();
        let request = EmbedRequest {
            text: "hello".to_string(),
            response_tx,
        };
        assert_eq!(request.text, "hello");
    }
}
