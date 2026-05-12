//! Helper for constructing loaded infer-backed llama adapters.

use crate::backend::InferenceBackend;
use crate::error::InferenceError;
use crate::stream::InferenceStream;
use crate::types::{BackendType, InferenceParams};
use async_trait::async_trait;
use infer::InferenceBackend as InferBackendTrait;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};
use tracing::info;

const DEFAULT_CTX_SIZE: u32 = 2048;

fn to_infer_params(prompt: String, params: InferenceParams) -> infer::InferenceParams {
    infer::InferenceParams {
        request_id: uuid::Uuid::new_v4(),
        prompt,
        temperature: params.temperature,
        top_k: params.top_k,
        top_p: params.top_p,
        repeat_penalty: params.repeat_penalty,
        max_tokens: params.max_tokens,
        ctx_size: DEFAULT_CTX_SIZE,
        stop_sequences: params.stop_sequences,
        kv_cache: infer::KvCacheConfig::none(),
    }
}

fn to_infer_params_ref(prompt: &str, params: &InferenceParams) -> infer::InferenceParams {
    infer::InferenceParams {
        request_id: uuid::Uuid::new_v4(),
        prompt: prompt.to_string(),
        temperature: params.temperature,
        top_k: params.top_k,
        top_p: params.top_p,
        repeat_penalty: params.repeat_penalty,
        max_tokens: params.max_tokens,
        ctx_size: DEFAULT_CTX_SIZE,
        stop_sequences: params.stop_sequences.clone(),
        kv_cache: infer::KvCacheConfig::none(),
    }
}

struct LlamaBackendAdapter {
    inner: Arc<Mutex<infer::LlamaBackend>>,
}

impl LlamaBackendAdapter {
    fn new(backend: infer::LlamaBackend) -> Self {
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
        let infer_params = to_infer_params(prompt, params);
        let backend_clone = Arc::clone(&self.inner);
        let stream_rx = tokio::task::spawn_blocking(move || {
            let backend = backend_clone.blocking_lock();
            backend.stream(infer_params)
        })
        .await
        .map_err(|error| InferenceError::ExecutionFailed(format!("spawn_blocking failed: {}", error)))?
        .map_err(|error| InferenceError::ExecutionFailed(format!("stream failed: {}", error)))?;

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
        let infer_params = to_infer_params_ref(prompt, params);
        let backend = self
            .inner
            .try_lock()
            .map_err(|_| InferenceError::ExecutionFailed("backend busy".to_string()))?;
        backend
            .complete(&infer_params)
            .map_err(|error| InferenceError::ExecutionFailed(format!("complete failed: {}", error)))
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
    let backend_type = preferred_llama_backend();
    let model_size_mb = std::fs::metadata(model_path)
        .map(|metadata| metadata.len() / (1024 * 1024))
        .unwrap_or(0);
    info!(
        compute_backend = %backend_type,
        model_path = %model_path.display(),
        model_size_mb,
        "loading infer llama generation model"
    );

    let mut backend = infer::LlamaBackend::new()
        .map_err(|error| InferenceError::BackendInit(format!("llama backend init failed: {}", error)))?;
    backend.load_model(model_path, backend_type).map_err(|error| {
        InferenceError::BackendFailed(format!(
            "failed to load model from {}: {}",
            model_path.display(),
            error
        ))
    })?;

    info!(
        compute_backend = %backend_type,
        model_path = %model_path.display(),
        model_size_mb,
        "infer llama generation model loaded"
    );

    Ok(Box::new(LlamaBackendAdapter::new(backend)))
}

struct LlamaEmbedBackendAdapter {
    inner: Arc<Mutex<infer::LlamaEmbedBackend>>,
}

impl LlamaEmbedBackendAdapter {
    fn new(backend: infer::LlamaEmbedBackend) -> Self {
        Self {
            inner: Arc::new(Mutex::new(backend)),
        }
    }
}

#[async_trait]
impl InferenceBackend for LlamaEmbedBackendAdapter {
    fn backend_type(&self) -> BackendType {
        BackendType::LlamaCpp
    }

    fn is_loaded(&self) -> bool {
        true
    }

    async fn infer(
        &self,
        _prompt: String,
        _params: InferenceParams,
    ) -> Result<InferenceStream, InferenceError> {
        Err(InferenceError::ExecutionFailed(
            "LlamaEmbedBackend is an embedding-only backend".to_string(),
        ))
    }

    async fn embed(&self, text: String) -> Result<Vec<f32>, InferenceError> {
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || {
            let backend = inner.blocking_lock();
            backend
                .embed(&text)
                .map_err(|error| InferenceError::ExecutionFailed(format!("embed failed: {}", error)))
        })
        .await
        .map_err(|error| InferenceError::ExecutionFailed(format!("embed: spawn_blocking: {error}")))?
    }

    async fn shutdown(&mut self) -> Result<(), InferenceError> {
        Ok(())
    }
}

pub fn build_loaded_embed_backend(
    model_path: &Path,
) -> Result<Box<dyn InferenceBackend>, InferenceError> {
    let backend_type = preferred_llama_backend();
    let backend = infer::LlamaEmbedBackend::load(model_path, backend_type).map_err(|error| {
        InferenceError::BackendFailed(format!(
            "embed: failed to load model {}: {}",
            model_path.display(),
            error
        ))
    })?;

    Ok(Box::new(LlamaEmbedBackendAdapter::new(backend)))
}

#[cfg(test)]
mod tests {
    #[test]
    fn preferred_backend_type_is_selectable() {
        let backend = super::preferred_llama_backend();
        assert!(!backend.to_string().is_empty());
    }
}