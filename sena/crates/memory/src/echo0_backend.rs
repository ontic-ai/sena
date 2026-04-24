//! ech0-based memory backend.
//!
//! BONES stub: the `Echo0Backend` struct satisfies the `MemoryBackend` trait
//! interface without making any real ech0 calls. The full implementation will
//! be wired to an `ech0::Store` once the encryption layer and embedder are
//! integrated.

use crate::backend::{MemoryBackend, MemoryStats};
use crate::error::MemoryError;
use async_trait::async_trait;
use bus::CausalId;
use bus::events::{MemoryKind, ScoredChunk};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::debug;

/// In-memory chunk with metadata for BONES testing.
#[derive(Clone)]
#[allow(dead_code)] // kind and causal_id reserved for future diagnostic use
struct MemoryChunk {
    text: String,
    kind: MemoryKind,
    causal_id: CausalId,
    timestamp: u64,
    importance: f32,
}

/// ech0-based memory backend.
///
/// Owns an `ech0::Store` for semantic storage and retrieval.  
/// The BONES implementation maintains in-memory chunks with decay/pruning
/// to provide real consolidation behavior for testing.
pub struct Echo0Backend {
    /// In-memory chunks with metadata.
    chunks: Vec<MemoryChunk>,
    /// Decay rate per consolidation cycle (0.0 to 1.0).
    decay_rate: f32,
    /// Minimum importance threshold for pruning.
    prune_threshold: f32,
}

impl Echo0Backend {
    /// Create a new (stub) ech0 backend.
    pub fn new() -> Self {
        Self {
            chunks: Vec::new(),
            decay_rate: 0.1,      // 10% decay per cycle
            prune_threshold: 0.2, // Prune chunks below 0.2 importance
        }
    }

    /// Get current time as Unix timestamp.
    fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}

impl Default for Echo0Backend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MemoryBackend for Echo0Backend {
    async fn ingest(
        &mut self,
        text: &str,
        kind: MemoryKind,
        causal_id: CausalId,
    ) -> Result<(), MemoryError> {
        debug!(
            text_len = text.len(),
            ?kind,
            ?causal_id,
            "Echo0Backend: ingest (in-memory chunk stored)"
        );

        self.chunks.push(MemoryChunk {
            text: text.to_string(),
            kind,
            causal_id,
            timestamp: Self::now(),
            importance: 1.0, // New chunks start at maximum importance
        });

        Ok(())
    }

    async fn query(&self, query: &str, limit: usize) -> Result<Vec<ScoredChunk>, MemoryError> {
        debug!(
            query_len = query.len(),
            limit,
            chunk_count = self.chunks.len(),
            "Echo0Backend: query (simple text matching)"
        );

        // Simple text matching for BONES: score chunks by substring presence
        let mut scored: Vec<_> = self
            .chunks
            .iter()
            .filter_map(|chunk| {
                // Simple scoring: 1.0 if query substring found, scaled by importance
                if chunk.text.to_lowercase().contains(&query.to_lowercase()) {
                    let age_seconds = Self::now().saturating_sub(chunk.timestamp);
                    Some(ScoredChunk {
                        content: chunk.text.clone(),
                        score: chunk.importance,
                        age_seconds,
                    })
                } else {
                    None
                }
            })
            .collect();

        // Sort by score descending
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Limit results
        scored.truncate(limit);

        Ok(scored)
    }

    async fn stats(&self) -> Result<MemoryStats, MemoryError> {
        Ok(MemoryStats {
            working_memory_chunks: 0,
            long_term_memory_nodes: self.chunks.len(),
        })
    }

    async fn consolidate(&mut self) -> Result<usize, MemoryError> {
        let initial_count = self.chunks.len();

        // Decay importance scores for all chunks
        for chunk in &mut self.chunks {
            chunk.importance *= 1.0 - self.decay_rate;
        }

        // Prune chunks below threshold
        self.chunks
            .retain(|chunk| chunk.importance >= self.prune_threshold);

        let pruned_count = initial_count.saturating_sub(self.chunks.len());
        let affected = initial_count; // All chunks were decayed

        debug!(
            decayed = affected,
            pruned = pruned_count,
            remaining = self.chunks.len(),
            "Echo0Backend: consolidation completed"
        );

        Ok(affected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bus::CausalId;
    use bus::events::MemoryKind;

    #[tokio::test]
    async fn ingest_stores_chunk() {
        let mut backend = Echo0Backend::new();
        let result = backend
            .ingest("hello world", MemoryKind::Episodic, CausalId::new())
            .await;
        assert!(result.is_ok());
        assert_eq!(backend.chunks.len(), 1);
    }

    #[tokio::test]
    async fn query_returns_matching_chunks() {
        let mut backend = Echo0Backend::new();
        backend
            .ingest("hello world", MemoryKind::Episodic, CausalId::new())
            .await
            .expect("ingest failed");
        backend
            .ingest("goodbye world", MemoryKind::Episodic, CausalId::new())
            .await
            .expect("ingest failed");
        backend
            .ingest("unrelated text", MemoryKind::Episodic, CausalId::new())
            .await
            .expect("ingest failed");

        let results = backend.query("world", 10).await.expect("query failed");
        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|c| c.content.contains("hello")));
        assert!(results.iter().any(|c| c.content.contains("goodbye")));
    }

    #[tokio::test]
    async fn query_respects_limit() {
        let mut backend = Echo0Backend::new();
        for i in 0..10 {
            backend
                .ingest(
                    &format!("text {}", i),
                    MemoryKind::Episodic,
                    CausalId::new(),
                )
                .await
                .expect("ingest failed");
        }

        let results = backend.query("text", 3).await.expect("query failed");
        assert_eq!(results.len(), 3);
    }

    #[tokio::test]
    async fn consolidate_decays_importance() {
        let mut backend = Echo0Backend::new();
        backend
            .ingest("test chunk", MemoryKind::Episodic, CausalId::new())
            .await
            .expect("ingest failed");

        let initial_importance = backend.chunks[0].importance;
        assert_eq!(initial_importance, 1.0);

        backend.consolidate().await.expect("consolidate failed");

        let decayed_importance = backend.chunks[0].importance;
        assert!(decayed_importance < initial_importance);
        assert_eq!(decayed_importance, 0.9); // 1.0 * (1.0 - 0.1)
    }

    #[tokio::test]
    async fn consolidate_prunes_low_importance_chunks() {
        let mut backend = Echo0Backend::new();
        backend
            .ingest("chunk1", MemoryKind::Episodic, CausalId::new())
            .await
            .expect("ingest failed");
        backend
            .ingest("chunk2", MemoryKind::Episodic, CausalId::new())
            .await
            .expect("ingest failed");

        assert_eq!(backend.chunks.len(), 2);

        // Run consolidation 16 times to decay below threshold (0.9^16 ≈ 0.185 < 0.2)
        for _ in 0..16 {
            backend.consolidate().await.expect("consolidate failed");
        }

        // Chunks should be pruned (importance < 0.2)
        assert_eq!(backend.chunks.len(), 0);
    }

    #[tokio::test]
    async fn consolidate_returns_affected_count() {
        let mut backend = Echo0Backend::new();
        backend
            .ingest("chunk1", MemoryKind::Episodic, CausalId::new())
            .await
            .expect("ingest failed");
        backend
            .ingest("chunk2", MemoryKind::Episodic, CausalId::new())
            .await
            .expect("ingest failed");

        let affected = backend.consolidate().await.expect("consolidate failed");
        assert_eq!(affected, 2); // Both chunks were decayed
    }

    #[tokio::test]
    async fn query_scores_reflect_importance() {
        let mut backend = Echo0Backend::new();
        backend
            .ingest("test chunk", MemoryKind::Episodic, CausalId::new())
            .await
            .expect("ingest failed");

        // Query before decay
        let results_before = backend.query("test", 10).await.expect("query failed");
        assert_eq!(results_before.len(), 1);
        assert_eq!(results_before[0].score, 1.0);

        // Consolidate to decay
        backend.consolidate().await.expect("consolidate failed");

        // Query after decay
        let results_after = backend.query("test", 10).await.expect("query failed");
        assert_eq!(results_after.len(), 1);
        assert_eq!(results_after[0].score, 0.9);
    }
}
