//! Re-exports for model discovery — lets CLI discover models via runtime without
//! importing inference or platform directly.

pub use inference::{discover_models, InferenceError, ModelRegistry};

/// Returns the Ollama models directory for the current platform.
///
/// Wraps `platform::ollama_models_dir()` and maps the error to `InferenceError`.
pub fn ollama_models_dir() -> Result<std::path::PathBuf, InferenceError> {
    platform::ollama_models_dir().map_err(|e| InferenceError::ModelLoadFailed(e.to_string()))
}
