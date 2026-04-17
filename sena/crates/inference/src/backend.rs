//! InferenceBackend trait definition.

use crate::error::InferenceError;
use crate::stream::InferenceStream;
use crate::types::{BackendType, InferenceParams};
use async_trait::async_trait;

/// Trait for inference backend implementations.
///
/// Backends provide lazy model state management plus three core operations:
/// - `infer`: text generation from prompts
/// - `embed`: generate embedding vectors for text (optional, default returns ModelNotLoaded)
/// - `extract`: extract structured facts from text (optional, default returns ModelNotLoaded)
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

    /// Run non-streaming completion and return full generated text.
    ///
    /// Default implementation returns an execution error for backends that only
    /// support streaming.
    fn complete(&self, _prompt: &str, _params: &InferenceParams) -> Result<String, InferenceError> {
        Err(InferenceError::ExecutionFailed(
            "non-streaming completion not supported".to_string(),
        ))
    }

    /// Generate an embedding vector for the given text.
    ///
    /// Default implementation returns ModelNotLoaded error.
    /// Override if your backend supports embeddings.
    async fn embed(&self, _text: String) -> Result<Vec<f32>, InferenceError> {
        Err(InferenceError::ModelNotLoaded)
    }

    /// Extract structured facts or information from the given text.
    ///
    /// Default implementation returns ModelNotLoaded error.
    /// Override if your backend supports extraction.
    async fn extract(&self, _text: String) -> Result<String, InferenceError> {
        Err(InferenceError::ModelNotLoaded)
    }

    /// Shutdown the backend gracefully.
    async fn shutdown(&mut self) -> Result<(), InferenceError>;

    /// Get current VRAM usage in megabytes.
    ///
    /// Returns (used_mb, total_mb, percent) tuple.
    /// Default implementation returns (0, 0, 0) for backends without VRAM tracking.
    fn vram_usage(&self) -> (u32, u32, u8) {
        (0, 0, 0)
    }
}
