//! Model discovery re-exports for CLI.
//!
//! The CLI crate depends only on runtime per architecture.md §2.
//! This module re-exports model discovery functions from inference and platform
//! to avoid direct CLI → inference/platform dependencies.

use std::path::PathBuf;

pub use inference::{discover_models, InferenceError, ModelRegistry};

/// Get the Ollama models directory path.
///
/// Re-exported from `platform::dirs::ollama_models_dir`.
///
/// # Errors
/// Returns `InferenceError` if the platform-specific directory cannot be determined.
pub fn ollama_models_dir() -> Result<PathBuf, InferenceError> {
    platform::dirs::ollama_models_dir().map_err(|e| InferenceError::ModelLoadFailed(e.to_string()))
}
