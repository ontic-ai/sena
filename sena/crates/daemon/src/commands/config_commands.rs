//! Config-related IPC command handlers.

use async_trait::async_trait;
use ipc::{CommandHandler, IpcError};
use serde_json::{Value, json};

/// Handler for "config.get" command.
pub struct ConfigGetHandler;

#[async_trait]
impl CommandHandler for ConfigGetHandler {
    fn name(&self) -> &'static str {
        "config.get"
    }

    fn description(&self) -> &'static str {
        "Get configuration value by key"
    }

    async fn handle(&self, payload: Value) -> Result<Value, IpcError> {
        let key = payload.get("key").and_then(|v| v.as_str()).ok_or_else(|| {
            IpcError::InvalidPayload("missing or invalid 'key' field".to_string())
        })?;

        // Phase 2 limitation: no runtime config subsystem yet.
        // Return null value with note.
        Ok(json!({
            "key": key,
            "value": null,
            "note": "Config subsystem not yet implemented"
        }))
    }
}

/// Handler for "config.set" command.
pub struct ConfigSetHandler;

#[async_trait]
impl CommandHandler for ConfigSetHandler {
    fn name(&self) -> &'static str {
        "config.set"
    }

    fn description(&self) -> &'static str {
        "Set configuration value by key"
    }

    async fn handle(&self, payload: Value) -> Result<Value, IpcError> {
        let key = payload.get("key").and_then(|v| v.as_str()).ok_or_else(|| {
            IpcError::InvalidPayload("missing or invalid 'key' field".to_string())
        })?;

        let _value = payload
            .get("value")
            .ok_or_else(|| IpcError::InvalidPayload("missing 'value' field".to_string()))?;

        // Phase 2 limitation: no runtime config subsystem yet.
        Err(IpcError::CommandNotReady(format!(
            "Config write not yet implemented (key: {})",
            key
        )))
    }
}
