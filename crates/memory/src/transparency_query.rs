//! Transparency query handler — responds to user queries about memory state.
//!
//! Handles `TransparencyQuery::UserMemory` by fetching the current Soul summary
//! and returning top memory chunks ranked by recency and importance.

use std::sync::Arc;
use std::time::Duration;

use bus::events::memory::MemoryChunk;
use bus::events::soul::SoulReadRequest;
use bus::events::transparency::{MemoryResponse, SoulSummaryForTransparency};
use bus::{Event, EventBus, SoulEvent, TransparencyEvent};
use ech0::schema::MemoryTier;
use ech0::{SearchOptions, Store};
use tokio::sync::broadcast;
use tokio::time::timeout;

use crate::embedder::SenaEmbedder;
use crate::error::MemoryError;
use crate::extractor::SenaExtractor;

const TRANSPARENCY_QUERY_TIMEOUT: Duration = Duration::from_secs(2);
const TOP_MEMORY_CHUNKS: usize = 8;

pub(crate) fn empty_memory_response() -> MemoryResponse {
    MemoryResponse {
        soul_summary: default_soul_summary(),
        memory_chunks: Vec::new(),
    }
}

/// Handle a transparency query for user memory: fetch Soul summary + recent memory chunks.
///
/// # Timeouts
/// If Soul does not respond within 2 seconds, emits a default summary.
/// If ech0 search fails (e.g. model not loaded yet), emits an empty chunk list.
/// In both cases a MemoryResponded event is always emitted.
pub async fn handle_transparency_query(
    store: Arc<Store<SenaEmbedder, SenaExtractor>>,
    bus: Arc<EventBus>,
    broadcast_rx: &mut broadcast::Receiver<Event>,
    request_id: u64,
) -> Result<(), MemoryError> {
    // Step 1: Request Soul summary
    let soul_request = SoulReadRequest { request_id };
    if let Err(error) = bus
        .broadcast(Event::Soul(SoulEvent::ReadRequested(soul_request)))
        .await
    {
        eprintln!(
            "[memory transparency] soul summary request failed ({}), using default summary",
            error
        );
    }

    // Step 2: Wait for Soul response with timeout
    let soul_summary = wait_for_soul_summary(broadcast_rx, request_id).await;

    // Step 3: Retrieve top recent memory chunks.
    // If the ech0 store search fails (e.g. embedding model not loaded yet),
    // fall back to an empty list so we always emit a response.
    let memory_chunks = retrieve_recent_memory_chunks(store.as_ref())
        .await
        .unwrap_or_else(|e| {
            eprintln!(
                "[memory transparency] chunk retrieval failed ({}), returning empty list",
                e
            );
            vec![]
        });

    // Step 4: Construct and emit response
    let response = MemoryResponse {
        soul_summary,
        memory_chunks,
    };

    let event = Event::Transparency(TransparencyEvent::MemoryResponded(response));
    bus.broadcast(event)
        .await
        .map_err(|e| MemoryError::Store(format!("failed to emit MemoryResponded: {e}")))?;

    Ok(())
}

/// Wait for `SoulReadCompleted` with the given request_id, with a timeout.
/// If timeout occurs, return a default (zero-valued) summary.
async fn wait_for_soul_summary(
    broadcast_rx: &mut broadcast::Receiver<Event>,
    request_id: u64,
) -> SoulSummaryForTransparency {
    wait_for_soul_summary_with_timeout(broadcast_rx, request_id, TRANSPARENCY_QUERY_TIMEOUT).await
}

async fn wait_for_soul_summary_with_timeout(
    broadcast_rx: &mut broadcast::Receiver<Event>,
    request_id: u64,
    timeout_duration: Duration,
) -> SoulSummaryForTransparency {
    let result = timeout(
        timeout_duration,
        wait_for_soul_event(broadcast_rx, request_id),
    )
    .await;

    match result {
        Ok(Some(summary)) => summary,
        Ok(None) | Err(_) => default_soul_summary(),
    }
}

fn default_soul_summary() -> SoulSummaryForTransparency {
    SoulSummaryForTransparency {
        user_name: None,
        inference_cycle_count: 0,
        work_patterns: vec![],
        tool_preferences: vec![],
        interest_clusters: vec![],
    }
}

/// Loop through broadcast events until we find `SoulReadCompleted` with matching request_id.
async fn wait_for_soul_event(
    broadcast_rx: &mut broadcast::Receiver<Event>,
    request_id: u64,
) -> Option<SoulSummaryForTransparency> {
    loop {
        match broadcast_rx.recv().await {
            Ok(Event::Soul(SoulEvent::ReadCompleted(completed))) => {
                if completed.request_id == request_id {
                    return Some(completed.summary.clone());
                }
            }
            Ok(_) => {}
            Err(broadcast::error::RecvError::Closed) => return None,
            Err(broadcast::error::RecvError::Lagged(_)) => {}
        }
    }
}

/// Retrieve the top N recent and important memory chunks from the store.
///
/// Returns `Ok(vec![])` on search failure so callers always receive a response.
async fn retrieve_recent_memory_chunks(
    store: &Store<SenaEmbedder, SenaExtractor>,
) -> Result<Vec<MemoryChunk>, MemoryError> {
    let query = "recent important memories and observations";

    let search_options = SearchOptions {
        limit: TOP_MEMORY_CHUNKS,
        vector_weight: 0.6,
        graph_weight: 0.4,
        min_importance: 0.0,
        tiers: vec![MemoryTier::Episodic, MemoryTier::Semantic],
    };

    let search_result = store
        .search(query, search_options)
        .await
        .map_err(|e| MemoryError::Store(e.to_string()))?;

    let chunks: Vec<MemoryChunk> = search_result
        .nodes
        .into_iter()
        .map(|scored_node| {
            let text = extract_node_text(&scored_node.node);
            MemoryChunk {
                text,
                score: scored_node.score,
                timestamp: std::time::SystemTime::now(),
            }
        })
        .collect();

    Ok(chunks)
}

/// Extract display text from an ech0 node.
fn extract_node_text(node: &ech0::Node) -> String {
    if let Some(src) = &node.source_text {
        return src.clone();
    }
    if let Some(t) = node.metadata.get("text").and_then(|v| v.as_str()) {
        return t.to_owned();
    }
    node.kind.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_soul_summary_is_empty() {
        let summary = default_soul_summary();
        assert_eq!(summary.inference_cycle_count, 0);
        assert!(summary.work_patterns.is_empty());
    }

    #[tokio::test]
    async fn wait_for_soul_summary_timeout_returns_default_summary() {
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        let summary =
            wait_for_soul_summary_with_timeout(&mut rx, 42, Duration::from_millis(10)).await;

        assert_eq!(summary.inference_cycle_count, 0);
        assert!(summary.work_patterns.is_empty());
        assert!(summary.tool_preferences.is_empty());
        assert!(summary.interest_clusters.is_empty());
    }

    #[test]
    fn empty_memory_response_has_no_chunks() {
        let response = empty_memory_response();

        assert_eq!(response.soul_summary.inference_cycle_count, 0);
        assert!(response.memory_chunks.is_empty());
    }

    #[test]
    fn memory_chunk_constructs_correctly() {
        let chunk = MemoryChunk {
            text: "test chunk".into(),
            score: 0.85,
            timestamp: std::time::SystemTime::now(),
        };
        assert_eq!(chunk.text, "test chunk");
        assert!((chunk.score - 0.85).abs() < 0.001);
    }
}
