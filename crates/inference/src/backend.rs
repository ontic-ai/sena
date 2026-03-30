//! LLM backend abstraction trait.
//!
//! Defines the interface that any LLM backend (llama-cpp-rs, mock, etc.)
//! must implement. Allows swapping backends without changing actor logic.

use std::path::Path;

/// Compute backend type for model inference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendType {
    /// Apple Metal (macOS, Apple Silicon).
    Metal,
    /// NVIDIA CUDA (Windows/Linux).
    Cuda,
    /// CPU fallback (all platforms).
    Cpu,
}

impl std::fmt::Display for BackendType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendType::Metal => write!(f, "Metal"),
            BackendType::Cuda => write!(f, "CUDA"),
            BackendType::Cpu => write!(f, "CPU"),
        }
    }
}

/// Parameters for inference generation.
#[derive(Debug, Clone)]
pub struct InferenceParams {
    /// Temperature for sampling (0.0 = deterministic, 1.0 = creative).
    pub temperature: f32,
    /// Top-p nucleus sampling threshold.
    pub top_p: f32,
    /// Maximum tokens to generate.
    pub max_tokens: usize,
}

impl Default for InferenceParams {
    fn default() -> Self {
        Self {
            temperature: 0.7,
            top_p: 0.9,
            max_tokens: 2048,
        }
    }
}

/// Errors from LLM backend operations.
#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    /// Model file not found or unreadable.
    #[error("model load failed: {0}")]
    ModelLoadFailed(String),

    /// Inference execution failed.
    #[error("inference failed: {0}")]
    InferenceFailed(String),

    /// Embedding generation failed.
    #[error("embedding failed: {0}")]
    EmbeddingFailed(String),

    /// Extraction failed.
    #[error("extraction failed: {0}")]
    ExtractionFailed(String),

    /// Backend not initialized (model not loaded yet).
    #[error("backend not initialized — model must be loaded first")]
    NotInitialized,
}

/// Trait for LLM backend implementations.
///
/// All methods are synchronous — callers must use `spawn_blocking`
/// to avoid blocking the async runtime.
pub trait LlmBackend: Send + 'static {
    /// Load model weights from the given GGUF path.
    fn load_model(
        &mut self,
        model_path: &Path,
        backend_type: BackendType,
    ) -> Result<(), BackendError>;

    /// Run text generation inference.
    fn infer(&self, prompt: &str, params: &InferenceParams) -> Result<String, BackendError>;

    /// Generate embedding vector for the given text.
    fn embed(&self, text: &str) -> Result<Vec<f32>, BackendError>;

    /// Extract structured facts from the given text.
    fn extract(&self, text: &str) -> Result<Vec<String>, BackendError>;

    /// Returns true if a model has been loaded.
    fn is_loaded(&self) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_type_display() {
        assert_eq!(BackendType::Metal.to_string(), "Metal");
        assert_eq!(BackendType::Cuda.to_string(), "CUDA");
        assert_eq!(BackendType::Cpu.to_string(), "CPU");
    }

    #[test]
    fn inference_params_default() {
        let params = InferenceParams::default();
        assert!((params.temperature - 0.7).abs() < f32::EPSILON);
        assert!((params.top_p - 0.9).abs() < f32::EPSILON);
        assert_eq!(params.max_tokens, 2048);
    }

    #[test]
    fn backend_error_variants() {
        let e = BackendError::ModelLoadFailed("bad path".to_string());
        assert!(e.to_string().contains("bad path"));

        let e = BackendError::InferenceFailed("oom".to_string());
        assert!(e.to_string().contains("oom"));

        let e = BackendError::NotInitialized;
        assert!(e.to_string().contains("not initialized"));
    }

    // Compile-time check: BackendType is Send
    #[allow(dead_code)]
    fn assert_send<T: Send>() {}

    #[test]
    fn types_are_send() {
        assert_send::<BackendType>();
        assert_send::<InferenceParams>();
        assert_send::<BackendError>();
    }
}
