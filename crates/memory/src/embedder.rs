//! Embedder trait implementation — calls inference actor for embedding generation.

use std::sync::Arc;
use bus::EventBus;
use crate::ech0_placeholder::Embedder as Ech0Embedder;

/// Embedder that delegates to the inference actor via directed mpsc channel.
///
/// Per architecture.md §8.3: memory crate owns this implementation,
/// calls inference actor for actual embedding computation.
pub struct SenaEmbedder {
    bus: Arc<EventBus>,
    request_id_counter: std::sync::atomic::AtomicU64,
}

impl SenaEmbedder {
    pub fn new(bus: Arc<EventBus>) -> Self {
        Self {
            bus,
            request_id_counter: std::sync::atomic::AtomicU64::new(1),
        }
    }

    fn next_request_id(&self) -> u64 {
        self.request_id_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }
}

impl Ech0Embedder for SenaEmbedder {
    fn embed(&self, _text: &str) -> Result<Vec<f32>, String> {
        // TODO M2.4: Implement actual embedding via inference actor directed channel.
        // For now, return a placeholder embedding vector.
        //
        // Real implementation should:
        // 1. Send EmbedRequested to inference actor via directed channel
        // 2. Await EmbedCompleted response
        // 3. Return the vector
        //
        // This requires async context, but ech0's Embedder trait is sync.
        // Resolution: use tokio::runtime::Handle::current().block_on() or similar.
        
        let _request_id = self.next_request_id();
        
        // Placeholder: return a dummy 384-dimensional vector (common embedding size)
        Ok(vec![0.0; 384])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedder_returns_placeholder_vector() {
        let bus = Arc::new(EventBus::new());
        let embedder = SenaEmbedder::new(bus);
        let result = embedder.embed("test text");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 384);
    }
}
