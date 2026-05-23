//! SRI visualization IPC command handlers.

use crate::commands::runtime_commands::RuntimeState;
use async_trait::async_trait;
use ipc::{CommandHandler, IpcError};
use serde_json::{Value, json};
use sri::SriState;

pub struct SriSubscribeHandler {
    state: RuntimeState,
}

impl SriSubscribeHandler {
    pub fn new(state: RuntimeState) -> Self {
        Self { state }
    }
}

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
        self.state.ensure_actors_running(&["sri"]).await?;

        Ok(json!({
            "subscribed": true,
            "stream": "sri"
        }))
    }
}

pub struct SriUnsubscribeHandler {
    state: RuntimeState,
}

impl SriUnsubscribeHandler {
    pub fn new(state: RuntimeState) -> Self {
        Self { state }
    }
}

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
        self.state.ensure_actors_running(&["sri"]).await?;

        Ok(json!({
            "subscribed": false,
            "stream": "sri"
        }))
    }
}

pub struct SriSnapshotHandler {
    runtime_state: RuntimeState,
    state: Option<SriState>,
}

impl SriSnapshotHandler {
    pub fn new(runtime_state: RuntimeState, state: Option<SriState>) -> Self {
        Self {
            runtime_state,
            state,
        }
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
        self.runtime_state.ensure_actors_running(&["sri"]).await?;

        let state = self.state.as_ref().ok_or_else(|| {
            IpcError::CommandFailed("actor not running in this session: sri".to_string())
        })?;
        let snapshot = state.snapshot();
        let snapshot = serde_json::to_value(snapshot)
            .map_err(|error| IpcError::Internal(error.to_string()))?;

        Ok(json!({
            "snapshot": snapshot
        }))
    }
}
