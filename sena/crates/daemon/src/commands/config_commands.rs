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
        "Get full runtime configuration"
    }

    async fn handle(&self, _payload: Value) -> Result<Value, IpcError> {
        let config = runtime::load_or_create_config()
            .await
            .map_err(|e| IpcError::Internal(format!("failed to load config: {}", e)))?;

        Ok(json!({
            "file_watch_paths": config
                .file_watch_paths
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>(),
            "clipboard_observation_enabled": config.clipboard_observation_enabled,
            "speech_enabled": config.speech_enabled,
            "inference_max_tokens": config.inference_max_tokens,
            "auto_tune_tokens": config.auto_tune_tokens,
            "auto_tune_min_tokens": config.auto_tune_min_tokens,
            "auto_tune_max_tokens": config.auto_tune_max_tokens,
        }))
    }
}

/// Handler for "config.set" command.
pub struct ConfigSetHandler {
    bus: std::sync::Arc<bus::EventBus>,
}

impl ConfigSetHandler {
    pub fn new(bus: std::sync::Arc<bus::EventBus>) -> Self {
        Self { bus }
    }
}

#[async_trait]
impl CommandHandler for ConfigSetHandler {
    fn name(&self) -> &'static str {
        "config.set"
    }

    fn description(&self) -> &'static str {
        "Set configuration value by dotted path"
    }

    async fn handle(&self, payload: Value) -> Result<Value, IpcError> {
        let path = payload
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                IpcError::InvalidPayload("missing or invalid 'path' field".to_string())
            })?;

        let value = payload
            .get("value")
            .ok_or_else(|| IpcError::InvalidPayload("missing 'value' field".to_string()))?;

        if is_non_editable_path(path) {
            return Err(IpcError::InvalidPayload(format!(
                "path '{}' is managed by runtime and cannot be edited",
                path
            )));
        }

        let value_string = match value {
            Value::String(s) => s.clone(),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => n.to_string(),
            Value::Array(values) if path == "file_watch_paths" => values
                .iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(";"),
            _ => {
                return Err(IpcError::InvalidPayload(format!(
                    "unsupported value type for path '{}'",
                    path
                )));
            }
        };

        runtime::config::apply_config_set(path, &value_string)
            .await
            .map_err(IpcError::CommandFailed)?;

        let _ = self
            .bus
            .broadcast(bus::Event::System(bus::SystemEvent::ConfigUpdated {
                path: path.to_string(),
            }))
            .await;

        Ok(json!({
            "path": path,
            "saved": true,
        }))
    }
}

fn is_non_editable_path(path: &str) -> bool {
    path == "models_dir"
        || path.starts_with("crypto.")
        || path.starts_with("bus.")
        || path.ends_with("_version")
        || path.ends_with("_schema")
}
