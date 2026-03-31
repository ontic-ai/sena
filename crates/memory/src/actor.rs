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

use crate::embedder::{SenaEmbedder, EMBEDDING_DIMENSIONS};
use crate::error::MemoryError;
use crate::extractor::SenaExtractor;
use crate::transparency_query;

const MEMORY_ACTOR_NAME: &str = "memory";
const MEMORY_CHANNEL_CAPACITY: usize = 256;

/// Memory actor — the sole owner of the `ech0::Store`.
///
/// Spawned by the runtime after the inference actor is ready.
pub struct MemoryActor {
    store_graph_path: std::path::PathBuf,
    store_vector_path: std::path::PathBuf,
    consolidation_interval: Duration,
    consolidation_idle_threshold: Duration,
    store: Option<Arc<Store<SenaEmbedder, SenaExtractor>>>,
    bus: Option<Arc<EventBus>>,
    broadcast_rx: Option<broadcast::Receiver<Event>>,
    directed_rx: Option<mpsc::Receiver<Event>>,
}

impl MemoryActor {
    /// Construct with explicit paths for the ech0 graph and vector index.
    /// Uses a default consolidation interval of 5 minutes.
    pub fn new(
        store_graph_path: impl Into<std::path::PathBuf>,
        store_vector_path: impl Into<std::path::PathBuf>,
    ) -> Self {
        Self::with_consolidation_interval(
            store_graph_path,
            store_vector_path,
            Duration::from_secs(300),
        )
    }

    /// Construct with a custom consolidation interval.
    pub fn with_consolidation_interval(
        store_graph_path: impl Into<std::path::PathBuf>,
        store_vector_path: impl Into<std::path::PathBuf>,
        consolidation_interval: Duration,
    ) -> Self {
        Self {
            store_graph_path: store_graph_path.into(),
            store_vector_path: store_vector_path.into(),
            consolidation_interval,
            consolidation_idle_threshold: Duration::from_secs(120),
            store: None,
            bus: None,
            broadcast_rx: None,
            directed_rx: None,
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
            let bus_c = Arc::clone(&bus);
            tokio::spawn(async move {
                let _ = bus_c
                    .broadcast(Event::Memory(MemoryEvent::ConflictDetected(event)))
                    .await;
            });
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
        let bus_c = Arc::clone(&bus);
        tokio::spawn(async move {
            let _ = bus_c
                .broadcast(Event::Memory(MemoryEvent::SemanticIngestComplete(complete)))
                .await;
        });
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
                let event = MemoryConsolidationCompleted {
                    nodes_decayed: report.nodes_decayed,
                };
                let _ = bus
                    .broadcast(Event::Memory(MemoryEvent::ConsolidationCompleted(event)))
                    .await;
            }
            Err(e) => {
                eprintln!("[memory] consolidation (decay) failed: {e}");
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

        chunks.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        chunks.truncate(req.token_budget.max(1));

        let response = MemoryQueryResponse {
            chunks,
            request_id: req.request_id,
        };

        let bus_c = Arc::clone(&bus);
        tokio::spawn(async move {
            let _ = bus_c
                .broadcast(Event::Memory(MemoryEvent::QueryCompleted(response)))
                .await;
        });

        Ok(())
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
        let config = StoreConfig {
            store: StorePathConfig {
                graph_path: self.store_graph_path.to_string_lossy().into_owned(),
                vector_path: self.store_vector_path.to_string_lossy().into_owned(),
                vector_dimensions: EMBEDDING_DIMENSIONS,
            },
            ..Default::default()
        };

        let embedder = SenaEmbedder::new(Arc::clone(&bus));
        let extractor = SenaExtractor::new(Arc::clone(&bus));

        let store = Store::new(config, embedder, extractor)
            .await
            .map_err(|e| ActorError::StartupFailed(format!("ech0 store init: {e}")))?;

        self.store = Some(Arc::new(store));
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
        // Skip ticks that we missed while processing — no burst-catch-up.
        consolidation_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // Consume the first immediate tick so we don't consolidate on startup.
        consolidation_ticker.tick().await;
        let mut last_activity = Instant::now();

        loop {
            tokio::select! {
                _ = consolidation_ticker.tick() => {
                    if last_activity.elapsed() >= self.consolidation_idle_threshold {
                        let s = Arc::clone(&store);
                        let b = Arc::clone(&bus);
                        tokio::spawn(async move {
                            Self::handle_consolidation(s, b).await;
                        });
                    }
                }
                msg = directed_rx.recv() => {
                    match msg {
                        Some(Event::Memory(mem_event)) => {
                            last_activity = Instant::now();
                            match mem_event {
                                MemoryEvent::WriteRequested(req) => {
                                    let s = Arc::clone(&store);
                                    let b = Arc::clone(&bus);
                                    tokio::spawn(async move {
                                        if let Err(e) = Self::handle_write(s, req, b).await {
                                            eprintln!("[memory] write failed: {e}");
                                        }
                                    });
                                }
                                MemoryEvent::SemanticIngestRequested(req) => {
                                    let s = Arc::clone(&store);
                                    let b = Arc::clone(&bus);
                                    tokio::spawn(async move {
                                        if let Err(e) = Self::handle_semantic_ingest(s, req, b).await {
                                            eprintln!("[memory] semantic ingest failed: {e}");
                                        }
                                    });
                                }
                                MemoryEvent::QueryRequested(req) => {
                                    let s = Arc::clone(&store);
                                    let b = Arc::clone(&bus);
                                    tokio::spawn(async move {
                                        if let Err(e) = Self::handle_query(s, req, b).await {
                                            eprintln!("[memory] query failed: {e}");
                                        }
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
                bcast = broadcast_rx.recv() => {
                    match bcast {
                        Ok(Event::System(SystemEvent::ShutdownSignal)) => break,
                        Ok(Event::Transparency(TransparencyEvent::QueryRequested(query))) => {
                            // Handle transparency queries on broadcast
                            match query {
                                TransparencyQuery::UserMemory => {
                                    let s = Arc::clone(&store);
                                    let b = Arc::clone(&bus);
                                    let mut bcast_rx = broadcast_rx.resubscribe();
                                    tokio::spawn(async move {
                                        // Generate a simple request_id based on timestamp
                                        let request_id = std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .map(|d| d.as_nanos() as u64)
                                            .unwrap_or(1);

                                        if let Err(e) = transparency_query::handle_transparency_query(
                                            s,
                                            b,
                                            &mut bcast_rx,
                                            request_id,
                                        )
                                        .await
                                        {
                                            eprintln!("[memory] transparency query failed: {e}");
                                        }
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
            }
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), ActorError> {
        self.store = None;
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

    #[tokio::test]
    async fn memory_actor_lifecycle_start_stop() {
        let dir = tempdir().expect("tempdir");
        let graph_path = dir.path().join("graph");
        let vector_path = dir.path().join("vector.usearch");

        let mut actor = MemoryActor::new(&graph_path, &vector_path);
        let bus = Arc::new(EventBus::new());

        actor
            .start(Arc::clone(&bus))
            .await
            .expect("start should succeed");
        actor.stop().await.expect("stop should succeed");
    }

    #[tokio::test]
    async fn memory_actor_handles_transparency_query_user_memory() {
        use bus::events::transparency::TransparencyQuery;

        let dir = tempdir().expect("tempdir");
        let graph_path = dir.path().join("graph");
        let vector_path = dir.path().join("vector.usearch");

        let mut actor = MemoryActor::new(&graph_path, &vector_path);
        let bus = Arc::new(EventBus::new());

        actor
            .start(Arc::clone(&bus))
            .await
            .expect("start should succeed");

        // Emit a transparency query for user memory
        let query = TransparencyQuery::UserMemory;
        bus.broadcast(Event::Transparency(TransparencyEvent::QueryRequested(
            query,
        )))
        .await
        .expect("broadcast sh   ould succeed");

        // Give the memory actor a moment to process
        tokio::time::sleep(Duration::from_millis(100)).await;

        // The transparency query handler should have emitted a response event or handled gracefully
        // (Since there's no Soul response, it should use a default summary)
        actor.stop().await.expect("stop should succeed");
    }
}
