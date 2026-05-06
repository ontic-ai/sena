//! Helper for constructing a loaded llama backend from a specific model path.

use crate::backend::InferenceBackend;
use crate::error::InferenceError;
use crate::stream::InferenceStream;
use crate::types::{BackendType, InferenceParams};
use async_trait::async_trait;
use infer::InferenceBackend as InferBackendTrait;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

struct LlamaBackendAdapter {
    inner: Arc<Mutex<crate::LlamaBackend>>,
}

impl LlamaBackendAdapter {
    fn new(backend: crate::LlamaBackend) -> Self {
        Self {
            inner: Arc::new(Mutex::new(backend)),
        }
    }
}

#[async_trait]
impl InferenceBackend for LlamaBackendAdapter {
    fn backend_type(&self) -> BackendType {
        BackendType::LlamaCpp
    }

    fn is_loaded(&self) -> bool {
        true
    }

    async fn infer(
        &self,
        prompt: String,
        params: InferenceParams,
    ) -> Result<InferenceStream, InferenceError> {
        let infer_params = infer::InferenceParams {
            request_id: uuid::Uuid::new_v4(),
            prompt,
            temperature: params.temperature,
            top_p: params.top_p,
            max_tokens: params.max_tokens,
            ctx_size: 2048,
        };

        let backend_clone = self.inner.clone();
        let stream_rx = tokio::task::spawn_blocking(move || {
            let backend = backend_clone.blocking_lock();
            backend.stream(infer_params)
        })
        .await
        .map_err(|e| InferenceError::ExecutionFailed(format!("spawn_blocking failed: {}", e)))?
        .map_err(|e| InferenceError::ExecutionFailed(format!("stream failed: {}", e)))?;

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

    fn complete(&self, prompt: &str, params: &InferenceParams) -> Result<String, InferenceError> {
        let infer_params = infer::InferenceParams {
            request_id: uuid::Uuid::new_v4(),
            prompt: prompt.to_string(),
            temperature: params.temperature,
            top_p: params.top_p,
            max_tokens: params.max_tokens,
            ctx_size: 2048,
        };

        let backend = self
            .inner
            .try_lock()
            .map_err(|_| InferenceError::ExecutionFailed("backend busy".to_string()))?;
        backend
            .complete(&infer_params)
            .map_err(|e| InferenceError::ExecutionFailed(format!("complete failed: {}", e)))
    }

    async fn shutdown(&mut self) -> Result<(), InferenceError> {
        Ok(())
    }
}

pub fn preferred_llama_backend() -> infer::BackendType {
    #[cfg(all(any(target_os = "windows", target_os = "linux"), feature = "vulkan"))]
    {
        infer::BackendType::Vulkan
    }

    #[cfg(all(
        not(all(any(target_os = "windows", target_os = "linux"), feature = "vulkan")),
        target_os = "macos",
        feature = "metal"
    ))]
    {
        infer::BackendType::Metal
    }

    #[cfg(all(
        not(all(any(target_os = "windows", target_os = "linux"), feature = "vulkan")),
        not(all(target_os = "macos", feature = "metal")),
        any(target_os = "windows", target_os = "linux"),
        feature = "cuda"
    ))]
    {
        infer::BackendType::Cuda
    }

    #[cfg(not(any(
        all(any(target_os = "windows", target_os = "linux"), feature = "vulkan"),
        all(target_os = "macos", feature = "metal"),
        all(any(target_os = "windows", target_os = "linux"), feature = "cuda")
    )))]
    {
        infer::BackendType::auto_detect()
    }
}

pub fn build_loaded_llama_backend(
    model_path: &Path,
) -> Result<Box<dyn InferenceBackend>, InferenceError> {
    let mut backend = crate::LlamaBackend::new()
        .map_err(|e| InferenceError::BackendInit(format!("llama backend init failed: {}", e)))?;

    let backend_type = preferred_llama_backend();
    backend.load_model(model_path, backend_type).map_err(|e| {
        InferenceError::BackendFailed(format!(
            "failed to load model from {}: {}",
            model_path.display(),
            e
        ))
    })?;

    Ok(Box::new(LlamaBackendAdapter::new(backend)))
}
