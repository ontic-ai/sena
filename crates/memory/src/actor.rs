//! Memory actor — owns the ech0 Store, handles memory ingest and queries.
//!
//! The `MemoryActor` is the sole owner of `ech0::Store`. All external interaction
//! goes through typed bus events:
//!
//! - `MemoryEvent::WriteRequested`          → `store.ingest_text()`
//! - `MemoryEvent::SemanticIngestRequested` → `store.ingest_text()` with routing prefix
//! - `MemoryEvent::QueryRequested`          → `store.search()`
//! - `MemoryEvent::ConflictDetected`        → (broadcast, outbound only)
//! - `MemoryEvent::QueryCompleted`          → (broadcast, outbound only)
//! - `TransparencyQuery::UserMemory`        → transparency query handler

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use async_trait::async_trait;
use bus::events::memory::{
    MemoryChunk, MemoryConflictDetected, MemoryConsolidationCompleted, MemoryQueryRequest,
    MemoryQueryResponse, MemoryWriteRequest, SemanticIngestComplete, SemanticIngestRequest,
};
use bus::events::transparency::TransparencyQuery;
use bus::{Actor, ActorError, Event, EventBus, MemoryEvent, SystemEvent, TransparencyEvent};
use ech0::schema::{MemoryTier, ScoredNode};
use ech0::{SearchOptions, Store, StoreConfig, StorePathConfig};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinSet;

use crate::embedder::{SenaEmbedder, EMBEDDING_DIMENSIONS};
use crate::encrypted_store::EncryptedStore;
use crate::error::MemoryError;
use crate::extractor::SenaExtractor;
use crate::transparency_query;

const MEMORY_ACTOR_NAME: &str = "memory";
const MEMORY_CHANNEL_CAPACITY: usize = 256;

/// Memory actor — the sole owner of the `ech0::Store`.
///
/// Spawned by the runtime after the inference actor is ready.
pub struct MemoryActor {
    encrypted_dir: std::path::PathBuf,
    master_key: Option<crypto::MasterKey>,
    consolidation_interval: Duration,
    consolidation_idle_threshold: Duration,
    encrypted_store: Option<EncryptedStore>,
    store: Option<Arc<Store<SenaEmbedder, SenaExtractor>>>,
    bus: Option<Arc<EventBus>>,
    broadcast_rx: Option<broadcast::Receiver<Event>>,
    directed_rx: Option<mpsc::Receiver<Event>>,
    task_set: JoinSet<()>,
}

impl MemoryActor {
    /// Construct with explicit directory for encrypted memory storage.
    /// Uses a default consolidation interval of 5 minutes.
    pub fn new(
        encrypted_dir: impl Into<std::path::PathBuf>,
        master_key: crypto::MasterKey,
    ) -> Self {
        Self::with_consolidation_interval(encrypted_dir, master_key, Duration::from_secs(300))
    }

    /// Construct with a custom consolidation interval.
    pub fn with_consolidation_interval(
        encrypted_dir: impl Into<std::path::PathBuf>,
        master_key: crypto::MasterKey,
        consolidation_interval: Duration,
    ) -> Self {
        Self {
            encrypted_dir: encrypted_dir.into(),
            master_key: Some(master_key),
            consolidation_interval,
            consolidation_idle_threshold: Duration::from_secs(120),
            encrypted_store: None,
            store: None,
            bus: None,
            broadcast_rx: None,
            directed_rx: None,
            task_set: JoinSet::new(),
        }
    }

    /// Override the idle threshold used before running consolidation jobs.
    pub fn with_consolidation_idle_threshold(mut self, idle_threshold: Duration) -> Self {
        self.consolidation_idle_threshold = idle_threshold;
        self
    }

    async fn handle_write(
        store: Arc<Store<SenaEmbedder, SenaExtractor>>,
        req: MemoryWriteRequest,
        bus: Arc<EventBus>,
    ) -> Result<(), MemoryError> {
        let result = store
            .ingest_text(&req.text)
            .await
            .map_err(|e| MemoryError::Store(e.to_string()))?;

        for conflict in result.conflicts {
            let description = format!(
                "conflict (kind={}) vs existing (kind={})",
                conflict.new_node.kind, conflict.existing_node.kind
            );
            let event = MemoryConflictDetected {
                description,
                request_id: req.request_id,
            };
            // Broadcast inline — no spawn needed for simple broadcast
            let _ = bus
                .broadcast(Event::Memory(MemoryEvent::ConflictDetected(event)))
                .await;
        }

        // Drop the linking_task — ech0 runs it in the background automatically.
        drop(result.linking_task);

        Ok(())
    }

    async fn handle_semantic_ingest(
        store: Arc<Store<SenaEmbedder, SenaExtractor>>,
        req: SemanticIngestRequest,
        bus: Arc<EventBus>,
    ) -> Result<(), MemoryError> {
        // Prefix the routing key so downstream retrieval can filter by tier.
        let tagged = format!("[{}] {}", req.routing_key, req.text);
        let result = store
            .ingest_text(&tagged)
            .await
            .map_err(|e| MemoryError::Store(e.to_string()))?;

        // Drop the linking_task — let it run in the background.
        drop(result.linking_task);

        let complete = SemanticIngestComplete {
            node_id: req.request_id,
            request_id: req.request_id,
        };
        // Broadcast inline — no spawn needed for simple broadcast
        let _ = bus
            .broadcast(Event::Memory(MemoryEvent::SemanticIngestComplete(complete)))
            .await;
        Ok(())
    }

    async fn handle_consolidation(
        store: Arc<Store<SenaEmbedder, SenaExtractor>>,
        bus: Arc<EventBus>,
    ) {
        match store.decay().await {
            Ok(report) => {
                // Keep decay lightweight and periodic. Prune only when something has
                // materially decayed so we avoid unnecessary churn.
                if report.nodes_below_threshold > 0 {
                    let _ = store.prune(0.05).await;
                }

                // Phase 3 — Semantic promotion pipeline.
                // After decay stabilises importance scores, search the episodic tier for
                // high-signal chunks (score >= 0.65) and promote them to semantic tier so
                // they persist across future sessions. We cap promotion at 5 nodes per
                // consolidation cycle to keep the operation cheap.
                if report.nodes_decayed > 0 {
                    let promotion_opts = SearchOptions {
                        limit: 5,
                        vector_weight: 0.4,
                        graph_weight: 0.6,
                        min_importance: 0.65,
                        tiers: vec![MemoryTier::Episodic],
                    };
                    if let Ok(results) = store.search("", promotion_opts).await {
                        for (idx, scored) in results.nodes.into_iter().enumerate() {
                            let text = node_to_text(&scored.node);
                            if text.is_empty() {
                                continue;
                            }
                            let request_id = SystemTime::now()
                                .duration_since(SystemTime::UNIX_EPOCH)
                                .map(|d| d.as_nanos() as u64)
                                .unwrap_or(idx as u64);
                            let req = SemanticIngestRequest {
                                text,
                                routing_key: "episodic:promoted".to_string(),
                                request_id,
                            };
                            let _ = bus
                                .send_directed(
                                    MEMORY_ACTOR_NAME,
                                    Event::Memory(MemoryEvent::SemanticIngestRequested(req)),
                                )
                                .await;
                        }
                    }
                }

                let event = MemoryConsolidationCompleted {
                    nodes_decayed: report.nodes_decayed,
                };
                let _ = bus
                    .broadcast(Event::Memory(MemoryEvent::ConsolidationCompleted(event)))
                    .await;
            }
            Err(e) => {
                let _ = e;
            }
        }
    }

    async fn handle_query(
        store: Arc<Store<SenaEmbedder, SenaExtractor>>,
        req: MemoryQueryRequest,
        bus: Arc<EventBus>,
    ) -> Result<(), MemoryError> {
        // Level 1 (coarse): graph-heavy search over semantic tier to discover
        // high-signal memory clusters relevant to this query.
        let level1_opts = SearchOptions {
            limit: (req.token_budget / 2).max(4),
            vector_weight: 0.15,
            graph_weight: 0.85,
            min_importance: 0.0,
            tiers: vec![MemoryTier::Semantic],
        };

        let level1 = store
            .search(&req.query, level1_opts)
            .await
            .map_err(|e| MemoryError::Store(e.to_string()))?;

        // Build a refined query by appending top coarse nodes so level 2 can
        // screen within those discovered regions.
        let refined_query = build_refined_query(&req.query, &level1.nodes);

        // Level 2 (fine): vector-heavy search across episodic + semantic tiers.
        let level2_opts = SearchOptions {
            limit: req.token_budget.max(1),
            vector_weight: 0.85,
            graph_weight: 0.15,
            min_importance: 0.0,
            tiers: vec![MemoryTier::Episodic, MemoryTier::Semantic],
        };

        let level2 = store
            .search(&refined_query, level2_opts)
            .await
            .map_err(|e| MemoryError::Store(e.to_string()))?;

        // Merge both levels, deduplicate by node id, keep best score.
        let mut by_id = std::collections::HashMap::new();
        for scored in level1.nodes.into_iter().chain(level2.nodes.into_iter()) {
            let text = node_to_text(&scored.node);
            by_id
                .entry(scored.node.id)
                .and_modify(|(_, score)| {
                    if scored.score > *score {
                        *score = scored.score;
                    }
                })
                .or_insert((text, scored.score));
        }

        let mut chunks: Vec<MemoryChunk> = by_id
            .into_values()
            .map(|(text, score)| MemoryChunk {
                text,
                score,
                timestamp: SystemTime::now(),
            })
            .collect();

        // Use total_cmp to handle NaN values without unwrap_or
        chunks.sort_by(|a, b| b.score.total_cmp(&a.score));
        chunks.truncate(req.token_budget.max(1));

        let response = MemoryQueryResponse {
            chunks,
            request_id: req.request_id,
        };

        // Broadcast inline — no spawn needed for simple broadcast
        let _ = bus
            .broadcast(Event::Memory(MemoryEvent::QueryCompleted(response)))
            .await;

        Ok(())
    }

    async fn handle_user_memory_transparency_query(
        store: Arc<Store<SenaEmbedder, SenaExtractor>>,
        bus: Arc<EventBus>,
        mut broadcast_rx: broadcast::Receiver<Event>,
    ) {
        let request_id = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => d.as_nanos() as u64,
            Err(_) => 1,
        };

        if let Err(_error) = transparency_query::handle_transparency_query(
            store,
            Arc::clone(&bus),
            &mut broadcast_rx,
            request_id,
        )
        .await
        {
            let _ = bus
                .broadcast(Event::Transparency(TransparencyEvent::MemoryResponded(
                    transparency_query::empty_memory_response(),
                )))
                .await;
        }
    }
}

/// Extract display text from an ech0 node.
///
/// Priority: `source_text` (provenance feature) → `metadata["text"]` → `kind`.
fn node_to_text(node: &ech0::Node) -> String {
    if let Some(src) = &node.source_text {
        return src.clone();
    }
    if let Some(t) = node.metadata.get("text").and_then(|v| v.as_str()) {
        return t.to_owned();
    }
    node.kind.clone()
}

#[async_trait]
impl Actor for MemoryActor {
    fn name(&self) -> &'static str {
        MEMORY_ACTOR_NAME
    }

    async fn start(&mut self, bus: Arc<EventBus>) -> Result<(), ActorError> {
        let master_key = self
            .master_key
            .take()
            .ok_or_else(|| ActorError::StartupFailed("MemoryActor already started".into()))?;

        // Open encrypted store first
        let encrypted_store = EncryptedStore::open(&self.encrypted_dir, &master_key)
            .map_err(|e| ActorError::StartupFailed(format!("encrypted store init: {e}")))?;

        // Use working directory for ech0 Store paths
        let working_dir = encrypted_store.working_dir();
        let graph_path = working_dir.join("graph.redb");
        let vector_path = working_dir.join("vector.index");

        let config = StoreConfig {
            store: StorePathConfig {
                graph_path: graph_path.to_string_lossy().into_owned(),
                vector_path: vector_path.to_string_lossy().into_owned(),
                vector_dimensions: EMBEDDING_DIMENSIONS,
            },
            ..Default::default()
        };

        let embedder = SenaEmbedder::new(Arc::clone(&bus));
        let extractor = SenaExtractor::new(Arc::clone(&bus));

        let (final_store, final_encrypted_store) =
            match Store::new(config.clone(), embedder, extractor).await {
                Ok(store) => (store, encrypted_store),
                Err(e) => {
                    // Check if this is a redb format mismatch (from dependency upgrade)
                    let error_msg = format!("{}", e);
                    if error_msg.contains("Manual upgrade required")
                        || error_msg.contains("Expected file format version")
                    {
                        // Close the encrypted store to release file locks
                        drop(encrypted_store);

                        // Backup all encrypted files
                        let timestamp = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs();

                        for entry in std::fs::read_dir(&self.encrypted_dir)
                            .map_err(|e| ActorError::StartupFailed(format!("backup failed: {e}")))?
                        {
                            let entry = entry.map_err(|e| {
                                ActorError::StartupFailed(format!("backup failed: {e}"))
                            })?;
                            let path = entry.path();
                            if path.extension().and_then(|e| e.to_str()) == Some("enc") {
                                let backup_path =
                                    path.with_extension(format!("enc.backup-{}", timestamp));
                                std::fs::rename(&path, &backup_path).map_err(|e| {
                                    ActorError::StartupFailed(format!("backup failed: {e}"))
                                })?;
                            }
                        }

                        // Reopen encrypted store (now empty)
                        let new_encrypted_store =
                            EncryptedStore::open(&self.encrypted_dir, &master_key).map_err(
                                |e| {
                                    ActorError::StartupFailed(format!(
                                        "encrypted store init after migration: {e}"
                                    ))
                                },
                            )?;

                        // Recreate embedder and extractor for retry
                        let embedder = SenaEmbedder::new(Arc::clone(&bus));
                        let extractor = SenaExtractor::new(Arc::clone(&bus));

                        // Retry with fresh store
                        let store = Store::new(config, embedder, extractor).await.map_err(|e| {
                            ActorError::StartupFailed(format!(
                                "ech0 store init after migration: {e}"
                            ))
                        })?;

                        (store, new_encrypted_store)
                    } else {
                        // Some other error - propagate it
                        return Err(ActorError::StartupFailed(format!("ech0 store init: {e}")));
                    }
                }
            };

        self.encrypted_store = Some(final_encrypted_store);
        self.store = Some(Arc::new(final_store));
        self.broadcast_rx = Some(bus.subscribe_broadcast());

        let (tx, rx) = mpsc::channel(MEMORY_CHANNEL_CAPACITY);
        bus.register_directed(MEMORY_ACTOR_NAME, tx)
            .map_err(|e| ActorError::StartupFailed(e.to_string()))?;
        self.directed_rx = Some(rx);

        self.bus = Some(bus.clone());

        bus.broadcast(Event::System(SystemEvent::ActorReady {
            actor_name: "Memory",
        }))
        .await
        .map_err(|e| ActorError::StartupFailed(format!("broadcast ActorReady failed: {}", e)))?;

        Ok(())
    }

    async fn run(&mut self) -> Result<(), ActorError> {
        let bus = self
            .bus
            .as_ref()
            .ok_or_else(|| ActorError::RuntimeError("bus not set".into()))?
            .clone();

        let store = self
            .store
            .as_ref()
            .ok_or_else(|| ActorError::RuntimeError("store not initialized".into()))?
            .clone();

        let mut directed_rx = self
            .directed_rx
            .take()
            .ok_or_else(|| ActorError::RuntimeError("directed_rx not set".into()))?;

        let mut broadcast_rx = self
            .broadcast_rx
            .take()
            .ok_or_else(|| ActorError::RuntimeError("broadcast_rx not set".into()))?;

        let mut consolidation_ticker = tokio::time::interval(self.consolidation_interval);
        consolidation_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // Consume the first immediate tick so we don't consolidate on startup.
        consolidation_ticker.tick().await;
        let mut last_activity = Instant::now();

        // Periodic wakeup to ensure shutdown responsibility (50ms)
        let mut wakeup = tokio::time::interval(std::time::Duration::from_millis(50));
        wakeup.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        wakeup.tick().await;

        loop {
            tokio::select! {
                biased;

                bcast = broadcast_rx.recv() => {
                    match bcast {
                        Ok(Event::System(SystemEvent::ShutdownSignal)) => break,
                        Ok(Event::Transparency(TransparencyEvent::QueryRequested(query))) => {
                            // Handle transparency queries on broadcast
                            match query {
                                TransparencyQuery::UserMemory => {
                                    // Spawn untracked (like inference actor) to avoid blocking shutdown.
                                    // Transparency queries are fail-safe and emit default output on failure.
                                    let s = Arc::clone(&store);
                                    let b = Arc::clone(&bus);
                                    let bcast_rx = broadcast_rx.resubscribe();
                                    tokio::spawn(async move {
                                        Self::handle_user_memory_transparency_query(s, b, bcast_rx)
                                            .await;
                                    });
                                }
                                _ => {
                                    // Other transparency queries handled by different actors
                                }
                            }
                        }
                        Ok(_) => {}
                        Err(broadcast::error::RecvError::Closed) => break,
                        Err(broadcast::error::RecvError::Lagged(_)) => {}
                    }
                }
                _ = wakeup.tick() => {
                    // Periodic wakeup for shutdown responsiveness
                }
                _ = consolidation_ticker.tick() => {
                    if last_activity.elapsed() >= self.consolidation_idle_threshold {
                        let s = Arc::clone(&store);
                        let b = Arc::clone(&bus);
                        self.task_set.spawn(async move {
                            Self::handle_consolidation(s, b).await;
                        });
                    }
                }
                msg = directed_rx.recv() => {
                    match msg {
                        Some(Event::System(SystemEvent::ShutdownSignal)) => {
                            break;
                        }
                        Some(Event::Memory(mem_event)) => {
                            last_activity = Instant::now();
                            match mem_event {
                                MemoryEvent::WriteRequested(req) => {
                                    let s = Arc::clone(&store);
                                    let b = Arc::clone(&bus);
                                    self.task_set.spawn(async move {
                                            let _ = Self::handle_write(s, req, b).await;
                                    });
                                }
                                MemoryEvent::SemanticIngestRequested(req) => {
                                    let s = Arc::clone(&store);
                                    let b = Arc::clone(&bus);
                                    self.task_set.spawn(async move {
                                            let _ = Self::handle_semantic_ingest(s, req, b).await;
                                    });
                                }
                                MemoryEvent::QueryRequested(req) => {
                                    let s = Arc::clone(&store);
                                    let b = Arc::clone(&bus);
                                    self.task_set.spawn(async move {
                                            let _ = Self::handle_query(s, req, b).await;
                                    });
                                }
                                // Outbound-only events — ignore if directed back.
                                _ => {}
                            }
                        }
                        Some(_) => {}
                        None => break,
                    }
                }
            }
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), ActorError> {
        // Drain outstanding tasks with a 1-second timeout.
        // Tasks that don't complete (e.g., waiting on inference during shutdown)
        // are aborted to prevent actor stop timeout.
        let drain_timeout = Duration::from_secs(1);
        let drain_deadline = tokio::time::Instant::now() + drain_timeout;

        loop {
            let remaining = drain_deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                // Timeout reached — abort all remaining tasks
                self.task_set.abort_all();
                break;
            }

            match tokio::time::timeout(remaining, self.task_set.join_next()).await {
                Ok(Some(_)) => continue, // Task completed, keep draining
                Ok(None) => break,       // All tasks completed
                Err(_) => {
                    // Timeout reached — abort remaining
                    self.task_set.abort_all();
                    break;
                }
            }
        }

        // Drop the ech0 store first to release any file handles
        self.store = None;

        // Explicitly close encrypted store for guaranteed encryption and cleanup
        if let Some(encrypted_store) = self.encrypted_store.take() {
            encrypted_store.close().map_err(|e| {
                ActorError::RuntimeError(format!("failed to close encrypted store: {e}"))
            })?;
        }

        Ok(())
    }
}

fn build_refined_query(base_query: &str, coarse_nodes: &[ScoredNode]) -> String {
    let mut hints: Vec<String> = Vec::new();
    for node in coarse_nodes.iter().take(3) {
        hints.push(node.node.kind.clone());
        if let Some(text) = node.node.metadata.get("text").and_then(|v| v.as_str()) {
            hints.push(text.to_owned());
        }
    }
    if hints.is_empty() {
        return base_query.to_owned();
    }
    format!("{} {}", base_query, hints.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn test_master_key() -> crypto::MasterKey {
        crypto::MasterKey::from_bytes([42u8; 32])
    }

    #[tokio::test]
    async fn memory_actor_lifecycle_start_stop() {
        let dir = tempdir().expect("tempdir");
        let memory_dir = dir.path().join("memory");

        let mut actor = MemoryActor::new(&memory_dir, test_master_key());
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor
            .start(Arc::clone(&bus))
            .await
            .expect("start should succeed");

        // Consume ActorReady
        let _ = rx.recv().await.expect("ActorReady event");
        actor.stop().await.expect("stop should succeed");
    }

    #[tokio::test]
    async fn memory_actor_handles_transparency_query_user_memory() {
        use bus::events::transparency::TransparencyQuery;

        let dir = tempdir().expect("tempdir");
        let memory_dir = dir.path().join("memory");

        let mut actor = MemoryActor::new(&memory_dir, test_master_key());
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor
            .start(Arc::clone(&bus))
            .await
            .expect("start should succeed");

        // Consume ActorReady
        let _ = rx.recv().await.expect("ActorReady event");

        let actor_handle = tokio::spawn(async move { actor.run().await });

        let mut saw_memory_response = false;

        // Emit a transparency query for user memory
        let query = TransparencyQuery::UserMemory;
        bus.broadcast(Event::Transparency(TransparencyEvent::QueryRequested(
            query,
        )))
        .await
        .expect("broadcast should succeed");

        // Even if Soul or ech0 is unavailable, actor must emit a fallback response.
        let wait_deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while tokio::time::Instant::now() < wait_deadline {
            match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
                Ok(Ok(Event::Transparency(TransparencyEvent::MemoryResponded(_)))) => {
                    saw_memory_response = true;
                    break;
                }
                Ok(Ok(_)) => continue,
                Ok(Err(_)) | Err(_) => continue,
            }
        }

        assert!(
            saw_memory_response,
            "memory actor should emit MemoryResponded for UserMemory queries"
        );

        bus.broadcast(Event::System(SystemEvent::ShutdownSignal))
            .await
            .expect("shutdown signal should broadcast");

        let run_result = tokio::time::timeout(Duration::from_secs(2), actor_handle)
            .await
            .expect("actor run loop should terminate within timeout")
            .expect("actor run loop should not panic");
        assert!(run_result.is_ok(), "actor run loop should complete cleanly");
    }

    #[tokio::test]
    async fn memory_actor_clean_shutdown_with_concurrent_transparency_queries() {
        // Regression test for shutdown timeout issue: transparency query tasks
        // now spawn with tokio::spawn (untracked) rather than into task_set,
        // so they don't block shutdown.
        use bus::events::transparency::TransparencyQuery;

        let dir = tempdir().expect("tempdir");
        let memory_dir = dir.path().join("memory");

        let mut actor = MemoryActor::new(&memory_dir, test_master_key());
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor
            .start(Arc::clone(&bus))
            .await
            .expect("start should succeed");

        // Consume ActorReady
        let _ = rx.recv().await.expect("ActorReady event");

        // Spawn the actor run loop in a background task
        let actor_handle = tokio::spawn(async move { actor.run().await });

        // Issue multiple transparency queries concurrently
        for _ in 0..5 {
            bus.broadcast(Event::Transparency(TransparencyEvent::QueryRequested(
                TransparencyQuery::UserMemory,
            )))
            .await
            .expect("broadcast should succeed");
        }

        // Give queries a moment to spawn
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Issue shutdown signal
        bus.broadcast(Event::System(SystemEvent::ShutdownSignal))
            .await
            .expect("shutdown signal should broadcast");

        // Actor run loop should terminate cleanly without timeout
        let run_result = tokio::time::timeout(Duration::from_secs(2), actor_handle)
            .await
            .expect("actor run loop should terminate within timeout")
            .expect("actor run loop should not panic");
        assert!(run_result.is_ok(), "actor run loop should complete cleanly");
    }

    #[tokio::test]
    async fn memory_actor_stops_on_directed_shutdown_signal() {
        let dir = tempdir().expect("tempdir");
        let memory_dir = dir.path().join("memory");

        let mut actor = MemoryActor::new(&memory_dir, test_master_key());
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor
            .start(Arc::clone(&bus))
            .await
            .expect("start should succeed");

        // Consume ActorReady
        let _ = rx.recv().await.expect("ActorReady event");

        let actor_handle = tokio::spawn(async move { actor.run().await });

        bus.send_directed("memory", Event::System(SystemEvent::ShutdownSignal))
            .await
            .expect("directed shutdown should send");

        let run_result = tokio::time::timeout(Duration::from_secs(2), actor_handle)
            .await
            .expect("actor run loop should terminate within timeout")
            .expect("actor run loop should not panic");
        assert!(run_result.is_ok(), "actor run loop should complete cleanly");
    }
}
