//! Extractor trait implementation — calls inference actor for fact extraction.
//!
//! `SenaExtractor` implements the real `ech0::Extractor` async trait by routing
//! extraction requests through the event bus to the inference actor, then
//! converting the returned string facts into ech0 `Node` objects.
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
use chrono::Utc;
use ech0::{
    schema::{Edge, Node},
    traits::ExtractionResult,
    EchoError,
};
use uuid::Uuid;

/// Timeout to wait for an ExtractionCompleted response from the inference actor.
const EXTRACT_TIMEOUT: Duration = Duration::from_secs(30);

/// Implements `ech0::Extractor` by forwarding extraction requests to the
/// inference actor and converting the returned Vec<String> facts into
/// `ech0::Node` objects.
///
/// Each fact string becomes a single `Node` of kind `"fact"`. No edges are
/// generated at this stage; ech0's dynamic-linking pass adds edges later.
pub struct SenaExtractor {
    bus: Arc<EventBus>,
    counter: AtomicU64,
}

impl SenaExtractor {
    /// Create a new extractor that communicates via the given bus.
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
impl ech0::Extractor for SenaExtractor {
    async fn extract(&self, text: &str) -> Result<ExtractionResult, EchoError> {
        let request_id = self.next_id();
        let mut rx = self.bus.subscribe_broadcast();

        self.bus
            .send_directed(
                "inference",
                Event::Inference(InferenceEvent::ExtractionRequested {
                    text: text.to_owned(),
                    request_id,
                }),
            )
            .await
            .map_err(|e| EchoError::extractor_failure(format!("bus send failed: {e}")))?;

        let deadline = tokio::time::Instant::now() + EXTRACT_TIMEOUT;

        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(EchoError::extractor_failure(
                    "extract timeout: no response from inference actor",
                ));
            }

            let recv_fut = rx.recv();
            match tokio::time::timeout(remaining, recv_fut).await {
                Ok(Ok(Event::Inference(InferenceEvent::ExtractionCompleted {
                    facts,
                    request_id: rid,
                }))) if rid == request_id => {
                    let nodes = facts_to_nodes(facts, text);
                    return Ok(ExtractionResult {
                        nodes,
                        edges: Vec::<Edge>::new(),
                    });
                }
                Ok(Ok(Event::Inference(InferenceEvent::InferenceFailed {
                    request_id: rid,
                    reason,
                }))) if rid == request_id => {
                    return Err(EchoError::extractor_failure(format!(
                        "inference actor failed: {reason}"
                    )));
                }
                Ok(Ok(_)) => continue,
                Ok(Err(_)) => {
                    return Err(EchoError::extractor_failure("bus channel closed"));
                }
                Err(_) => {
                    return Err(EchoError::extractor_failure(
                        "extract timeout: no response from inference actor",
                    ));
                }
            }
        }
    }
}

/// Convert a list of extracted fact strings into ech0 `Node`s.
///
/// Each fact becomes a node of kind `"fact"` with the fact text stored in
/// `metadata["text"]`. The ingest_id is left as `Uuid::nil()` — ech0
/// overwrites it with the canonical ingest ID during the write path.
fn facts_to_nodes(facts: Vec<String>, source_text: &str) -> Vec<Node> {
    if facts.is_empty() {
        // When the model returns no structured facts, fall back to a single
        // "raw" node carrying the full source text so the text is still
        // indexed and retrievable.
        let node = Node {
            id: Uuid::new_v4(),
            kind: "raw".to_owned(),
            metadata: serde_json::json!({ "text": source_text }),
            importance: 1.0,
            created_at: Utc::now(),
            ingest_id: Uuid::nil(),
            source_text: None,
        };
        return vec![node];
    }

    facts
        .into_iter()
        .map(|fact| Node {
            id: Uuid::new_v4(),
            kind: "fact".to_owned(),
            metadata: serde_json::json!({ "text": fact }),
            importance: 1.0,
            created_at: Utc::now(),
            ingest_id: Uuid::nil(),
            source_text: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facts_to_nodes_empty_produces_raw_fallback() {
        let nodes = facts_to_nodes(vec![], "some source text");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].kind, "raw");
        assert_eq!(nodes[0].metadata["text"], "some source text");
    }

    #[test]
    fn facts_to_nodes_maps_each_fact_to_node() {
        let facts = vec!["Alice is 30".to_string(), "Alice likes Rust".to_string()];
        let nodes = facts_to_nodes(facts, "anything");
        assert_eq!(nodes.len(), 2);
        assert!(nodes.iter().all(|n| n.kind == "fact"));
        assert_eq!(nodes[0].metadata["text"], "Alice is 30");
        assert_eq!(nodes[1].metadata["text"], "Alice likes Rust");
    }
}
