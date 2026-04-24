//! Memory-related IPC command handlers.

use async_trait::async_trait;
use bus::{CausalId, Event, EventBus, MemoryEvent};
use ipc::{CommandHandler, IpcError};
use serde_json::{Value, json};
use std::sync::Arc;

/// Handler for "memory.stats" command.
pub struct MemoryStatsHandler {
    bus: Arc<EventBus>,
}

impl MemoryStatsHandler {
    pub fn new(bus: Arc<EventBus>) -> Self {
        Self { bus }
    }
}

#[async_trait]
impl CommandHandler for MemoryStatsHandler {
    fn name(&self) -> &'static str {
        "memory.stats"
    }

    fn description(&self) -> &'static str {
        "Get memory subsystem statistics"
    }

    async fn handle(&self, _payload: Value) -> Result<Value, IpcError> {
        let causal_id = CausalId::new();
        let mut rx = self.bus.subscribe_broadcast();

        self.bus
            .broadcast(Event::Memory(MemoryEvent::StatsRequested { causal_id }))
            .await
            .map_err(|e| IpcError::CommandFailed(e.to_string()))?;

        let wait_result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                match rx.recv().await {
                    Ok(Event::Memory(MemoryEvent::StatsCompleted {
                        working_memory_chunks,
                        long_term_memory_nodes,
                        causal_id: event_causal_id,
                    })) if event_causal_id == causal_id => {
                        return Ok(json!({
                            "working_memory_chunks": working_memory_chunks,
                            "long_term_memory_nodes": long_term_memory_nodes,
                        }));
                    }
                    Ok(Event::Memory(MemoryEvent::StatsFailed {
                        reason,
                        causal_id: event_causal_id,
                    })) if event_causal_id == causal_id => {
                        return Err(IpcError::CommandFailed(reason));
                    }
                    Ok(_) => {}
                    Err(e) => return Err(IpcError::CommandFailed(e.to_string())),
                }
            }
        })
        .await;

        match wait_result {
            Ok(result) => result,
            Err(_) => Err(IpcError::CommandFailed(
                "timed out waiting for memory stats".to_string(),
            )),
        }
    }
}

/// Handler for "memory.query" command.
pub struct MemoryQueryHandler {
    bus: Arc<EventBus>,
}

impl MemoryQueryHandler {
    pub fn new(bus: Arc<EventBus>) -> Self {
        Self { bus }
    }
}

#[async_trait]
impl CommandHandler for MemoryQueryHandler {
    fn name(&self) -> &'static str {
        "memory.query"
    }

    fn description(&self) -> &'static str {
        "Query long-term memory with a semantic search"
    }

    async fn handle(&self, payload: Value) -> Result<Value, IpcError> {
        let query = payload
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                IpcError::InvalidPayload("missing or invalid 'query' field".to_string())
            })?;

        let limit = payload
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(5);

        let causal_id = CausalId::new();
        let mut rx = self.bus.subscribe_broadcast();

        self.bus
            .broadcast(Event::Memory(MemoryEvent::QueryRequested {
                query: query.to_string(),
                limit,
                causal_id,
            }))
            .await
            .map_err(|e| IpcError::CommandFailed(e.to_string()))?;

        let wait_result = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                match rx.recv().await {
                    Ok(Event::Memory(MemoryEvent::QueryCompleted {
                        chunks,
                        causal_id: event_causal_id,
                    })) if event_causal_id == causal_id => {
                        let chunks_json: Vec<Value> = chunks
                            .into_iter()
                            .map(|chunk| json!({
                                "content": chunk.content,
                                "score": chunk.score,
                                "age_seconds": chunk.age_seconds,
                            }))
                            .collect();
                        return Ok(json!({ "chunks": chunks_json }));
                    }
                    Ok(Event::Memory(MemoryEvent::QueryFailed {
                        reason,
                        causal_id: event_causal_id,
                    })) if event_causal_id == causal_id => {
                        return Err(IpcError::CommandFailed(reason));
                    }
                    Ok(_) => {}
                    Err(e) => return Err(IpcError::CommandFailed(e.to_string())),
                }
            }
        })
        .await;

        match wait_result {
            Ok(result) => result,
            Err(_) => Err(IpcError::CommandFailed(
                "timed out waiting for memory query result".to_string(),
            )),
        }
    }
}
