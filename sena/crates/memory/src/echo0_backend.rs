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
use tracing::{debug, info, warn};

const NODES_TABLE: TableDefinition<u64, &[u8]> = TableDefinition::new("memory_nodes");
const META_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("memory_meta");
const META_NEXT_NODE_ID_KEY: &str = "next_node_id";
const DEFAULT_PRUNE_THRESHOLD: f32 = 0.2;
const DEFAULT_MIN_RETRIEVAL_SIMILARITY: f32 = 0.65;

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

fn is_zero_vector(vector: &[f32]) -> bool {
    !vector.is_empty() && vector.iter().all(|value| *value == 0.0)
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

impl MemoryNode {
    fn has_embedding(&self) -> bool {
        self.embedding.len() == EMBEDDING_DIMENSIONS && !is_zero_vector(&self.embedding)
    }
}

/// Persistent redb-backed memory store.
pub struct PersistentMemoryStore {
    path: PathBuf,
    db: Database,
    embedder: SenaEmbedder,
    decay_rate: f32,
    prune_threshold: f32,
    min_retrieval_similarity: f32,
}

pub type Echo0Backend = PersistentMemoryStore;

impl PersistentMemoryStore {
    pub fn open(path: &Path, embedder: SenaEmbedder) -> Result<Self, MemoryError> {
        Self::open_with_thresholds(
            path,
            embedder,
            DEFAULT_PRUNE_THRESHOLD,
            DEFAULT_MIN_RETRIEVAL_SIMILARITY,
        )
    }

    pub fn open_with_prune_threshold(
        path: &Path,
        embedder: SenaEmbedder,
        prune_threshold: f32,
    ) -> Result<Self, MemoryError> {
        Self::open_with_thresholds(
            path,
            embedder,
            prune_threshold,
            DEFAULT_MIN_RETRIEVAL_SIMILARITY,
        )
    }

    pub fn open_with_thresholds(
        path: &Path,
        embedder: SenaEmbedder,
        prune_threshold: f32,
        min_retrieval_similarity: f32,
    ) -> Result<Self, MemoryError> {
        if !(0.0..=1.0).contains(&prune_threshold) {
            return Err(MemoryError::BackendError(format!(
                "prune_threshold must be between 0.0 and 1.0, got {prune_threshold}"
            )));
        }

        if !(0.0..=1.0).contains(&min_retrieval_similarity) {
            return Err(MemoryError::BackendError(format!(
                "min_retrieval_similarity must be between 0.0 and 1.0, got {min_retrieval_similarity}"
            )));
        }

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
            prune_threshold,
            min_retrieval_similarity,
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

    #[cfg(test)]
    pub fn with_embedder_and_prune_threshold(
        embedder: SenaEmbedder,
        prune_threshold: f32,
    ) -> Result<Self, MemoryError> {
        let path = std::env::temp_dir().join(format!("sena-memory-{}.redb", uuid::Uuid::new_v4()));
        Self::open_with_prune_threshold(&path, embedder, prune_threshold)
    }

    #[cfg(test)]
    pub fn with_embedder_and_thresholds(
        embedder: SenaEmbedder,
        prune_threshold: f32,
        min_retrieval_similarity: f32,
    ) -> Result<Self, MemoryError> {
        let path = std::env::temp_dir().join(format!("sena-memory-{}.redb", uuid::Uuid::new_v4()));
        Self::open_with_thresholds(
            &path,
            embedder,
            prune_threshold,
            min_retrieval_similarity,
        )
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

    fn fallback_recency_chunks(mut nodes: Vec<MemoryNode>, limit: usize) -> Vec<ScoredChunk> {
        let now = Self::now();

        nodes.sort_by(|left, right| {
            right.timestamp.cmp(&left.timestamp).then_with(|| {
                right
                    .importance
                    .partial_cmp(&left.importance)
                    .unwrap_or(Ordering::Equal)
            })
        });

        nodes
            .into_iter()
            .take(limit)
            .map(|node| ScoredChunk {
                content: node.text,
                score: node.importance.clamp(0.0, 1.0),
                age_seconds: now.saturating_sub(node.timestamp),
            })
            .collect()
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

        let embedding = match self.embedder.embed(text).await {
            Ok(vector) if vector.len() == EMBEDDING_DIMENSIONS && !is_zero_vector(&vector) => {
                vector
            }
            Ok(vector) => {
                warn!(
                    expected = EMBEDDING_DIMENSIONS,
                    actual = vector.len(),
                    zero_vector = is_zero_vector(&vector),
                    "embedding unavailable or unsupported; storing node without semantic vector"
                );
                Vec::new()
            }
            Err(error) => {
                warn!(
                    error = %error,
                    "embedding request failed; storing node without semantic vector"
                );
                Vec::new()
            }
        };

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

        let nodes = self.load_nodes()?;
        if nodes.is_empty() {
            return Ok(Vec::new());
        }

        let has_embeddings = nodes.iter().any(MemoryNode::has_embedding);
        let query_embedding = match self.embedder.embed(query).await {
            Ok(vector) if vector.len() == EMBEDDING_DIMENSIONS && !is_zero_vector(&vector) => {
                Some(vector)
            }
            Ok(vector) => {
                warn!(
                    expected = EMBEDDING_DIMENSIONS,
                    actual = vector.len(),
                    zero_vector = is_zero_vector(&vector),
                    "query embedding unavailable; falling back to recency retrieval"
                );
                None
            }
            Err(error) => {
                warn!(
                    error = %error,
                    "query embedding failed; falling back to recency retrieval"
                );
                None
            }
        };

        let Some(query_embedding) = query_embedding else {
            return Ok(Self::fallback_recency_chunks(nodes, limit));
        };

        if !has_embeddings {
            return Ok(Self::fallback_recency_chunks(nodes, limit));
        }

        let now = Self::now();
        let mut scored: Vec<_> = nodes
            .into_iter()
            .filter(MemoryNode::has_embedding)
            .filter_map(|node| {
                let similarity = cosine_similarity(&query_embedding, &node.embedding);
                if similarity < self.min_retrieval_similarity {
                    return None;
                }

                Some(ScoredChunk {
                    content: node.text,
                    score: (similarity * node.importance).clamp(0.0, 1.0),
                    age_seconds: now.saturating_sub(node.timestamp),
                })
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

    async fn clear(&mut self) -> Result<(), MemoryError> {
        self.replace_nodes(&[])
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

    fn spawn_zero_embed_sender() -> mpsc::Sender<EmbedRequest> {
        let (embed_tx, mut embed_rx) = mpsc::channel::<EmbedRequest>(8);
        tokio::spawn(async move {
            while let Some(request) = embed_rx.recv().await {
                let _ = request
                    .response_tx
                    .send(Ok(vec![0.0; EMBEDDING_DIMENSIONS]));
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
        let initial_score = results_before[0].score;
        assert!(initial_score > 0.0);

        // Consolidate to decay
        backend.consolidate().await.expect("consolidate failed");

        // Query after decay
        let results_after = backend.query("test", 10).await.expect("query failed");
        assert_eq!(results_after.len(), 1);
        assert!((results_after[0].score - (initial_score * 0.9)).abs() < f32::EPSILON);
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
    async fn clear_removes_all_nodes() {
        let temp_dir = tempdir().expect("failed to create temp dir");
        let mut backend = build_backend(&temp_dir);
        backend
            .ingest("hello world", MemoryKind::Episodic, CausalId::new())
            .await
            .expect("ingest failed");

        backend.clear().await.expect("clear failed");

        assert!(backend.load_nodes().expect("load nodes failed").is_empty());
    }

    #[tokio::test]
    async fn custom_prune_threshold_is_respected() {
        let (embed_tx, _embed_rx) = mpsc::channel::<EmbedRequest>(8);
        let embedder = SenaEmbedder::new(embed_tx);
        let mut backend = PersistentMemoryStore::with_embedder_and_prune_threshold(embedder, 0.95)
            .expect("backend should build");

        backend
            .ingest("important chunk", MemoryKind::Episodic, CausalId::new())
            .await
            .expect("ingest failed");

        backend.consolidate().await.expect("consolidate failed");

        assert!(backend.load_nodes().expect("load nodes failed").is_empty());
    }

    #[tokio::test]
    async fn semantic_query_respects_min_retrieval_similarity() {
        let embedder = SenaEmbedder::new(spawn_embed_sender());
        let mut backend = PersistentMemoryStore::with_embedder_and_thresholds(embedder, 0.2, 0.8)
            .expect("backend should build");

        backend
            .ingest("rust coding", MemoryKind::Semantic, CausalId::new())
            .await
            .expect("ingest failed");
        backend
            .ingest("rust", MemoryKind::Semantic, CausalId::new())
            .await
            .expect("ingest failed");

        let results = backend
            .query_semantic("rust coding", 5)
            .await
            .expect("query failed");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "rust coding");
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

    #[tokio::test]
    async fn ingest_without_embeddings_still_persists_node() {
        let temp_dir = tempdir().expect("failed to create temp dir");
        let mut backend = PersistentMemoryStore::open(
            &temp_dir.path().join("memory.redb"),
            SenaEmbedder::disconnected(),
        )
        .expect("store should open");

        backend
            .ingest("first memory", MemoryKind::Episodic, CausalId::new())
            .await
            .expect("ingest should succeed without embeddings");

        let nodes = backend.load_nodes().expect("load nodes failed");
        assert_eq!(nodes.len(), 1);
        assert!(nodes[0].embedding.is_empty());

        let results = backend
            .query_semantic("anything", 5)
            .await
            .expect("query should fall back to recency");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "first memory");
    }

    #[tokio::test]
    async fn zero_vector_embeddings_are_stored_without_semantic_vector() {
        let temp_dir = tempdir().expect("failed to create temp dir");
        let embedder = SenaEmbedder::new(spawn_zero_embed_sender());
        let mut backend =
            PersistentMemoryStore::open(&temp_dir.path().join("memory.redb"), embedder)
                .expect("store should open");

        backend
            .ingest("fallback memory", MemoryKind::Semantic, CausalId::new())
            .await
            .expect("ingest should succeed");

        let nodes = backend.load_nodes().expect("load nodes failed");
        assert_eq!(nodes.len(), 1);
        assert!(nodes[0].embedding.is_empty());
    }
}
