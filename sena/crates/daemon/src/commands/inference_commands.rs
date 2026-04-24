//! Inference-related IPC command handlers.

use async_trait::async_trait;
use bus::{CausalId, Event, EventBus, InferenceEvent, InferenceSource, Priority};
use ipc::{CommandHandler, IpcError};
use serde_json::{Value, json};
use std::sync::Arc;

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
        let models_dir = runtime::ollama_models_dir().map_err(|e| {
            IpcError::CommandFailed(format!("unable to resolve Ollama models dir: {}", e))
        })?;

        let registry = runtime::discover_models(&models_dir)
            .map_err(|e| IpcError::CommandFailed(format!("model discovery failed: {}", e)))?;

        let models: Vec<Value> = registry
            .models
            .iter()
            .map(|m| {
                json!({
                    "name": m.name,
                    "path": m.path,
                    "size_bytes": m.size_bytes,
                })
            })
            .collect();

        Ok(json!({
            "models_dir": models_dir,
            "models": models,
            "default_model": registry.models.iter().max_by_key(|m| m.size_bytes).map(|m| m.name.clone())
        }))
    }
}

/// Handler for "inference.load_model" command.
pub struct LoadModelHandler {
    bus: Arc<EventBus>,
}

impl LoadModelHandler {
    pub fn new(bus: Arc<EventBus>) -> Self {
        Self { bus }
    }
}

#[async_trait]
impl CommandHandler for LoadModelHandler {
    fn name(&self) -> &'static str {
        "inference.load_model"
    }

    fn description(&self) -> &'static str {
        "Load an inference model by filesystem path"
    }

    async fn handle(&self, payload: Value) -> Result<Value, IpcError> {
        let model_path = payload
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                IpcError::InvalidPayload("missing or invalid 'path' field".to_string())
            })?;

        let path = std::path::Path::new(model_path);
        if !path.exists() {
            return Err(IpcError::InvalidPayload(format!(
                "model path does not exist: {}",
                model_path
            )));
        }

        let causal_id = CausalId::new();
        let mut rx = self.bus.subscribe_broadcast();

        self.bus
            .broadcast(Event::Inference(InferenceEvent::ModelLoadRequested {
                model_path: model_path.to_string(),
                causal_id,
            }))
            .await
            .map_err(|e| IpcError::CommandFailed(e.to_string()))?;

        let wait_result = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                match rx.recv().await {
                    Ok(Event::Inference(InferenceEvent::ModelLoaded {
                        model_path,
                        model_name,
                        causal_id: event_causal_id,
                    })) if event_causal_id == causal_id => {
                        return Ok(json!({
                            "status": "loaded",
                            "model_path": model_path,
                            "model_name": model_name,
                            "causal_id": causal_id.as_u64(),
                        }));
                    }
                    Ok(Event::Inference(InferenceEvent::ModelLoadFailed {
                        model_path,
                        reason,
                        causal_id: event_causal_id,
                    })) if event_causal_id == causal_id => {
                        return Err(IpcError::CommandFailed(format!(
                            "failed to load {}: {}",
                            model_path, reason
                        )));
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
                "timed out waiting for model load result".to_string(),
            )),
        }
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
        let backend = runtime::auto_detect_backend_name();
        let models_dir = runtime::ollama_models_dir().ok();

        let (model_loaded, model_name) = if let Some(dir) = &models_dir {
            match runtime::discover_models(dir) {
                Ok(registry) => {
                    let default = registry
                        .models
                        .iter()
                        .max_by_key(|m| m.size_bytes)
                        .map(|m| m.name.clone());
                    (default.is_some(), default)
                }
                Err(_) => (false, None),
            }
        } else {
            (false, None)
        };

        Ok(json!({
            "model_loaded": model_loaded,
            "model_name": model_name,
            "backend": backend,
            "models_dir": models_dir,
        }))
    }
}

/// Handler for "inference.run" command.
pub struct RunInferenceHandler {
    bus: Arc<EventBus>,
}

impl RunInferenceHandler {
    pub fn new(bus: Arc<EventBus>) -> Self {
        Self { bus }
    }
}

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

        let causal_id = CausalId::new();

        self.bus
            .broadcast(Event::Inference(InferenceEvent::InferenceRequested {
                prompt: prompt.to_string(),
                priority: Priority::Normal,
                source: InferenceSource::UserText,
                causal_id,
            }))
            .await
            .map_err(|e| IpcError::CommandFailed(e.to_string()))?;

        Ok(json!({
            "status": "requested",
            "prompt_chars": prompt.len(),
            "causal_id": causal_id.as_u64(),
        }))
    }
}
