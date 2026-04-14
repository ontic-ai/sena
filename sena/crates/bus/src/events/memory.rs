//! Memory-layer events: ingest, query, retrieval.

use crate::causal::CausalId;

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

    /// Request to query long-term memory using semantic embedding.
    QueryRequested {
        embedding: Vec<f32>,
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
}

impl std::fmt::Debug for MemoryEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IngestRequested {
                kind, causal_id, ..
            } => f
                .debug_struct("IngestRequested")
                .field("text", &"[REDACTED]")
                .field("kind", kind)
                .field("causal_id", causal_id)
                .finish(),
            Self::IngestCompleted { causal_id } => f
                .debug_struct("IngestCompleted")
                .field("causal_id", causal_id)
                .finish(),
            Self::IngestFailed { causal_id, reason } => f
                .debug_struct("IngestFailed")
                .field("causal_id", causal_id)
                .field("reason", reason)
                .finish(),
            Self::QueryRequested {
                embedding,
                limit,
                causal_id,
            } => f
                .debug_struct("QueryRequested")
                .field("embedding", &format!("[{} dims]", embedding.len()))
                .field("limit", limit)
                .field("causal_id", causal_id)
                .finish(),
            Self::QueryCompleted { chunks, causal_id } => f
                .debug_struct("QueryCompleted")
                .field("chunks", chunks)
                .field("causal_id", causal_id)
                .finish(),
            Self::QueryFailed { causal_id, reason } => f
                .debug_struct("QueryFailed")
                .field("causal_id", causal_id)
                .field("reason", reason)
                .finish(),
        }
    }
}

impl MemoryEvent {
    pub fn causal_id(&self) -> Option<CausalId> {
        match self {
            Self::IngestRequested { causal_id, .. }
            | Self::IngestCompleted { causal_id, .. }
            | Self::IngestFailed { causal_id, .. }
            | Self::QueryRequested { causal_id, .. }
            | Self::QueryCompleted { causal_id, .. }
            | Self::QueryFailed { causal_id, .. } => Some(*causal_id),
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
