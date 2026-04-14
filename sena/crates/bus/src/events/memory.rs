//! Memory-layer events: ingest, query, retrieval.

use crate::causal::CausalId;
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

/// Memory subsystem events.
#[derive(Debug, Clone)]
pub enum MemoryEvent {
    /// Request to ingest text into long-term memory.
    WriteRequested {
        text: String,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Write to long-term memory completed.
    WriteCompleted {
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Request to query long-term memory.
    QueryRequested {
        query: String,
        token_budget: usize,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Response to a memory query.
    QueryCompleted {
        chunks: Vec<MemoryChunk>,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Memory operation failed.
    OperationFailed {
        reason: String,
        /// Causal chain ID.
        causal_id: CausalId,
    },
}

impl MemoryEvent {
    pub fn causal_id(&self) -> Option<CausalId> {
        match self {
            Self::WriteRequested { causal_id, .. }
            | Self::WriteCompleted { causal_id, .. }
            | Self::QueryRequested { causal_id, .. }
            | Self::QueryCompleted { causal_id, .. }
            | Self::OperationFailed { causal_id, .. } => Some(*causal_id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_event_causal_id_extraction() {
        let cid = CausalId::new();
        let event = MemoryEvent::WriteRequested {
            text: "test".to_string(),
            causal_id: cid,
        };
        assert_eq!(event.causal_id(), Some(cid));
    }

    #[test]
    fn memory_chunk_debug_redacts_content() {
        let chunk = MemoryChunk {
            text: "sensitive data".to_string(),
            score: 0.9,
            timestamp: SystemTime::now(),
        };
        let debug_str = format!("{:?}", chunk);
        assert!(debug_str.contains("REDACTED"));
        assert!(!debug_str.contains("sensitive data"));
    }
}
