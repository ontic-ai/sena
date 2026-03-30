//! Memory-layer events: ingest, query, retrieval, conflicts.

use std::time::SystemTime;

/// A chunk of retrieved memory with a relevance score.
#[derive(Debug, Clone)]
pub struct MemoryChunk {
    pub text: String,
    pub score: f32,
    pub timestamp: SystemTime,
}

/// Request to ingest text into long-term memory (ech0 store).
#[derive(Debug, Clone)]
pub struct MemoryWriteRequest {
    pub text: String,
    pub request_id: u64,
}

/// Request to query long-term memory.
#[derive(Debug, Clone)]
pub struct MemoryQueryRequest {
    pub query: String,
    pub token_budget: usize,
    pub request_id: u64,
}

/// Response to a memory query.
#[derive(Debug, Clone)]
pub struct MemoryQueryResponse {
    pub chunks: Vec<MemoryChunk>,
    pub request_id: u64,
}

/// Emitted when ech0 detects a conflict during ingest.
#[derive(Debug, Clone)]
pub struct MemoryConflictDetected {
    pub description: String,
    pub request_id: u64,
}

/// Request to ingest distilled factual/semantic content into long-term memory
/// with an explicit routing key for cluster assignment.
#[derive(Debug, Clone)]
pub struct SemanticIngestRequest {
    /// The distilled text (facts/patterns extracted from episodic memory).
    pub text: String,
    /// Routing / cluster key (e.g. "factual", "preference", "habit").
    pub routing_key: String,
    pub request_id: u64,
}

/// Emitted after a semantic ingest completes.
#[derive(Debug, Clone)]
pub struct SemanticIngestComplete {
    pub node_id: u64,
    pub request_id: u64,
}

/// Emitted when the background consolidation job completes successfully.
#[derive(Debug, Clone)]
pub struct MemoryConsolidationCompleted {
    /// Number of nodes whose importance was updated by decay.
    pub nodes_decayed: usize,
}

/// Top-level memory event enum wrapping all memory subsystem events.
#[derive(Debug, Clone)]
pub enum MemoryEvent {
    WriteRequested(MemoryWriteRequest),
    SemanticIngestRequested(SemanticIngestRequest),
    SemanticIngestComplete(SemanticIngestComplete),
    QueryRequested(MemoryQueryRequest),
    QueryCompleted(MemoryQueryResponse),
    ConflictDetected(MemoryConflictDetected),
    ConsolidationCompleted(MemoryConsolidationCompleted),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_chunk_constructs_and_clones() {
        let c = MemoryChunk {
            text: "t".into(),
            score: 0.9,
            timestamp: SystemTime::now(),
        };
        assert_eq!(c.clone().text, "t");
    }

    #[test]
    fn memory_event_all_variants_clone() {
        let events = vec![
            MemoryEvent::WriteRequested(MemoryWriteRequest {
                text: "t".into(),
                request_id: 1,
            }),
            MemoryEvent::QueryRequested(MemoryQueryRequest {
                query: "q".into(),
                token_budget: 512,
                request_id: 2,
            }),
            MemoryEvent::QueryCompleted(MemoryQueryResponse {
                chunks: vec![],
                request_id: 3,
            }),
            MemoryEvent::ConflictDetected(MemoryConflictDetected {
                description: "d".into(),
                request_id: 4,
            }),
        ];
        assert_eq!(events.iter().cloned().count(), 4);
    }

    fn assert_send_static<T: Send + 'static>() {}

    #[test]
    fn all_memory_types_are_send_and_static() {
        assert_send_static::<MemoryChunk>();
        assert_send_static::<MemoryWriteRequest>();
        assert_send_static::<MemoryQueryRequest>();
        assert_send_static::<MemoryQueryResponse>();
        assert_send_static::<MemoryConflictDetected>();
        assert_send_static::<MemoryEvent>();
    }
}
