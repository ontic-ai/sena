//! Memory-layer events: ingest, query, retrieval, conflicts.

use std::time::SystemTime;

/// A chunk of retrieved memory with a relevance score.
#[derive(Clone)]
pub struct MemoryChunk {
    pub text: String,
    pub score: f32,
    pub timestamp: SystemTime,
}

impl std::fmt::Debug for MemoryChunk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryChunk")
            .field("text", &"[REDACTED]")
            .field("score", &self.score)
            .field("timestamp", &self.timestamp)
            .finish()
    }
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
#[derive(Clone)]
pub struct MemoryQueryResponse {
    pub chunks: Vec<MemoryChunk>,
    pub request_id: u64,
}

impl std::fmt::Debug for MemoryQueryResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryQueryResponse")
            .field(
                "chunks",
                &format!("[{} chunks, content REDACTED]", self.chunks.len()),
            )
            .field("request_id", &self.request_id)
            .finish()
    }
}

/// Emitted after a write to long-term memory completes successfully.
#[derive(Debug, Clone)]
pub struct MemoryWriteCompleted {
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

/// Request from CTP to query memories relevant to the current context.
/// Unlike QueryRequested (user-initiated), this is automatic/proactive.
#[derive(Debug, Clone)]
pub struct ContextMemoryQueryRequest {
    /// Semantic description of current activity (from EnrichedInferredTask).
    pub context_description: String,
    /// Maximum chunks to return.
    pub max_chunks: usize,
    /// Unique request ID for response correlation.
    pub request_id: u64,
}

/// Response to a context memory query.
#[derive(Clone)]
pub struct ContextMemoryQueryResponse {
    pub chunks: Vec<MemoryChunk>,
    /// Relevance score (0.0-1.0) — how relevant the retrieved memories are overall.
    pub relevance_score: f64,
    pub request_id: u64,
}

impl std::fmt::Debug for ContextMemoryQueryResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContextMemoryQueryResponse")
            .field(
                "chunks",
                &format!("[{} chunks, content REDACTED]", self.chunks.len()),
            )
            .field("relevance_score", &self.relevance_score)
            .field("request_id", &self.request_id)
            .finish()
    }
}

/// Top-level memory event enum wrapping all memory subsystem events.
#[derive(Debug, Clone)]
pub enum MemoryEvent {
    WriteRequested(MemoryWriteRequest),
    WriteCompleted(MemoryWriteCompleted),
    SemanticIngestRequested(SemanticIngestRequest),
    SemanticIngestComplete(SemanticIngestComplete),
    QueryRequested(MemoryQueryRequest),
    QueryCompleted(MemoryQueryResponse),
    ConflictDetected(MemoryConflictDetected),
    ConsolidationCompleted(MemoryConsolidationCompleted),
    ContextQueryRequested(ContextMemoryQueryRequest),
    ContextQueryCompleted(ContextMemoryQueryResponse),
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
    fn memory_chunk_debug_redacts_text() {
        let c = MemoryChunk {
            text: "sensitive content".into(),
            score: 0.85,
            timestamp: SystemTime::now(),
        };
        let debug_output = format!("{:?}", c);
        assert!(debug_output.contains("[REDACTED]"));
        assert!(!debug_output.contains("sensitive content"));
        assert!(debug_output.contains("0.85"));
    }

    #[test]
    fn memory_query_response_debug_redacts_chunks() {
        let response = MemoryQueryResponse {
            chunks: vec![
                MemoryChunk {
                    text: "secret text 1".into(),
                    score: 0.9,
                    timestamp: SystemTime::now(),
                },
                MemoryChunk {
                    text: "secret text 2".into(),
                    score: 0.8,
                    timestamp: SystemTime::now(),
                },
            ],
            request_id: 42,
        };
        let debug_output = format!("{:?}", response);
        assert!(debug_output.contains("2 chunks"));
        assert!(debug_output.contains("REDACTED"));
        assert!(!debug_output.contains("secret text"));
        assert!(debug_output.contains("42"));
    }

    #[test]
    fn memory_event_all_variants_clone() {
        let events = [
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
        assert_eq!(events.iter().count(), 4);
    }

    fn assert_send_static<T: Send + 'static>() {}

    #[test]
    fn all_memory_types_are_send_and_static() {
        assert_send_static::<MemoryChunk>();
        assert_send_static::<MemoryWriteRequest>();
        assert_send_static::<MemoryQueryRequest>();
        assert_send_static::<MemoryQueryResponse>();
        assert_send_static::<MemoryConflictDetected>();
        assert_send_static::<ContextMemoryQueryRequest>();
        assert_send_static::<ContextMemoryQueryResponse>();
        assert_send_static::<MemoryEvent>();
    }

    #[test]
    fn context_query_request_constructs() {
        let req = ContextMemoryQueryRequest {
            context_description: "coding in Rust".to_string(),
            max_chunks: 10,
            request_id: 42,
        };
        assert_eq!(req.context_description, "coding in Rust");
        assert_eq!(req.max_chunks, 10);
        assert_eq!(req.request_id, 42);
    }

    #[test]
    fn context_memory_query_response_debug_redacts() {
        let response = ContextMemoryQueryResponse {
            chunks: vec![
                MemoryChunk {
                    text: "secret memory 1".into(),
                    score: 0.9,
                    timestamp: SystemTime::now(),
                },
                MemoryChunk {
                    text: "secret memory 2".into(),
                    score: 0.8,
                    timestamp: SystemTime::now(),
                },
            ],
            relevance_score: 0.85,
            request_id: 99,
        };
        let debug_output = format!("{:?}", response);
        assert!(debug_output.contains("2 chunks"));
        assert!(debug_output.contains("REDACTED"));
        assert!(!debug_output.contains("secret memory"));
        assert!(debug_output.contains("0.85"));
        assert!(debug_output.contains("99"));
    }
}
