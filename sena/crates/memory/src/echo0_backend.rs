//! redb-backed persistent memory backend.

use crate::backend::{MemoryBackend, MemoryStats};
use crate::embedder::{EMBEDDING_DIMENSIONS, SenaEmbedder};
use crate::error::MemoryError;
use async_trait::async_trait;
use bus::CausalId;
use bus::events::{MemoryKind, ScoredChunk};
use ech0::traits::Embedder;
use redb::{Database, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, info};

const NODES_TABLE: TableDefinition<u64, &[u8]> = TableDefinition::new("memory_nodes");
const META_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("memory_meta");
const META_NEXT_NODE_ID_KEY: &str = "next_node_id";

fn db_error(error: impl std::fmt::Display) -> MemoryError {
    MemoryError::BackendError(error.to_string())
}

fn parse_u64(bytes: &[u8]) -> Result<u64, MemoryError> {
    if bytes.len() != 8 {
        return Err(MemoryError::BackendError(
            "invalid u64 metadata payload".to_string(),
        ));
    }

    let mut raw = [0_u8; 8];
    raw.copy_from_slice(bytes);
    Ok(u64::from_le_bytes(raw))
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> f32 {
    if left.len() != right.len() || left.is_empty() {
        return 0.0;
    }

    let mut dot = 0.0_f32;
    let mut left_norm = 0.0_f32;
    let mut right_norm = 0.0_f32;

    for (lhs, rhs) in left.iter().zip(right.iter()) {
        dot += lhs * rhs;
        left_norm += lhs * lhs;
        right_norm += rhs * rhs;
    }

    if left_norm == 0.0 || right_norm == 0.0 {
        return 0.0;
    }

    let similarity = dot / (left_norm.sqrt() * right_norm.sqrt());
    similarity.clamp(0.0, 1.0)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryNode {
    pub id: u64,
    pub text: String,
    pub embedding: Vec<f32>,
    pub importance: f32,
    pub kind: MemoryKind,
    pub timestamp: u64,
    pub causal_id: u64,
}

/// Persistent redb-backed memory store.
pub struct PersistentMemoryStore {
    path: PathBuf,
    db: Database,
    embedder: SenaEmbedder,
    decay_rate: f32,
    prune_threshold: f32,
}

pub type Echo0Backend = PersistentMemoryStore;

impl PersistentMemoryStore {
    pub fn open(path: &Path, embedder: SenaEmbedder) -> Result<Self, MemoryError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                MemoryError::BackendError(format!("failed to create memory dir: {e}"))
            })?;
        }

        let db = if path.exists() {
            Database::open(path).map_err(db_error)?
        } else {
            Database::create(path).map_err(db_error)?
        };

        let store = Self {
            path: path.to_path_buf(),
            db,
            embedder,
            decay_rate: 0.1,
            prune_threshold: 0.2,
        };
        store.ensure_tables()?;
        info!(path = %store.path.display(), "persistent memory store initialized");
        Ok(store)
    }

    #[cfg(test)]
    pub fn with_embedder(embedder: SenaEmbedder) -> Result<Self, MemoryError> {
        let path = std::env::temp_dir().join(format!("sena-memory-{}.redb", uuid::Uuid::new_v4()));
        Self::open(&path, embedder)
    }

    fn ensure_tables(&self) -> Result<(), MemoryError> {
        let write_txn = self.db.begin_write().map_err(db_error)?;
        {
            write_txn.open_table(NODES_TABLE).map_err(db_error)?;
            let mut meta_table = write_txn.open_table(META_TABLE).map_err(db_error)?;

            if meta_table
                .get(META_NEXT_NODE_ID_KEY)
                .map_err(db_error)?
                .is_none()
            {
                meta_table
                    .insert(META_NEXT_NODE_ID_KEY, 1_u64.to_le_bytes().as_slice())
                    .map_err(db_error)?;
            }
        }
        write_txn.commit().map_err(db_error)?;
        Ok(())
    }

    fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    fn next_node_id(&self) -> Result<u64, MemoryError> {
        let read_txn = self.db.begin_read().map_err(db_error)?;
        let meta_table = read_txn.open_table(META_TABLE).map_err(db_error)?;
        let value = meta_table
            .get(META_NEXT_NODE_ID_KEY)
            .map_err(db_error)?
            .ok_or_else(|| {
                MemoryError::BackendError("missing next node id metadata".to_string())
            })?;
        parse_u64(value.value())
    }

    fn load_nodes(&self) -> Result<Vec<MemoryNode>, MemoryError> {
        let read_txn = self.db.begin_read().map_err(db_error)?;
        let table = read_txn.open_table(NODES_TABLE).map_err(db_error)?;
        let mut nodes = Vec::new();

        for entry in table.iter().map_err(db_error)? {
            let (_, value) = entry.map_err(db_error)?;
            let node = serde_json::from_slice(value.value()).map_err(|e| {
                MemoryError::BackendError(format!("failed to decode memory node: {e}"))
            })?;
            nodes.push(node);
        }

        Ok(nodes)
    }

    fn collect_node_ids(&self) -> Result<Vec<u64>, MemoryError> {
        let read_txn = self.db.begin_read().map_err(db_error)?;
        let table = read_txn.open_table(NODES_TABLE).map_err(db_error)?;
        let mut ids = Vec::new();

        for entry in table.iter().map_err(db_error)? {
            let (key, _) = entry.map_err(db_error)?;
            ids.push(key.value());
        }

        Ok(ids)
    }

    fn write_node(&self, node: &MemoryNode) -> Result<(), MemoryError> {
        let payload = serde_json::to_vec(node)
            .map_err(|e| MemoryError::BackendError(format!("failed to serialize node: {e}")))?;
        let write_txn = self.db.begin_write().map_err(db_error)?;

        {
            let mut table = write_txn.open_table(NODES_TABLE).map_err(db_error)?;
            table
                .insert(node.id, payload.as_slice())
                .map_err(db_error)?;
        }

        {
            let mut meta_table = write_txn.open_table(META_TABLE).map_err(db_error)?;
            meta_table
                .insert(
                    META_NEXT_NODE_ID_KEY,
                    node.id.saturating_add(1).to_le_bytes().as_slice(),
                )
                .map_err(db_error)?;
        }

        write_txn.commit().map_err(db_error)?;
        Ok(())
    }

    fn replace_nodes(&self, nodes: &[MemoryNode]) -> Result<(), MemoryError> {
        let node_ids = self.collect_node_ids()?;
        let serialized_nodes: Vec<(u64, Vec<u8>)> = nodes
            .iter()
            .map(|node| {
                serde_json::to_vec(node)
                    .map(|payload| (node.id, payload))
                    .map_err(|e| {
                        MemoryError::BackendError(format!("failed to serialize node: {e}"))
                    })
            })
            .collect::<Result<_, _>>()?;

        let write_txn = self.db.begin_write().map_err(db_error)?;
        {
            let mut table = write_txn.open_table(NODES_TABLE).map_err(db_error)?;
            for id in node_ids {
                table.remove(id).map_err(db_error)?;
            }

            for (id, payload) in serialized_nodes {
                table.insert(id, payload.as_slice()).map_err(db_error)?;
            }
        }
        write_txn.commit().map_err(db_error)?;
        Ok(())
    }

    pub async fn ingest(
        &mut self,
        text: &str,
        kind: MemoryKind,
        causal_id: CausalId,
    ) -> Result<(), MemoryError> {
        debug!(
            text_len = text.len(),
            ?kind,
            causal_id = causal_id.as_u64(),
            "persistent memory ingest requested"
        );

        let embedding = self
            .embedder
            .embed(text)
            .await
            .map_err(|e| MemoryError::InvalidEmbedding(e.to_string()))?;

        if embedding.len() != EMBEDDING_DIMENSIONS {
            return Err(MemoryError::InvalidEmbedding(format!(
                "expected {}-dim embedding, got {}",
                EMBEDDING_DIMENSIONS,
                embedding.len()
            )));
        }

        let node = MemoryNode {
            id: self.next_node_id()?,
            text: text.to_string(),
            embedding,
            importance: 1.0,
            kind,
            timestamp: Self::now(),
            causal_id: causal_id.as_u64(),
        };

        self.write_node(&node)?;
        Ok(())
    }

    pub async fn query_semantic(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<ScoredChunk>, MemoryError> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }

        let query_embedding = self
            .embedder
            .embed(query)
            .await
            .map_err(|e| MemoryError::InvalidEmbedding(e.to_string()))?;
        let query_lower = query.to_lowercase();
        let now = Self::now();
        let mut scored: Vec<_> = self
            .load_nodes()?
            .into_iter()
            .map(|node| {
                let similarity = cosine_similarity(&query_embedding, &node.embedding);
                let lexical_match = node.text.to_lowercase().contains(&query_lower);
                ScoredChunk {
                    content: node.text,
                    score: if lexical_match {
                        node.importance
                    } else {
                        (similarity * node.importance).clamp(0.0, 1.0)
                    },
                    age_seconds: now.saturating_sub(node.timestamp),
                }
            })
            .filter(|chunk| chunk.score > 0.0)
            .collect();

        scored.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(Ordering::Equal)
        });
        scored.truncate(limit);
        Ok(scored)
    }

    pub async fn decay_and_prune(&mut self) -> Result<usize, MemoryError> {
        let mut nodes = self.load_nodes()?;
        let affected = nodes.len();

        for node in &mut nodes {
            node.importance *= 1.0 - self.decay_rate;
        }

        nodes.retain(|node| node.importance >= self.prune_threshold);
        self.replace_nodes(&nodes)?;

        debug!(
            affected,
            remaining = nodes.len(),
            "persistent memory consolidation completed"
        );

        Ok(affected)
    }

    pub async fn export_json(&self, path: &Path) -> Result<(), MemoryError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                MemoryError::BackendError(format!("failed to create backup directory: {e}"))
            })?;
        }

        let payload = serde_json::to_string_pretty(&self.load_nodes()?)
            .map_err(|e| MemoryError::BackendError(format!("failed to serialize export: {e}")))?;
        std::fs::write(path, payload)
            .map_err(|e| MemoryError::BackendError(format!("failed to write export: {e}")))?;
        Ok(())
    }
}

#[async_trait]
impl MemoryBackend for PersistentMemoryStore {
    async fn ingest(
        &mut self,
        text: &str,
        kind: MemoryKind,
        causal_id: CausalId,
    ) -> Result<(), MemoryError> {
        PersistentMemoryStore::ingest(self, text, kind, causal_id).await
    }

    async fn query(&self, query: &str, limit: usize) -> Result<Vec<ScoredChunk>, MemoryError> {
        self.query_semantic(query, limit).await
    }

    async fn stats(&self) -> Result<MemoryStats, MemoryError> {
        Ok(MemoryStats {
            working_memory_chunks: 0,
            long_term_memory_nodes: self.load_nodes()?.len(),
        })
    }

    async fn consolidate(&mut self) -> Result<usize, MemoryError> {
        self.decay_and_prune().await
    }

    async fn export_json(&self, path: PathBuf) -> Result<(), MemoryError> {
        PersistentMemoryStore::export_json(self, path.as_path()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedder::{EMBEDDING_DIMENSIONS, SenaEmbedder};
    use bus::CausalId;
    use bus::events::MemoryKind;
    use inference::EmbedRequest;
    use tempfile::tempdir;
    use tokio::sync::mpsc;

    fn test_embedding(text: &str) -> Vec<f32> {
        let mut vector = vec![0.0_f32; EMBEDDING_DIMENSIONS];
        for token in text.to_lowercase().split_whitespace() {
            let slot = match token {
                "rust" => 0,
                "world" => 1,
                "important" => 2,
                "coding" | "code" => 3,
                other => {
                    4 + (other.bytes().fold(0_u64, |acc, byte| acc + byte as u64) as usize
                        % (EMBEDDING_DIMENSIONS.saturating_sub(4).max(1)))
                }
            };
            vector[slot] += 1.0;
        }

        if vector.iter().all(|value| *value == 0.0) {
            vector[EMBEDDING_DIMENSIONS - 1] = 1.0;
        }

        let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
        if norm > 0.0 {
            for value in &mut vector {
                *value /= norm;
            }
        }

        vector
    }

    fn spawn_embed_sender() -> mpsc::Sender<EmbedRequest> {
        let (embed_tx, mut embed_rx) = mpsc::channel::<EmbedRequest>(8);
        tokio::spawn(async move {
            while let Some(request) = embed_rx.recv().await {
                let _ = request.response_tx.send(Ok(test_embedding(&request.text)));
            }
        });
        embed_tx
    }

    fn build_backend(temp_dir: &tempfile::TempDir) -> PersistentMemoryStore {
        let embedder = SenaEmbedder::new(spawn_embed_sender());
        PersistentMemoryStore::open(&temp_dir.path().join("memory.redb"), embedder)
            .expect("persistent memory store should open")
    }

    #[tokio::test]
    async fn ingest_stores_chunk() {
        let temp_dir = tempdir().expect("failed to create temp dir");
        let mut backend = build_backend(&temp_dir);
        let result = backend
            .ingest("hello world", MemoryKind::Episodic, CausalId::new())
            .await;
        assert!(result.is_ok());
        assert_eq!(backend.load_nodes().expect("load nodes failed").len(), 1);
    }

    #[tokio::test]
    async fn query_returns_matching_chunks() {
        let temp_dir = tempdir().expect("failed to create temp dir");
        let mut backend = build_backend(&temp_dir);
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
        let temp_dir = tempdir().expect("failed to create temp dir");
        let mut backend = build_backend(&temp_dir);
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
        let temp_dir = tempdir().expect("failed to create temp dir");
        let mut backend = build_backend(&temp_dir);
        backend
            .ingest("test chunk", MemoryKind::Episodic, CausalId::new())
            .await
            .expect("ingest failed");

        let initial_importance = backend.load_nodes().expect("load nodes failed")[0].importance;
        assert_eq!(initial_importance, 1.0);

        backend.consolidate().await.expect("consolidate failed");

        let decayed_importance = backend.load_nodes().expect("load nodes failed")[0].importance;
        assert!(decayed_importance < initial_importance);
        assert_eq!(decayed_importance, 0.9); // 1.0 * (1.0 - 0.1)
    }

    #[tokio::test]
    async fn consolidate_prunes_low_importance_chunks() {
        let temp_dir = tempdir().expect("failed to create temp dir");
        let mut backend = build_backend(&temp_dir);
        backend
            .ingest("chunk1", MemoryKind::Episodic, CausalId::new())
            .await
            .expect("ingest failed");
        backend
            .ingest("chunk2", MemoryKind::Episodic, CausalId::new())
            .await
            .expect("ingest failed");

        assert_eq!(backend.load_nodes().expect("load nodes failed").len(), 2);

        // Run consolidation 16 times to decay below threshold (0.9^16 ≈ 0.185 < 0.2)
        for _ in 0..16 {
            backend.consolidate().await.expect("consolidate failed");
        }

        // Chunks should be pruned (importance < 0.2)
        assert_eq!(backend.load_nodes().expect("load nodes failed").len(), 0);
    }

    #[tokio::test]
    async fn consolidate_returns_affected_count() {
        let temp_dir = tempdir().expect("failed to create temp dir");
        let mut backend = build_backend(&temp_dir);
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
        let temp_dir = tempdir().expect("failed to create temp dir");
        let mut backend = build_backend(&temp_dir);
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

    #[tokio::test]
    async fn export_json_writes_real_snapshot() {
        let temp_dir = tempdir().expect("failed to create temp dir");
        let mut backend = build_backend(&temp_dir);
        let export_path = temp_dir.path().join("backup").join("memory.json");

        backend
            .ingest("hello world", MemoryKind::Episodic, CausalId::new())
            .await
            .expect("ingest failed");

        backend
            .export_json(export_path.as_path())
            .await
            .expect("export failed");

        let exported = std::fs::read_to_string(export_path).expect("read export failed");
        assert!(exported.contains("hello world"));
        assert!(exported.contains("embedding"));
    }

    #[tokio::test]
    async fn store_persists_across_reopen() {
        let temp_dir = tempdir().expect("failed to create temp dir");
        let store_path = temp_dir.path().join("memory.redb");

        {
            let embedder = SenaEmbedder::new(spawn_embed_sender());
            let mut backend =
                PersistentMemoryStore::open(&store_path, embedder).expect("store should open");
            backend
                .ingest("rust coding world", MemoryKind::Semantic, CausalId::new())
                .await
                .expect("ingest failed");
        }

        let embedder = SenaEmbedder::new(spawn_embed_sender());
        let backend =
            PersistentMemoryStore::open(&store_path, embedder).expect("store should reopen");
        let results = backend
            .query_semantic("rust world", 5)
            .await
            .expect("query failed");

        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("rust coding world"));
    }
}
