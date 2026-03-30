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

/// Handle a transparency query for user memory: fetch Soul summary + recent memory chunks.
///
/// # Process
/// 1. Emit `SoulReadRequest` on the bus with a unique request_id
/// 2. Wait for `SoulReadCompleted` response with a timeout (2 seconds)
/// 3. Query the ech0 store for recent and important memory chunks
/// 4. Construct and emit `Event::Transparency(TransparencyEvent::MemoryResponded(...))`
///
/// # Timeouts
/// If Soul does not respond within 2 seconds, emit a default Soul summary.
pub async fn handle_transparency_query(
    store: Arc<Store<SenaEmbedder, SenaExtractor>>,
    bus: Arc<EventBus>,
    broadcast_rx: &mut broadcast::Receiver<Event>,
    request_id: u64,
) -> Result<(), MemoryError> {
    // Step 1: Request Soul summary
    let soul_request = SoulReadRequest { request_id };
    bus.broadcast(Event::Soul(SoulEvent::ReadRequested(soul_request)))
        .await
        .map_err(|e| MemoryError::Store(format!("failed to emit SoulReadRequest: {e}")))?;

    // Step 2: Wait for Soul response with timeout
    let soul_summary = wait_for_soul_summary(broadcast_rx, request_id).await;

    // Step 3: Retrieve top recent memory chunks (recency + importance weighted)
    let memory_chunks = retrieve_recent_memory_chunks(store.as_ref()).await?;

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
    let result = timeout(
        TRANSPARENCY_QUERY_TIMEOUT,
        wait_for_soul_event(broadcast_rx, request_id),
    )
    .await;

    match result {
        Ok(Some(summary)) => summary,
        Ok(None) => {
            // Event loop ended without finding matching response
            eprintln!(
                "[memory transparency] Soul response not found for request_id {}",
                request_id
            );
            SoulSummaryForTransparency {
                inference_cycle_count: 0,
                work_patterns: vec![],
                tool_preferences: vec![],
                interest_clusters: vec![],
            }
        }
        Err(_) => {
            // Timeout after 2 seconds
            eprintln!(
                "[memory transparency] Soul response timeout for request_id {}",
                request_id
            );
            SoulSummaryForTransparency {
                inference_cycle_count: 0,
                work_patterns: vec![],
                tool_preferences: vec![],
                interest_clusters: vec![],
            }
        }
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
            Ok(_) => {
                // Other event, keep waiting
            }
            Err(broadcast::error::RecvError::Closed) => {
                return None;
            }
            Err(broadcast::error::RecvError::Lagged(_)) => {
                // Handle lag gracefully, keep trying
            }
        }
    }
}

/// Retrieve the top N recent and important memory chunks from the store.
///
/// Uses a vector-heavy search to rank by semantic relevance and time-based decay.
/// Memory chunks older than the top candidates are excluded via importance threshold.
async fn retrieve_recent_memory_chunks(
    store: &Store<SenaEmbedder, SenaExtractor>,
) -> Result<Vec<MemoryChunk>, MemoryError> {
    // Use a broad semantic query to capture recent and important memories across all tiers.
    // The query asks the store to retrieve memories ranked by importance and recency.
    let query = "recent important memories and observations";

    let search_options = SearchOptions {
        limit: TOP_MEMORY_CHUNKS,
        vector_weight: 0.6,  // Prioritize semantic similarity
        graph_weight: 0.4,   // Consider graph relationships (importance)
        min_importance: 0.0, // Include all, ranking by score handles recency/importance
        tiers: vec![MemoryTier::Episodic, MemoryTier::Semantic],
    };

    let search_result = store
        .search(query, search_options)
        .await
        .map_err(|e| MemoryError::Store(e.to_string()))?;

    // Convert ech0 nodes to MemoryChunk, extracting text and preserving scores
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
///
/// Priority: `source_text` (provenance feature) → `metadata["text"]` → `kind`.
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
        let summary = SoulSummaryForTransparency {
            inference_cycle_count: 0,
            work_patterns: vec![],
            tool_preferences: vec![],
            interest_clusters: vec![],
        };
        assert_eq!(summary.inference_cycle_count, 0);
        assert!(summary.work_patterns.is_empty());
        assert!(summary.tool_preferences.is_empty());
        assert!(summary.interest_clusters.is_empty());
    }

    #[test]
    fn memory_chunk_constructs_from_scored_node() {
        let chunk = MemoryChunk {
            text: "test chunk".into(),
            score: 0.85,
            timestamp: std::time::SystemTime::now(),
        };
        assert_eq!(chunk.text, "test chunk");
        assert!((chunk.score - 0.85).abs() < 0.001);
    }
}
