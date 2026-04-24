//! Memory-layer events: ingest, query, retrieval.

use crate::causal::CausalId;
use std::path::PathBuf;

/// Kind of memory being stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
#[derive(Clone)]
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
            | Self::BackupCompleted { causal_id, .. }
            | Self::BackupFailed { causal_id, .. } => Some(*causal_id),
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
}
