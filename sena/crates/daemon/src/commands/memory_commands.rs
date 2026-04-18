//! Memory-related IPC command handlers.

use async_trait::async_trait;
use ipc::{CommandHandler, IpcError};
use serde_json::{Value, json};

/// Handler for "memory.stats" command.
pub struct MemoryStatsHandler;

#[async_trait]
impl CommandHandler for MemoryStatsHandler {
    fn name(&self) -> &'static str {
        "memory.stats"
    }

    fn description(&self) -> &'static str {
        "Get memory subsystem statistics"
    }

    async fn handle(&self, _payload: Value) -> Result<Value, IpcError> {
        // Phase 2 limitation: no runtime helper for memory stats.
        Ok(json!({
            "working_memory_chunks": 0,
            "long_term_memory_nodes": 0,
            "note": "Memory stats not yet implemented"
        }))
    }
}

/// Handler for "memory.query" command.
pub struct MemoryQueryHandler;

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

        // Phase 2 limitation: memory query via bus events not yet wired.
        Err(IpcError::CommandNotReady(format!(
            "Memory query not yet implemented (query: {} chars)",
            query.len()
        )))
    }
}
