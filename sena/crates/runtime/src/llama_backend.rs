//! LlamaBackend construction helper for runtime.
//!
//! Provides strict Llama backend construction for runtime boot.
//!
//! Runtime boot must either load a real GGUF model or fail. This module
//! delegates llama construction to the inference crate so boot-time and
//! hot-load inference share one infer-backed implementation.

use crate::error::RuntimeError;
use std::path::Path;
use tracing::{info, warn};

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
    inference::build_loaded_llama_backend(model_path).map_err(|error| {
        RuntimeError::ModelLoadFailed(format!(
            "failed to load model from {}: {}",
            model_path.display(),
            error
        ))
    })
}

/// Discover a usable model and construct a backend.
///
/// Uses `inference::discover_models()` to scan the default models directory for
/// GGUF files and attempts to load the first one found. Runtime boot is strict:
/// if no usable model is available, it returns an error instead of falling back.
pub fn build_default_backend() -> Result<Box<dyn inference::InferenceBackend>, RuntimeError> {
    let models_dir = infer::ollama_models_dir()
        .map_err(|e| RuntimeError::DirectoryResolutionFailed(e.to_string()))?;

    if !models_dir.exists() {
        return Err(RuntimeError::RequiredModelMissing {
            model_name: "gguf model".to_string(),
            reason: format!("models directory does not exist: {}", models_dir.display()),
        });
    }

    info!(path = ?models_dir, "scanning for GGUF models");
    let registry = inference::discover_models(&models_dir)
        .map_err(|e| RuntimeError::ModelLoadFailed(format!("model discovery failed: {}", e)))?;

    info!(
        count = registry.len(),
        "discovered {} model(s)",
        registry.len()
    );

    if registry.models.is_empty() {
        return Err(RuntimeError::RequiredModelMissing {
            model_name: "gguf model".to_string(),
            reason: format!("no GGUF models found in {}", models_dir.display()),
        });
    }

    let mut last_error = None;

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
                return Ok(backend);
            }
            Err(e) => {
                warn!(
                    name = %model.name,
                    path = ?model.path,
                    error = %e,
                    "failed to load model — trying next"
                );
                last_error = Some(e.to_string());
                continue;
            }
        }
    }

    Err(RuntimeError::ModelLoadFailed(format!(
        "no usable GGUF models could be loaded from {}: {}",
        models_dir.display(),
        last_error.unwrap_or_else(|| "all discovered models failed".to_string())
    )))
}

#[cfg(test)]
mod tests {
    #[test]
    fn preferred_backend_type_is_selectable() {
        let backend = inference::preferred_llama_backend();
        assert!(!backend.to_string().is_empty());
    }
}
