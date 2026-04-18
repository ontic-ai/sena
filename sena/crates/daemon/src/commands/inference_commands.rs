//! Inference-related IPC command handlers.

use async_trait::async_trait;
use ipc::{CommandHandler, IpcError};
use serde_json::{Value, json};

/// Handler for "inference.list_models" command.
pub struct ListModelsHandler;

#[async_trait]
impl CommandHandler for ListModelsHandler {
    fn name(&self) -> &'static str {
        "inference.list_models"
    }

    fn description(&self) -> &'static str {
        "List available inference models"
    }

    async fn handle(&self, _payload: Value) -> Result<Value, IpcError> {
        // Phase 2 limitation: no runtime helper for model enumeration yet.
        // Return empty list with a note.
        Ok(json!({
            "models": [],
            "note": "Model enumeration not yet implemented"
        }))
    }
}

/// Handler for "inference.load_model" command.
pub struct LoadModelHandler;

#[async_trait]
impl CommandHandler for LoadModelHandler {
    fn name(&self) -> &'static str {
        "inference.load_model"
    }

    fn description(&self) -> &'static str {
        "Load an inference model by name"
    }

    async fn handle(&self, payload: Value) -> Result<Value, IpcError> {
        let model_name = payload
            .get("model")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                IpcError::InvalidPayload("missing or invalid 'model' field".to_string())
            })?;

        // Phase 2 limitation: model loading via bus events not yet wired.
        Err(IpcError::CommandNotReady(format!(
            "Model loading not yet implemented (model: {})",
            model_name
        )))
    }
}

/// Handler for "inference.status" command.
pub struct InferenceStatusHandler;

#[async_trait]
impl CommandHandler for InferenceStatusHandler {
    fn name(&self) -> &'static str {
        "inference.status"
    }

    fn description(&self) -> &'static str {
        "Get inference subsystem status"
    }

    async fn handle(&self, _payload: Value) -> Result<Value, IpcError> {
        // Phase 2 limitation: no runtime helper for inference actor status.
        Ok(json!({
            "model_loaded": false,
            "backend": "none",
            "note": "Inference status query not yet implemented"
        }))
    }
}

/// Handler for "inference.run" command.
pub struct RunInferenceHandler;

#[async_trait]
impl CommandHandler for RunInferenceHandler {
    fn name(&self) -> &'static str {
        "inference.run"
    }

    fn description(&self) -> &'static str {
        "Run inference with a prompt"
    }

    async fn handle(&self, payload: Value) -> Result<Value, IpcError> {
        let prompt = payload
            .get("prompt")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                IpcError::InvalidPayload("missing or invalid 'prompt' field".to_string())
            })?;

        // Phase 2 limitation: inference dispatch via bus events not yet wired.
        // Return an error indicating the capability is not ready.
        Err(IpcError::CommandNotReady(format!(
            "Inference dispatch not yet implemented (prompt: {} chars)",
            prompt.len()
        )))
    }
}
