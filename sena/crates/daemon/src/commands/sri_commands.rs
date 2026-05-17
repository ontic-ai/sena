//! SRI visualization IPC command handlers.

use async_trait::async_trait;
use ipc::{CommandHandler, IpcError};
use serde_json::{Value, json};
use sri::SriState;

pub struct SriSubscribeHandler;

#[async_trait]
impl CommandHandler for SriSubscribeHandler {
    fn name(&self) -> &'static str {
        "sri.subscribe"
    }

    fn description(&self) -> &'static str {
        "Subscribe to the SRI visualization stream"
    }

    fn requires_boot(&self) -> bool {
        false
    }

    async fn handle(&self, _payload: Value) -> Result<Value, IpcError> {
        Ok(json!({
            "subscribed": true,
            "stream": "sri"
        }))
    }
}

pub struct SriUnsubscribeHandler;

#[async_trait]
impl CommandHandler for SriUnsubscribeHandler {
    fn name(&self) -> &'static str {
        "sri.unsubscribe"
    }

    fn description(&self) -> &'static str {
        "Unsubscribe from the SRI visualization stream"
    }

    fn requires_boot(&self) -> bool {
        false
    }

    async fn handle(&self, _payload: Value) -> Result<Value, IpcError> {
        Ok(json!({
            "subscribed": false,
            "stream": "sri"
        }))
    }
}

pub struct SriSnapshotHandler {
    state: SriState,
}

impl SriSnapshotHandler {
    pub fn new(state: SriState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl CommandHandler for SriSnapshotHandler {
    fn name(&self) -> &'static str {
        "sri.snapshot"
    }

    fn description(&self) -> &'static str {
        "Return the current SRI snapshot"
    }

    fn requires_boot(&self) -> bool {
        false
    }

    async fn handle(&self, _payload: Value) -> Result<Value, IpcError> {
        let snapshot = self.state.snapshot();
        let snapshot = serde_json::to_value(snapshot)
            .map_err(|error| IpcError::Internal(error.to_string()))?;

        Ok(json!({
            "snapshot": snapshot
        }))
    }
}