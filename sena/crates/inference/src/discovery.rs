//! Model discovery orchestration.
//!
//! Discovers GGUF models in the Ollama directory and builds a registry.

use std::path::Path;

use crate::error::InferenceError;
use crate::registry::{ModelInfo, ModelRegistry};

/// Discover all available GGUF models in the given Ollama models directory.
///
/// The `models_dir` path should resolve to the Ollama models root, e.g.
/// `%USERPROFILE%\\.ollama\\models` on Windows.
pub fn discover_models(models_dir: &Path) -> Result<ModelRegistry, InferenceError> {
    if !models_dir.exists() {
        return Err(InferenceError::OllamaNotInstalled(format!(
            "Ollama models directory not found at {}",
            models_dir.display()
        )));
    }

    let infer_registry = ::infer::discover_models(models_dir)
        .map_err(|e| InferenceError::BackendFailed(e.to_string()))?;

    if infer_registry.is_empty() {
        return Err(InferenceError::NoModelsFound);
    }

    let models = infer_registry
        .models()
        .iter()
        .map(|m| ModelInfo {
            path: m.path.clone(),
            name: m.name.clone(),
            size_bytes: m.size_bytes,
        })
        .collect();

    Ok(ModelRegistry { models })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn create_mock_ollama_structure(base_dir: &std::path::Path) {
        let manifests_lib = base_dir
            .join("manifests")
            .join("registry.ollama.ai")
            .join("library");
        fs::create_dir_all(&manifests_lib).expect("failed to create manifests dir");

        let model_dir = manifests_lib.join("test-model");
        fs::create_dir_all(&model_dir).expect("failed to create model dir");

        let manifest_json = r#"{
  "schemaVersion": 2,
  "layers": [
    {
      "mediaType": "application/vnd.ollama.image.model",
      "digest": "sha256:testdigest123",
      "size": 3000000000
    }
  ]
}"#;

        fs::write(model_dir.join("latest"), manifest_json).expect("failed to write manifest");

        let blobs_dir = base_dir.join("blobs");
        fs::create_dir_all(&blobs_dir).expect("failed to create blobs dir");

        let blob_data = vec![0u8; 1024];
        fs::write(blobs_dir.join("sha256-testdigest123"), blob_data)
            .expect("failed to write blob");
    }

    #[test]
    fn discover_models_succeeds_with_valid_ollama_structure() {
        let temp_dir = tempdir().expect("failed to create temp dir");
        create_mock_ollama_structure(temp_dir.path());

        let registry = discover_models(temp_dir.path()).expect("discovery should succeed");

        assert!(!registry.is_empty());
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn discover_models_error_when_dir_missing() {
        let temp_dir = tempdir().expect("failed to create temp dir");
        let missing = temp_dir.path().join("nonexistent");

        let result = discover_models(&missing);
        assert!(result.is_err());
        match result {
            Err(InferenceError::OllamaNotInstalled(_)) => {}
            other => panic!("expected OllamaNotInstalled, got {:?}", other),
        }
    }
}
