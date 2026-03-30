//! Embedder trait implementation — calls inference actor for embedding generation.
//!
//! `SenaEmbedder` implements the real `ech0::Embedder` async trait by routing
//! embedding requests through the event bus to the inference actor.
//!
//! Per architecture.md: memory crate owns this implementation and communicates
//! with inference via bus — never by importing llama-cpp-rs directly.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::Duration;

use async_trait::async_trait;
use bus::{Event, EventBus, InferenceEvent};
use ech0::EchoError;

/// Timeout to wait for an EmbedCompleted response from the inference actor.
const EMBED_TIMEOUT: Duration = Duration::from_secs(30);

/// Fixed embedding dimensionality produced by the current model.
///
/// Must match `StoreConfig.store.vector_dimensions`. Currently set to 384
/// which matches the mock backend used in tests and small local models.
/// This will become configurable when model hot-swap is implemented (Phase 4).
pub const EMBEDDING_DIMENSIONS: usize = 384;

/// Implements `ech0::Embedder` by forwarding embed requests to the inference
/// actor via the event bus directed channel.
///
/// The inference actor runs embed calls inside `spawn_blocking` (backed by
/// llama-cpp-rs) and broadcasts the result as `InferenceEvent::EmbedCompleted`.
pub struct SenaEmbedder {
    bus: Arc<EventBus>,
    counter: AtomicU64,
}

impl SenaEmbedder {
    /// Create a new embedder that communicates via the given bus.
    pub fn new(bus: Arc<EventBus>) -> Self {
        Self {
            bus,
            counter: AtomicU64::new(1),
        }
    }

    fn next_id(&self) -> u64 {
        self.counter.fetch_add(1, Ordering::Relaxed)
    }
}

#[async_trait]
impl ech0::Embedder for SenaEmbedder {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EchoError> {
        let request_id = self.next_id();
        let mut rx = self.bus.subscribe_broadcast();

        self.bus
            .send_directed(
                "inference",
                Event::Inference(InferenceEvent::EmbedRequested {
                    text: text.to_owned(),
                    request_id,
                }),
            )
            .await
            .map_err(|e| EchoError::embedder_failure(format!("bus send failed: {e}")))?;

        let deadline = tokio::time::Instant::now() + EMBED_TIMEOUT;

        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(EchoError::embedder_failure(
                    "embed timeout: no response from inference actor",
                ));
            }

            let recv_fut = rx.recv();
            match tokio::time::timeout(remaining, recv_fut).await {
                Ok(Ok(Event::Inference(InferenceEvent::EmbedCompleted {
                    vector,
                    request_id: rid,
                }))) if rid == request_id => {
                    return Ok(vector);
                }
                Ok(Ok(Event::Inference(InferenceEvent::InferenceFailed {
                    request_id: rid,
                    reason,
                }))) if rid == request_id => {
                    return Err(EchoError::embedder_failure(format!(
                        "inference actor failed: {reason}"
                    )));
                }
                Ok(Ok(_)) => continue,
                Ok(Err(_)) => {
                    return Err(EchoError::embedder_failure("bus channel closed"));
                }
                Err(_) => {
                    return Err(EchoError::embedder_failure(
                        "embed timeout: no response from inference actor",
                    ));
                }
            }
        }
    }

    fn dimensions(&self) -> usize {
        EMBEDDING_DIMENSIONS
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ech0::Embedder;

    #[test]
    fn embedding_dimensions_is_positive() {
        assert!(EMBEDDING_DIMENSIONS > 0);
    }

    #[test]
    fn sena_embedder_reports_correct_dimensions() {
        let bus = Arc::new(EventBus::new());
        let embedder = SenaEmbedder::new(bus);
        // dimensions() must match EMBEDDING_DIMENSIONS
        assert_eq!(embedder.dimensions(), EMBEDDING_DIMENSIONS);
    }
}
