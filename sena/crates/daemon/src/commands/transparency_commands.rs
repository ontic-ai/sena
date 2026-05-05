//! Transparency command handlers.

use async_trait::async_trait;
use bus::{Event, TransparencyEvent, TransparencyQuery};
use ipc::{CommandHandler, IpcError};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::warn;

/// Handler for the "transparency_query" IPC command.
///
/// This handler broadcasts a `TransparencyEvent::QueryRequested` on the bus,
/// waits for the corresponding response event, and returns it to the client.
pub struct TransparencyQueryHandler {
    bus: Arc<bus::EventBus>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use bus::events::transparency::ReasoningResponse;
    use bus::{EventBus, TransparencyResult};
    use serde_json::json;

    #[tokio::test]
    async fn transparency_handler_returns_matching_query_response() {
        let bus = Arc::new(EventBus::new());
        let handler = TransparencyQueryHandler::new(bus.clone());
        let responder_bus = bus.clone();
        let mut rx = responder_bus.subscribe_broadcast();

        tokio::spawn(async move {
            while let Ok(event) = rx.recv().await {
                if let Event::Transparency(TransparencyEvent::QueryRequested(
                    TransparencyQuery::CurrentObservation,
                )) = event
                {
                    responder_bus
                        .broadcast(Event::Transparency(TransparencyEvent::QueryResponse {
                            query: TransparencyQuery::CurrentObservation,
                            result: Box::new(TransparencyResult::Reasoning(ReasoningResponse {
                                causal_id: 42,
                                source_description: "test".to_string(),
                                token_count: 100,
                                response_preview: "ok".to_string(),
                            })),
                        }))
                        .await
                        .expect("responder should publish query response");
                    break;
                }
            }
        });

        let result = handler
            .handle(json!("CurrentObservation"))
            .await
            .expect("handler should return response");

        // Check the serialization contains expected structured fields
        assert!(result.get("Reasoning").is_some());
        let reasoning = result.get("Reasoning").unwrap();
        assert_eq!(reasoning.get("causal_id").unwrap(), 42);
    }
}

impl TransparencyQueryHandler {
    pub fn new(bus: Arc<bus::EventBus>) -> Self {
        Self { bus }
    }
}

#[async_trait]
impl CommandHandler for TransparencyQueryHandler {
    fn name(&self) -> &'static str {
        "transparency_query"
    }

    fn description(&self) -> &'static str {
        "Query Sena's state for transparency (observation, memory, reasoning)"
    }

    async fn handle(&self, payload: Value) -> Result<Value, IpcError> {
        // Parse requested query from payload
        let query: TransparencyQuery = serde_json::from_value(payload).map_err(|e| {
            IpcError::InvalidRequest(format!("failed to parse transparency query: {}", e))
        })?;

        // Subscribe to the bus to catch the response
        let mut rx = self.bus.subscribe_broadcast();

        // Broadcast the query
        self.bus
            .broadcast(Event::Transparency(TransparencyEvent::QueryRequested(
                query.clone(),
            )))
            .await
            .map_err(|e| IpcError::Internal(format!("failed to broadcast query: {}", e)))?;

        // Wait for response
        let timeout = std::time::Duration::from_secs(5);
        let start = std::time::Instant::now();

        while start.elapsed() < timeout {
            match tokio::time::timeout(timeout - start.elapsed(), rx.recv()).await {
                Ok(Ok(event)) => {
                    if let Event::Transparency(TransparencyEvent::QueryResponse {
                        query: response_query,
                        result,
                    }) = event
                    {
                        // Check if this response matches our query
                        let matches = match (&query, &response_query) {
                            (
                                TransparencyQuery::CurrentObservation,
                                TransparencyQuery::CurrentObservation,
                            ) => true,
                            (TransparencyQuery::UserMemory, TransparencyQuery::UserMemory) => true,
                            (
                                TransparencyQuery::ReasoningChain { .. },
                                TransparencyQuery::ReasoningChain { .. },
                            ) => true,
                            _ => false,
                        };

                        if matches {
                            return serde_json::to_value(result).map_err(|e| {
                                IpcError::Internal(format!("failed to serialize result: {}", e))
                            });
                        }
                    }
                }
                Ok(Err(broadcast::error::RecvError::Lagged(n))) => {
                    warn!("Transparency handler lagged by {} messages", n);
                    continue;
                }
                Ok(Err(_)) | Err(_) => break, // Channel closed or timeout
            }
        }

        Err(IpcError::Timeout(
            "transparency response timed out".to_string(),
        ))
    }
}
