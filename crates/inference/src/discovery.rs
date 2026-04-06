//! Model discovery orchestration.
//!
//! Discovers GGUF models in the Ollama directory and builds a registry.

use std::path::Path;

use crate::error::InferenceError;
use crate::registry::ModelRegistry;

/// Discover all available GGUF models in the given Ollama models directory.
///
/// The `models_dir` path should be resolved by the caller via
/// `platform::dirs::ollama_models_dir()`. This keeps OS-specific path
/// resolution in the platform crate per architecture.md §5.
///
/// # Errors
///
/// Returns an error if:
/// - The models directory does not exist
/// - No models are found (Ollama installed but no models pulled)
/// - Model discovery fails critically
pub fn discover_models(models_dir: &Path) -> Result<ModelRegistry, InferenceError> {
    if !models_dir.exists() {
        return Err(InferenceError::OllamaNotInstalled(format!(
            "Ollama models directory not found at {}",
            models_dir.display()
        )));
    }

    // Use infer's discover_models to get a ModelRegistry
    // The infer crate returns Result<ModelRegistry, InferError>
    let infer_registry = match ::infer::discover_models(models_dir) {
        Ok(registry) => registry,
        Err(e) => {
            // Check if the error message indicates no models found
            let err_msg = e.to_string();
            if err_msg.contains("no-models-found") || err_msg.contains("no models") {
                return Err(InferenceError::NoModelsFound);
            }
            return Err(InferenceError::BackendFailed(err_msg));
        }
    };

    if infer_registry.is_empty() {
        return Err(InferenceError::NoModelsFound);
    }

    // Convert infer's ModelRegistry to our local ModelRegistry wrapper
    // Extract the models and rebuild with our wrapper
    let models: Vec<_> = infer_registry.models().to_vec();
    Ok(ModelRegistry::from_models(models))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn create_mock_ollama_structure(base_dir: &std::path::Path) {
        let models_dir = base_dir;

        // Create manifests structure
        let manifests_lib = models_dir
            .join("manifests")
            .join("registry.ollama.ai")
            .join("library");
        fs::create_dir_all(&manifests_lib).expect("Failed to create manifests dir");

        // Create a model manifest
        let model_dir = manifests_lib.join("test-model");
        fs::create_dir_all(&model_dir).expect("Failed to create model dir");

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

        fs::write(model_dir.join("latest"), manifest_json).expect("Failed to write manifest");

        // Create corresponding blob
        let blobs_dir = models_dir.join("blobs");
        fs::create_dir_all(&blobs_dir).expect("Failed to create blobs dir");

        let blob_data = vec![0u8; 1024];
        fs::write(blobs_dir.join("sha256-testdigest123"), blob_data).expect("Failed to write blob");
    }

    #[test]
    fn discover_models_succeeds_with_valid_ollama_structure() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        create_mock_ollama_structure(temp_dir.path());

        let registry = discover_models(temp_dir.path()).expect("discovery should succeed");

        assert!(!registry.is_empty());
        assert_eq!(registry.model_count(), 1);
    }

    #[test]
    fn discover_models_error_when_dir_missing() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let missing = temp_dir.path().join("nonexistent");

        let result = discover_models(&missing);
        assert!(result.is_err());
        match result {
            Err(InferenceError::OllamaNotInstalled(_)) => { /* expected */ }
            other => panic!("Expected OllamaNotInstalled, got {:?}", other),
        }
    }

    #[test]
    fn discover_models_error_no_models_pulled() {
        let temp_dir = tempdir().expect("Failed to create temp dir");

        // Create empty manifests directory
        let manifests_lib = temp_dir
            .path()
            .join("manifests")
            .join("registry.ollama.ai")
            .join("library");
        fs::create_dir_all(&manifests_lib).expect("Failed to create manifests dir");

        let result = discover_models(temp_dir.path());
        assert!(result.is_err());
        match result {
            Err(InferenceError::NoModelsFound) => { /* expected */ }
            other => panic!("Expected NoModelsFound, got {:?}", other),
        }
    }

    #[test]
    fn discover_models_error_no_manifests_dir() {
        let temp_dir = tempdir().expect("Failed to create temp dir");

        // Directory exists but manifests/ is missing
        let result = discover_models(temp_dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn discover_models_registry_selects_default() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        create_mock_ollama_structure(temp_dir.path());

        let registry = discover_models(temp_dir.path()).expect("discovery should succeed");

        assert!(registry.default_model().is_some());
        assert_eq!(registry.default_model(), Some("test-model"));
    }
}
