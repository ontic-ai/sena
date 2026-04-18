//! Adapter bridge between Sena's async inference trait and ontic `infer` backends.
//!
//! This adapter wraps a loaded infer backend (typically llama via `auto_backend`)
//! and exposes Sena's async streaming/embed/extract contract used by InferenceActor.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tracing::{debug, info, warn};

use crate::backend::InferenceBackend;
use crate::error::InferenceError;
use crate::stream::InferenceStream;
use crate::types::{BackendType, InferenceParams};

/// Adapter over an already-constructed infer backend.
///
/// The inner backend is synchronous; this adapter bridges calls through
/// `spawn_blocking` where needed.
pub struct LlamaAdapter {
    inner: Arc<Mutex<Box<dyn infer::InferenceBackend + Send + Sync>>>,
}

impl LlamaAdapter {
    /// Wrap a loaded infer backend.
    pub fn from_infer_backend(inner: Box<dyn infer::InferenceBackend + Send + Sync>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(inner)),
        }
    }
}

fn to_infer_params(prompt: String, params: &InferenceParams) -> infer::InferenceParams {
    infer::InferenceParams {
        prompt,
        temperature: params.temperature,
        top_p: params.top_p,
        max_tokens: params.max_tokens,
        ..Default::default()
    }
}

#[async_trait]
impl InferenceBackend for LlamaAdapter {
    fn backend_type(&self) -> BackendType {
        BackendType::LlamaCpp
    }

    fn is_loaded(&self) -> bool {
        match self.inner.lock() {
            Ok(guard) => guard.is_loaded(),
            Err(_) => false,
        }
    }

    async fn infer(
        &self,
        prompt: String,
        params: InferenceParams,
    ) -> Result<InferenceStream, InferenceError> {
        let inner = Arc::clone(&self.inner);
        let infer_params = to_infer_params(prompt, &params);

        let (tx, stream) = InferenceStream::channel(256);

        tokio::task::spawn_blocking(move || {
            let guard = match inner.lock() {
                Ok(g) => g,
                Err(_) => {
                    let _ = tx.blocking_send(Err(InferenceError::ExecutionFailed(
                        "infer backend mutex poisoned".to_string(),
                    )));
                    return;
                }
            };

            match guard.stream(infer_params) {
                Ok(token_rx) => {
                    for token in token_rx {
                        if tx.blocking_send(Ok(token)).is_err() {
                            debug!("llama adapter: stream receiver dropped");
                            break;
                        }
                    }
                }
                Err(e) => {
                    warn!(error = %e, "llama adapter: stream failed");
                    let _ = tx.blocking_send(Err(InferenceError::ExecutionFailed(e.to_string())));
                }
            }
        });

        Ok(stream)
    }

    fn complete(&self, prompt: &str, params: &InferenceParams) -> Result<String, InferenceError> {
        let infer_params = to_infer_params(prompt.to_string(), params);
        let guard = self.inner.lock().map_err(|_| {
            InferenceError::ExecutionFailed("infer backend mutex poisoned".to_string())
        })?;

        guard
            .complete(&infer_params)
            .map_err(|e| InferenceError::ExecutionFailed(e.to_string()))
    }

    async fn embed(&self, text: String) -> Result<Vec<f32>, InferenceError> {
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || {
            let guard = inner.lock().map_err(|_| {
                InferenceError::ExecutionFailed("infer backend mutex poisoned".to_string())
            })?;
            guard
                .embed(&text)
                .map_err(|e| InferenceError::ExecutionFailed(e.to_string()))
        })
        .await
        .map_err(|e| InferenceError::ExecutionFailed(format!("embed task failed: {e}")))?
    }

    async fn extract(&self, text: String) -> Result<String, InferenceError> {
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || {
            let guard = inner.lock().map_err(|_| {
                InferenceError::ExecutionFailed("infer backend mutex poisoned".to_string())
            })?;
            let extracted = guard
                .extract(&text)
                .map_err(|e| InferenceError::ExecutionFailed(e.to_string()))?;

            serde_json::to_string(&extracted.facts).map_err(|e| {
                InferenceError::ExecutionFailed(format!("serialize facts failed: {e}"))
            })
        })
        .await
        .map_err(|e| InferenceError::ExecutionFailed(format!("extract task failed: {e}")))?
    }

    async fn shutdown(&mut self) -> Result<(), InferenceError> {
        info!("llama adapter: shutdown");
        Ok(())
    }

    fn vram_usage(&self) -> (u32, u32, u8) {
        // infer does not currently expose VRAM telemetry through the trait.
        (0, 0, 0)
    }
}
