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

        let config = runtime::load_or_create_config()
            .await
            .map_err(|e| IpcError::Internal(format!("failed to load config: {}", e)))?;

        let value = match key {
            "file_watch_paths" => json!(
                config
                    .file_watch_paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
            ),
            "clipboard_observation_enabled" => json!(config.clipboard_observation_enabled),
            "speech_enabled" => json!(config.speech_enabled),
            "inference_max_tokens" => json!(config.inference_max_tokens),
            "auto_tune_tokens" => json!(config.auto_tune_tokens),
            "auto_tune_min_tokens" => json!(config.auto_tune_min_tokens),
            "auto_tune_max_tokens" => json!(config.auto_tune_max_tokens),
            _ => {
                return Err(IpcError::InvalidPayload(format!(
                    "unknown config key '{}'",
                    key
                )));
            }
        };

        Ok(json!({ "key": key, "value": value }))
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

        let value = payload
            .get("value")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                IpcError::InvalidPayload("missing or invalid 'value' field".to_string())
            })?;

        runtime::config::apply_config_set(key, value)
            .await
            .map_err(IpcError::CommandFailed)?;

        Ok(json!({
            "key": key,
            "value": value,
            "saved": true,
            "restart_required": true,
        }))
    }
}
