//! Extractor trait implementation — calls inference actor for fact extraction.

use std::sync::Arc;
use bus::EventBus;
use crate::ech0_placeholder::Extractor as Ech0Extractor;

/// Extractor that delegates to the inference actor via directed mpsc channel.
///
/// Per architecture.md §8.3: memory crate owns this implementation,
/// calls inference actor for actual extraction computation.
pub struct SenaExtractor {
    bus: Arc<EventBus>,
    request_id_counter: std::sync::atomic::AtomicU64,
}

impl SenaExtractor {
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

impl Ech0Extractor for SenaExtractor {
    fn extract(&self, _text: &str) -> Result<Vec<String>, String> {
        // TODO M2.4: Implement actual extraction via inference actor directed channel.
        // For now, return placeholder extracted facts.
        //
        // Real implementation should:
        // 1. Send ExtractionRequested to inference actor via directed channel
        // 2. Await ExtractionCompleted response
        // 3. Return the facts
        //
        // This requires async context, but ech0's Extractor trait is sync.
        // Resolution: use tokio::runtime::Handle::current().block_on() or similar.
        
        let _request_id = self.next_request_id();
        
        // Placeholder: return empty fact list
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extractor_returns_placeholder_facts() {
        let bus = Arc::new(EventBus::new());
        let extractor = SenaExtractor::new(bus);
        let result = extractor.extract("test text");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 0);
    }
}
