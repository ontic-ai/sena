//! Memory actor — owns ech0 Store, handles memory ingest and queries.

use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use bus::{Actor, ActorError, Event, EventBus, MemoryEvent, SystemEvent};
use bus::events::memory::{MemoryChunk, MemoryConflictDetected, MemoryQueryRequest, MemoryQueryResponse, MemoryWriteRequest};
use tokio::sync::{broadcast, mpsc};

use crate::ech0_placeholder::{SearchOptions, SearchTier, Store, StorePathConfig};
use crate::embedder::SenaEmbedder;
use crate::error::MemoryError;
use crate::extractor::SenaExtractor;

const MEMORY_ACTOR_NAME: &str = "memory";
const MEMORY_CHANNEL_CAPACITY: usize = 256;

/// Memory actor that owns the ech0 Store and handles ingest/retrieval.
pub struct MemoryActor {
    store_graph_path: std::path::PathBuf,
    store_vector_path: std::path::PathBuf,
    store: Option<Store>,
    bus: Option<Arc<EventBus>>,
    broadcast_rx: Option<broadcast::Receiver<Event>>,
    directed_rx: Option<mpsc::Receiver<Event>>,
}

impl MemoryActor {
    pub fn new(
        store_graph_path: impl Into<std::path::PathBuf>,
        store_vector_path: impl Into<std::path::PathBuf>,
    ) -> Self {
        Self {
            store_graph_path: store_graph_path.into(),
            store_vector_path: store_vector_path.into(),
            store: None,
            bus: None,
            broadcast_rx: None,
            directed_rx: None,
        }
    }

    async fn handle_write(&self, req: MemoryWriteRequest, bus: &Arc<EventBus>) -> Result<(), MemoryError> {
        let store = self.store.as_ref().ok_or_else(|| MemoryError::Store("not initialized".into()))?;
        
        let result = store.ingest_text(&req.text).await
            .map_err(|e| MemoryError::Store(e))?;

        if let Some(conflict) = result.conflict {
            let conflict_event = MemoryConflictDetected {
                description: conflict.description,
                request_id: req.request_id,
            };
            let bus_clone = Arc::clone(bus);
            tokio::spawn(async move {
                let _ = bus_clone.broadcast(Event::Memory(MemoryEvent::ConflictDetected(conflict_event))).await;
            });
        }

        Ok(())
    }

    async fn handle_query(&self, req: MemoryQueryRequest, bus: &Arc<EventBus>) -> Result<(), MemoryError> {
        let store = self.store.as_ref().ok_or_else(|| MemoryError::Store("not initialized".into()))?;

        // Level 1: Graph search
        let graph_results = store.search(&req.query, SearchOptions {
            tier: SearchTier::Graph,
            max_results: req.token_budget / 2,
        }).await.map_err(|e| MemoryError::Store(e))?;

        // Level 2: Vector search
        let vector_results = store.search(&req.query, SearchOptions {
            tier: SearchTier::Vector,
            max_results: req.token_budget / 2,
        }).await.map_err(|e| MemoryError::Store(e))?;

        // Merge and deduplicate
        let mut chunks: Vec<MemoryChunk> = vec![];
        for node in graph_results.iter().chain(vector_results.iter()) {
            chunks.push(MemoryChunk {
                text: node.text.clone(),
                score: node.score,
                timestamp: SystemTime::now(),
            });
        }

        // Apply token budget (simplified: use first N chunks)
        chunks.truncate(req.token_budget);

        let response = MemoryQueryResponse {
            chunks,
            request_id: req.request_id,
        };

        let bus_clone = Arc::clone(bus);
        tokio::spawn(async move {
            let _ = bus_clone.broadcast(Event::Memory(MemoryEvent::QueryCompleted(response))).await;
        });

        Ok(())
    }
}

#[async_trait]
impl Actor for MemoryActor {
    fn name(&self) -> &'static str {
        MEMORY_ACTOR_NAME
    }

    async fn start(&mut self, bus: Arc<EventBus>) -> Result<(), ActorError> {
        let config = StorePathConfig {
            graph_path: self.store_graph_path.clone(),
            vector_path: self.store_vector_path.clone(),
        };

        let embedder = SenaEmbedder::new(Arc::clone(&bus));
        let extractor = SenaExtractor::new(Arc::clone(&bus));

        let store = Store::new(config, embedder, extractor)
            .map_err(|e| ActorError::StartupFailed(format!("ech0 store init: {}", e)))?;

        self.store = Some(store);
        self.broadcast_rx = Some(bus.subscribe_broadcast());

        let (tx, rx) = mpsc::channel(MEMORY_CHANNEL_CAPACITY);
        bus.register_directed(MEMORY_ACTOR_NAME, tx)
            .map_err(|e| ActorError::StartupFailed(e.to_string()))?;
        self.directed_rx = Some(rx);

        self.bus = Some(bus);
        Ok(())
    }

    async fn run(&mut self) -> Result<(), ActorError> {
        let bus = self.bus.as_ref()
            .ok_or_else(|| ActorError::RuntimeError("bus not set".into()))?
            .clone();

        let mut directed_rx = self.directed_rx.take()
            .ok_or_else(|| ActorError::RuntimeError("directed_rx not set".into()))?;

        let mut broadcast_rx = self.broadcast_rx.take()
            .ok_or_else(|| ActorError::RuntimeError("broadcast_rx not set".into()))?;

        loop {
            tokio::select! {
                msg = directed_rx.recv() => {
                    match msg {
                        Some(Event::Memory(mem_event)) => {
                            match mem_event {
                                MemoryEvent::WriteRequested(req) => {
                                    if let Err(e) = self.handle_write(req, &bus).await {
                                        eprintln!("[memory] write failed: {}", e);
                                    }
                                }
                                MemoryEvent::QueryRequested(req) => {
                                    if let Err(e) = self.handle_query(req, &bus).await {
                                        eprintln!("[memory] query failed: {}", e);
                                    }
                                }
                                _ => {} // QueryCompleted, ConflictDetected are outbound only
                            }
                        }
                        Some(_) => {}
                        None => break,
                    }
                }
                bcast = broadcast_rx.recv() => {
                    match bcast {
                        Ok(Event::System(SystemEvent::ShutdownSignal)) => break,
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn memory_actor_lifecycle() {
        let dir = tempdir().expect("tempdir");
        let graph_path = dir.path().join("graph.redb.enc");
        let vector_path = dir.path().join("vector.usearch.enc");

        let mut actor = MemoryActor::new(&graph_path, &vector_path);
        let bus = Arc::new(EventBus::new());

        actor.start(Arc::clone(&bus)).await.expect("start should succeed");
        actor.stop().await.expect("stop should succeed");
    }
}
