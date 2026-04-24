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

    async fn shutdown(&mut self) -> Result<(), InferenceError> {
        Ok(())
    }
}

pub fn build_loaded_llama_backend(
    model_path: &Path,
) -> Result<Box<dyn InferenceBackend>, InferenceError> {
    let mut backend = crate::LlamaBackend::new().map_err(|e| {
        InferenceError::BackendInit(format!("llama backend init failed: {}", e))
    })?;

    let backend_type = infer::BackendType::auto_detect();
    backend.load_model(model_path, backend_type).map_err(|e| {
        InferenceError::BackendFailed(format!(
            "failed to load model from {}: {}",
            model_path.display(),
            e
        ))
    })?;

    Ok(Box::new(LlamaBackendAdapter::new(backend)))
}