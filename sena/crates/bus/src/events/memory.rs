//! Memory-layer events: ingest, query, retrieval.

use crate::causal::CausalId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Kind of memory being stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MemoryKind {
    /// Episodic memory derived from platform observations.
    Episodic,
    /// Semantic memory from inferred facts or user statements.
    Semantic,
    /// Procedural memory from interaction patterns.
    Procedural,
    /// Working memory — ephemeral, not persisted to long-term store.
    Working,
}

/// A chunk of retrieved memory with relevance score and age metadata.
#[derive(Clone, Serialize, Deserialize)]
pub struct MemoryChunk {
    /// The text content of the memory chunk.
    pub content: String,
    /// Relevance score (higher is more relevant).
    pub score: f32,
    /// Age of the memory in seconds since creation.
    pub age_seconds: u64,
}

impl std::fmt::Debug for MemoryChunk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryChunk")
            .field("content", &format!("[{} chars]", self.content.len()))
            .field("score", &self.score)
            .field("age_seconds", &self.age_seconds)
            .finish()
    }
}

/// A chunk of retrieved memory with relevance score and age metadata.
#[derive(Clone, Serialize, Deserialize)]
pub struct ScoredChunk {
    /// The text content of the memory chunk.
    pub content: String,
    /// Relevance score (higher is more relevant).
    pub score: f32,
    /// Age of the memory in seconds since creation.
    pub age_seconds: u64,
}

impl std::fmt::Debug for ScoredChunk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScoredChunk")
            .field("content", &"[REDACTED]")
            .field("score", &self.score)
            .field("age_seconds", &self.age_seconds)
            .finish()
    }
}

/// Request from CTP to query memories relevant to the current context.
///
/// This is distinct from user-initiated queries (`QueryRequested`).
/// Context queries are automatic/proactive and include aggregate relevance scoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextMemoryQueryRequest {
    /// Semantic description of current activity (from snapshot or inferred task).
    pub context_description: String,
    /// Maximum chunks to return.
    pub max_chunks: usize,
    /// Causal chain ID.
    pub causal_id: CausalId,
}

/// Response to a context memory query.
///
/// Includes both the memory chunks and an aggregate relevance score
/// to help CTP assess overall memory utility for the current context.
#[derive(Clone, Serialize, Deserialize)]
pub struct ContextMemoryQueryResponse {
    /// Retrieved memory chunks with scores and age metadata.
    pub chunks: Vec<ScoredChunk>,
    /// Aggregate relevance score (0.0-1.0) — how relevant the retrieved memories are overall.
    pub relevance_score: f64,
    /// Causal chain ID.
    pub causal_id: CausalId,
}

impl std::fmt::Debug for ContextMemoryQueryResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContextMemoryQueryResponse")
            .field("chunk_count", &self.chunks.len())
            .field("relevance_score", &self.relevance_score)
            .field("causal_id", &self.causal_id)
            .finish()
    }
}

/// Memory subsystem events.
#[derive(Clone)]
pub enum MemoryEvent {
    /// Request current memory statistics.
    StatsRequested {
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Current memory statistics snapshot.
    StatsCompleted {
        working_memory_chunks: usize,
        long_term_memory_nodes: usize,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Memory stats request failed.
    StatsFailed {
        /// Causal chain ID.
        causal_id: CausalId,
        reason: String,
    },

    /// Request to ingest text into long-term memory.
    IngestRequested {
        text: String,
        kind: MemoryKind,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Ingest to long-term memory completed.
    IngestCompleted {
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Ingest to long-term memory failed.
    IngestFailed {
        /// Causal chain ID.
        causal_id: CausalId,
        reason: String,
    },

    /// Request to query long-term memory using semantic query string.
    QueryRequested {
        query: String,
        limit: usize,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Response to a memory query.
    QueryCompleted {
        chunks: Vec<ScoredChunk>,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Memory query failed.
    QueryFailed {
        /// Causal chain ID.
        causal_id: CausalId,
        reason: String,
    },

    /// Request from CTP to query memories relevant to the current context.
    ContextQueryRequested(ContextMemoryQueryRequest),

    /// Response to a context memory query with aggregate relevance scoring.
    ContextQueryCompleted(ContextMemoryQueryResponse),

    /// Context memory query failed.
    ContextQueryFailed {
        /// Causal chain ID.
        causal_id: CausalId,
        reason: String,
    },

    // Aliases for alternative naming (used in some documentation/specs)
    /// Alias for IngestRequested.
    MemoryWriteRequest {
        text: String,
        kind: MemoryKind,
        causal_id: CausalId,
    },

    /// Alias for IngestCompleted.
    MemoryWriteCompleted { causal_id: CausalId },

    /// Alias for IngestFailed.
    MemoryWriteFailed { causal_id: CausalId, reason: String },

    /// Alias for QueryRequested.
    MemoryQueryRequest {
        query: String,
        limit: usize,
        causal_id: CausalId,
    },

    /// Alias for QueryCompleted.
    MemoryQueryResponse {
        chunks: Vec<ScoredChunk>,
        causal_id: CausalId,
    },

    /// Memory backup completed successfully.
    BackupCompleted {
        /// Path to the backup file that was written.
        path: PathBuf,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Memory backup failed.
    BackupFailed {
        /// Human-readable failure reason.
        reason: String,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Memory consolidation completed (periodic background maintenance).
    ConsolidationCompleted {
        /// Number of memory nodes decayed in this consolidation cycle.
        nodes_decayed: usize,
    },
}

impl std::fmt::Debug for MemoryEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StatsRequested { causal_id } => f
                .debug_struct("StatsRequested")
                .field("causal_id", causal_id)
                .finish(),
            Self::IngestRequested {
                kind, causal_id, ..
            }
            | Self::MemoryWriteRequest {
                kind, causal_id, ..
            } => f
                .debug_struct("IngestRequested/MemoryWriteRequest")
                .field("text", &"[REDACTED]")
                .field("kind", kind)
                .field("causal_id", causal_id)
                .finish(),
            Self::StatsCompleted {
                working_memory_chunks,
                long_term_memory_nodes,
                causal_id,
            } => f
                .debug_struct("StatsCompleted")
                .field("working_memory_chunks", working_memory_chunks)
                .field("long_term_memory_nodes", long_term_memory_nodes)
                .field("causal_id", causal_id)
                .finish(),
            Self::StatsFailed { causal_id, reason } => f
                .debug_struct("StatsFailed")
                .field("causal_id", causal_id)
                .field("reason", reason)
                .finish(),
            Self::IngestCompleted { causal_id } | Self::MemoryWriteCompleted { causal_id } => f
                .debug_struct("IngestCompleted/MemoryWriteCompleted")
                .field("causal_id", causal_id)
                .finish(),
            Self::IngestFailed { causal_id, reason }
            | Self::MemoryWriteFailed { causal_id, reason } => f
                .debug_struct("IngestFailed/MemoryWriteFailed")
                .field("causal_id", causal_id)
                .field("reason", reason)
                .finish(),
            Self::QueryRequested {
                query,
                limit,
                causal_id,
            }
            | Self::MemoryQueryRequest {
                query,
                limit,
                causal_id,
            } => f
                .debug_struct("QueryRequested/MemoryQueryRequest")
                .field("query", &format!("[{} chars]", query.len()))
                .field("limit", limit)
                .field("causal_id", causal_id)
                .finish(),
            Self::QueryCompleted { chunks, causal_id }
            | Self::MemoryQueryResponse { chunks, causal_id } => f
                .debug_struct("QueryCompleted/MemoryQueryResponse")
                .field("chunk_count", &chunks.len())
                .field("causal_id", causal_id)
                .finish(),
            Self::QueryFailed { causal_id, reason } => f
                .debug_struct("QueryFailed")
                .field("causal_id", causal_id)
                .field("reason", reason)
                .finish(),
            Self::ContextQueryRequested(req) => f
                .debug_struct("ContextQueryRequested")
                .field(
                    "context_description",
                    &format!("[{} chars]", req.context_description.len()),
                )
                .field("max_chunks", &req.max_chunks)
                .field("causal_id", &req.causal_id)
                .finish(),
            Self::ContextQueryCompleted(resp) => f
                .debug_struct("ContextQueryCompleted")
                .field("chunk_count", &resp.chunks.len())
                .field("relevance_score", &resp.relevance_score)
                .field("causal_id", &resp.causal_id)
                .finish(),
            Self::ContextQueryFailed { causal_id, reason } => f
                .debug_struct("ContextQueryFailed")
                .field("causal_id", causal_id)
                .field("reason", reason)
                .finish(),
            Self::BackupCompleted { path, causal_id } => f
                .debug_struct("BackupCompleted")
                .field("path", path)
                .field("causal_id", causal_id)
                .finish(),
            Self::BackupFailed { reason, causal_id } => f
                .debug_struct("BackupFailed")
                .field("reason", reason)
                .field("causal_id", causal_id)
                .finish(),
            Self::ConsolidationCompleted { nodes_decayed } => f
                .debug_struct("ConsolidationCompleted")
                .field("nodes_decayed", nodes_decayed)
                .finish(),
        }
    }
}

impl MemoryEvent {
    pub fn causal_id(&self) -> Option<CausalId> {
        match self {
            Self::StatsRequested { causal_id, .. }
            | Self::StatsCompleted { causal_id, .. }
            | Self::StatsFailed { causal_id, .. }
            | Self::IngestRequested { causal_id, .. }
            | Self::IngestCompleted { causal_id, .. }
            | Self::IngestFailed { causal_id, .. }
            | Self::QueryRequested { causal_id, .. }
            | Self::QueryCompleted { causal_id, .. }
            | Self::QueryFailed { causal_id, .. }
            | Self::MemoryWriteRequest { causal_id, .. }
            | Self::MemoryWriteCompleted { causal_id, .. }
            | Self::MemoryWriteFailed { causal_id, .. }
            | Self::MemoryQueryRequest { causal_id, .. }
            | Self::MemoryQueryResponse { causal_id, .. }
            | Self::ContextQueryFailed { causal_id, .. }
            | Self::BackupCompleted { causal_id, .. }
            | Self::BackupFailed { causal_id, .. } => Some(*causal_id),
            Self::ContextQueryRequested(req) => Some(req.causal_id),
            Self::ContextQueryCompleted(resp) => Some(resp.causal_id),
            Self::ConsolidationCompleted { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_event_causal_id_extraction() {
        let cid = CausalId::new();
        let event = MemoryEvent::IngestRequested {
            text: "test".to_string(),
            kind: MemoryKind::Episodic,
            causal_id: cid,
        };
        assert_eq!(event.causal_id(), Some(cid));
    }

    #[test]
    fn scored_chunk_debug_redacts_content() {
        let chunk = ScoredChunk {
            content: "sensitive data".to_string(),
            score: 0.9,
            age_seconds: 3600,
        };
        let debug_str = format!("{:?}", chunk);
        assert!(debug_str.contains("REDACTED"));
        assert!(!debug_str.contains("sensitive data"));
    }

    #[test]
    fn context_memory_query_request_constructs() {
        let cid = CausalId::new();
        let req = ContextMemoryQueryRequest {
            context_description: "coding in Rust".to_string(),
            max_chunks: 10,
            causal_id: cid,
        };
        assert_eq!(req.context_description, "coding in Rust");
        assert_eq!(req.max_chunks, 10);
        assert_eq!(req.causal_id, cid);
    }

    #[test]
    fn context_memory_query_response_debug_redacts() {
        let cid = CausalId::new();
        let response = ContextMemoryQueryResponse {
            chunks: vec![
                ScoredChunk {
                    content: "secret memory 1".into(),
                    score: 0.9,
                    age_seconds: 3600,
                },
                ScoredChunk {
                    content: "secret memory 2".into(),
                    score: 0.8,
                    age_seconds: 7200,
                },
            ],
            relevance_score: 0.85,
            causal_id: cid,
        };
        let debug_output = format!("{:?}", response);
        assert!(debug_output.contains("chunk_count"));
        assert!(debug_output.contains("2"));
        assert!(debug_output.contains("0.85"));
        assert!(!debug_output.contains("secret memory"));
    }

    #[test]
    fn context_query_events_extract_causal_id() {
        let cid = CausalId::new();
        let req_event = MemoryEvent::ContextQueryRequested(ContextMemoryQueryRequest {
            context_description: "test context".to_string(),
            max_chunks: 5,
            causal_id: cid,
        });
        assert_eq!(req_event.causal_id(), Some(cid));

        let resp_event = MemoryEvent::ContextQueryCompleted(ContextMemoryQueryResponse {
            chunks: vec![],
            relevance_score: 0.5,
            causal_id: cid,
        });
        assert_eq!(resp_event.causal_id(), Some(cid));

        let fail_event = MemoryEvent::ContextQueryFailed {
            causal_id: cid,
            reason: "test error".to_string(),
        };
        assert_eq!(fail_event.causal_id(), Some(cid));
    }

    #[test]
    fn context_query_request_serializes() {
        let cid = CausalId::new();
        let req = ContextMemoryQueryRequest {
            context_description: "test".to_string(),
            max_chunks: 3,
            causal_id: cid,
        };
        let json = serde_json::to_string(&req).unwrap();
        let deserialized: ContextMemoryQueryRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.context_description, "test");
        assert_eq!(deserialized.max_chunks, 3);
        assert_eq!(deserialized.causal_id, cid);
    }

    #[test]
    fn context_query_response_serializes() {
        let cid = CausalId::new();
        let resp = ContextMemoryQueryResponse {
            chunks: vec![],
            relevance_score: 0.75,
            causal_id: cid,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let deserialized: ContextMemoryQueryResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.chunks.len(), 0);
        assert!((deserialized.relevance_score - 0.75).abs() < 1e-6);
        assert_eq!(deserialized.causal_id, cid);
    }
}
