//! Memory actor: manages memory ingestion, consolidation, and retrieval.

use crate::backend::MemoryBackend;
use crate::error::MemoryError;
use bus::events::{MemoryEvent, MemoryKind, ScoredChunk, SystemEvent};
use bus::{Actor, ActorError, CausalId, Event, EventBus};
use std::sync::Arc;
use tracing::{debug, error, info};

/// Memory actor state.
pub struct MemoryActor {
    bus: Option<Arc<EventBus>>,
    backend: Box<dyn MemoryBackend>,
}

impl MemoryActor {
    /// Create a new memory actor with the given backend.
    pub fn new(backend: Box<dyn MemoryBackend>) -> Self {
        Self { bus: None, backend }
    }

    /// Handle memory ingestion request.
    fn handle_ingest_request(
        &mut self,
        text: String,
        kind: MemoryKind,
        causal_id: CausalId,
    ) -> Result<(), MemoryError> {
        debug!(
            text_len = text.len(),
            ?kind,
            causal_id = causal_id.as_u64(),
            "handling ingest request"
        );

        self.backend.ingest(&text, kind, causal_id)?;

        debug!(
            causal_id = causal_id.as_u64(),
            "ingest completed successfully"
        );

        Ok(())
    }

    /// Handle memory query request.
    fn handle_query_request(
        &self,
        embedding: Vec<f32>,
        limit: usize,
        causal_id: CausalId,
    ) -> Result<Vec<ScoredChunk>, MemoryError> {
        debug!(
            embedding_len = embedding.len(),
            limit,
            causal_id = causal_id.as_u64(),
            "handling query request"
        );

        let chunks = self.backend.query(&embedding, limit)?;

        debug!(
            causal_id = causal_id.as_u64(),
            chunk_count = chunks.len(),
            "query completed successfully"
        );

        Ok(chunks)
    }
}

impl Actor for MemoryActor {
    fn name(&self) -> &'static str {
        "memory"
    }

    #[allow(clippy::manual_async_fn)]
    fn start(
        &mut self,
        bus: Arc<EventBus>,
    ) -> impl std::future::Future<Output = Result<(), ActorError>> + Send {
        async move {
            info!("memory actor starting");
            self.bus = Some(bus);
            Ok(())
        }
    }

    #[allow(clippy::manual_async_fn)]
    fn run(&mut self) -> impl std::future::Future<Output = Result<(), ActorError>> + Send {
        async {
            let bus = self
                .bus
                .as_ref()
                .ok_or_else(|| ActorError::StartupFailed("bus not initialized".to_string()))?
                .clone();

            let mut rx = bus.subscribe_broadcast();

            loop {
                match rx.recv().await {
                    Ok(event) => match event {
                        Event::System(SystemEvent::ShutdownSignal) => {
                            info!("shutdown signal received, stopping memory actor");
                            break;
                        }
                        Event::Memory(memory_event) => match memory_event {
                            MemoryEvent::IngestRequested {
                                text,
                                kind,
                                causal_id,
                            } => {
                                if let Err(e) = self.handle_ingest_request(text, kind, causal_id) {
                                    error!(
                                        causal_id = causal_id.as_u64(),
                                        error = %e,
                                        "ingest failed"
                                    );
                                    bus.broadcast(Event::Memory(MemoryEvent::IngestFailed {
                                        causal_id,
                                        reason: e.to_string(),
                                    }))
                                    .await
                                    .map_err(|e| {
                                        ActorError::RuntimeError(format!("broadcast failed: {}", e))
                                    })?;
                                } else {
                                    bus.broadcast(Event::Memory(MemoryEvent::IngestCompleted {
                                        causal_id,
                                    }))
                                    .await
                                    .map_err(|e| {
                                        ActorError::RuntimeError(format!("broadcast failed: {}", e))
                                    })?;
                                }
                            }
                            MemoryEvent::QueryRequested {
                                embedding,
                                limit,
                                causal_id,
                            } => match self.handle_query_request(embedding, limit, causal_id) {
                                Ok(chunks) => {
                                    bus.broadcast(Event::Memory(MemoryEvent::QueryCompleted {
                                        chunks,
                                        causal_id,
                                    }))
                                    .await
                                    .map_err(|e| {
                                        ActorError::RuntimeError(format!("broadcast failed: {}", e))
                                    })?;
                                }
                                Err(e) => {
                                    error!(
                                        causal_id = causal_id.as_u64(),
                                        error = %e,
                                        "query failed"
                                    );
                                    bus.broadcast(Event::Memory(MemoryEvent::QueryFailed {
                                        causal_id,
                                        reason: e.to_string(),
                                    }))
                                    .await
                                    .map_err(|e| {
                                        ActorError::RuntimeError(format!("broadcast failed: {}", e))
                                    })?;
                                }
                            },
                            _ => {}
                        },
                        _ => {}
                    },
                    Err(e) => {
                        error!(error = %e, "broadcast channel error");
                        return Err(ActorError::ChannelClosed(e.to_string()));
                    }
                }
            }

            info!("memory actor stopped");
            Ok(())
        }
    }

    #[allow(clippy::manual_async_fn)]
    fn stop(&mut self) -> impl std::future::Future<Output = Result<(), ActorError>> + Send {
        async {
            info!("memory actor cleanup complete");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::StubBackend;

    #[test]
    fn memory_actor_constructs_with_backend() {
        let backend = Box::new(StubBackend::new());
        let actor = MemoryActor::new(backend);
        assert_eq!(actor.name(), "memory");
    }

    #[tokio::test]
    async fn memory_actor_lifecycle_completes() {
        let backend = Box::new(StubBackend::new());
        let mut actor = MemoryActor::new(backend);
        let bus = Arc::new(EventBus::new());

        actor.start(Arc::clone(&bus)).await.expect("start failed");
        actor.stop().await.expect("stop failed");
    }

    #[tokio::test]
    async fn memory_actor_handles_ingest_through_backend() {
        let backend = Box::new(StubBackend::new());
        let mut actor = MemoryActor::new(backend);

        let result = actor.handle_ingest_request(
            "test memory".to_string(),
            MemoryKind::Episodic,
            CausalId::new(),
        );

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn memory_actor_handles_query_through_backend() {
        let backend = Box::new(StubBackend::new());
        let actor = MemoryActor::new(backend);

        let result = actor.handle_query_request(vec![0.1, 0.2, 0.3], 10, CausalId::new());

        assert!(result.is_ok());
        let chunks = result.unwrap();
        assert!(chunks.is_empty(), "stub backend returns empty chunks");
    }
}
