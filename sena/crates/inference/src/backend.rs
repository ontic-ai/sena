//! InferenceBackend trait definition.

use crate::error::InferenceError;
use crate::stream::InferenceStream;
use crate::types::{BackendType, InferenceParams};
use async_trait::async_trait;

/// Trait for inference backend implementations.
#[async_trait]
pub trait InferenceBackend: Send + Sync {
    /// Return the backend type.
    fn backend_type(&self) -> BackendType;

    /// Check if a model is currently loaded.
    fn is_loaded(&self) -> bool;

    /// Run inference with the given prompt and parameters, returning a token stream.
    async fn infer(
        &self,
        prompt: String,
        params: InferenceParams,
    ) -> Result<InferenceStream, InferenceError>;

    /// Shutdown the backend gracefully.
    async fn shutdown(&mut self) -> Result<(), InferenceError>;
}
