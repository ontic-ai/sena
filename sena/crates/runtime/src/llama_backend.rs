//! LlamaBackend construction helper for runtime.
//!
//! Provides `build_llama_backend` which attempts to construct a real
//! `infer::LlamaBackend` wrapped in an adapter that implements Sena's
//! `inference::InferenceBackend` trait. Falls back gracefully if no model
//! is available.

use crate::error::RuntimeError;
use async_trait::async_trait;
use inference::{BackendType, InferenceError, InferenceParams, InferenceStream, LlmBackend};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};
use tracing::{info, warn};

/// Adapter that wraps `infer::LlamaBackend` and implements `inference::InferenceBackend`.
///
/// This bridges the two trait definitions so we can use the external `infer` crate's
/// LlamaBackend with Sena's InferenceActor.
struct LlamaBackendAdapter {
    inner: Arc<Mutex<inference::LlamaBackend>>,
}

impl LlamaBackendAdapter {
    /// Create a new adapter from a constructed and loaded LlamaBackend.
    fn new(backend: inference::LlamaBackend) -> Self {
        Self {
            inner: Arc::new(Mutex::new(backend)),
        }
    }
}

#[async_trait]
impl inference::InferenceBackend for LlamaBackendAdapter {
    fn backend_type(&self) -> BackendType {
        // All real backends map to LlamaCpp in Sena's type system
        BackendType::LlamaCpp
    }

    fn is_loaded(&self) -> bool {
        // If we've constructed the adapter, the model is loaded
        true
    }

    async fn infer(
        &self,
        prompt: String,
        params: InferenceParams,
    ) -> Result<InferenceStream, InferenceError> {
        // Convert Sena's InferenceParams to infer crate's params
        let infer_params = infer::InferenceParams {
            request_id: uuid::Uuid::new_v4(),
            prompt: prompt.clone(),
            temperature: params.temperature,
            top_p: params.top_p,
            max_tokens: params.max_tokens,
            ctx_size: 2048, // Default context size
        };

        // Run inference in a blocking task since the infer crate's methods are sync
        let backend_clone = self.inner.clone();
        let stream_rx = tokio::task::spawn_blocking(move || {
            let backend = backend_clone.blocking_lock();
            backend.stream(infer_params)
        })
        .await
        .map_err(|e| InferenceError::ExecutionFailed(format!("spawn_blocking failed: {}", e)))?
        .map_err(|e| InferenceError::ExecutionFailed(format!("stream failed: {}", e)))?;

        // Convert std::sync::mpsc::Receiver to tokio::sync::mpsc::Receiver
        // and wrap in InferenceStream
        let (tx, rx) = mpsc::channel(100);

        tokio::task::spawn_blocking(move || {
            while let Ok(token) = stream_rx.recv() {
                if tx.blocking_send(Ok(token)).is_err() {
                    break;
                }
            }
        });

        Ok(InferenceStream::new(rx))
    }

    async fn shutdown(&mut self) -> Result<(), InferenceError> {
        // LlamaBackend doesn't have an explicit shutdown method; drop handles cleanup
        Ok(())
    }
}

/// Attempt to construct a real LlamaBackend from a model path.
///
/// Returns a boxed `InferenceBackend` trait object on success.
/// If construction fails, returns a RuntimeError.
///
/// # Parameters
/// - `model_path`: Path to the GGUF model file
///
/// # Errors
/// - `ModelLoadFailed` if backend construction or model loading fails
pub fn build_llama_backend(
    model_path: &Path,
) -> Result<Box<dyn inference::InferenceBackend>, RuntimeError> {
    info!(path = ?model_path, "attempting to load LlamaBackend");

    // Construct the backend
    let mut backend = inference::LlamaBackend::new()
        .map_err(|e| RuntimeError::ModelLoadFailed(format!("backend init failed: {}", e)))?;

    // Auto-detect the best backend type (Metal on macOS, CUDA if available, CPU otherwise)
    let backend_type = infer::BackendType::auto_detect();
    info!(backend_type = ?backend_type, "detected compute backend");

    // Load the model
    backend.load_model(model_path, backend_type).map_err(|e| {
        RuntimeError::ModelLoadFailed(format!(
            "failed to load model from {}: {}",
            model_path.display(),
            e
        ))
    })?;

    info!(path = ?model_path, backend = ?backend_type, "LlamaBackend loaded successfully");

    // Wrap in adapter
    Ok(Box::new(LlamaBackendAdapter::new(backend)))
}

/// Discover a usable model and construct a backend, or return None if unavailable.
///
/// Uses `inference::discover_models()` to scan the default models directory for
/// GGUF files and attempts to load the first one found. Returns None (with a warning)
/// if no models are discovered.
///
/// Graceful fallback: boot remains non-fatal if no models are available.
pub fn try_build_default_backend() -> Option<Box<dyn inference::InferenceBackend>> {
    // Resolve the default Ollama models directory.
    let models_dir = infer::ollama_models_dir().ok()?;

    if !models_dir.exists() {
        warn!(
            path = ?models_dir,
            "default models directory does not exist — no backend available"
        );
        return None;
    }

    // Use inference crate's discovery system
    info!(path = ?models_dir, "scanning for GGUF models");
    let registry = match inference::discover_models(&models_dir) {
        Ok(registry) => registry,
        Err(e) => {
            warn!(path = ?models_dir, error = %e, "model discovery failed");
            return None;
        }
    };

    info!(
        count = registry.len(),
        "discovered {} model(s)",
        registry.len()
    );

    // Attempt to load models in order until one succeeds
    for model in &registry.models {
        info!(
            name = %model.name,
            path = ?model.path,
            size_mb = model.size_bytes / (1024 * 1024),
            "attempting to load model"
        );

        match build_llama_backend(&model.path) {
            Ok(backend) => {
                info!(name = %model.name, "model loaded successfully");
                return Some(backend);
            }
            Err(e) => {
                warn!(
                    name = %model.name,
                    path = ?model.path,
                    error = %e,
                    "failed to load model — trying next"
                );
                continue;
            }
        }
    }

    warn!("no usable GGUF models could be loaded — all models failed");
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_try_build_default_backend_returns_none_when_no_models() {
        // This test will return None because the models directory likely doesn't exist
        // in test environment. Just verify it doesn't panic.
        let result = try_build_default_backend();
        assert!(result.is_none());
    }
}
