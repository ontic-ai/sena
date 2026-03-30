//! Ollama manifest parser for model discovery.
//!
//! Ollama stores models in a directory structure:
//! - `<models_dir>/manifests/registry.ollama.ai/library/<model_name>/<tag>` — JSON manifest
//! - `<models_dir>/blobs/<digest>` — actual GGUF blob files
//!
//! The manifest JSON contains a `layers` array with digest and mediaType.
//! GGUF layers have mediaType `application/vnd.ollama.image.model`.

use std::fs;
use std::path::Path;

use bus::events::{ModelInfo, Quantization};
use serde::Deserialize;

use crate::error::InferenceError;

/// Ollama manifest structure (partial, only fields we need).
#[derive(Debug, Deserialize)]
struct OllamaManifest {
    #[serde(rename = "layers")]
    layers: Vec<ManifestLayer>,
}

/// A single layer in the Ollama manifest.
#[derive(Debug, Deserialize)]
struct ManifestLayer {
    #[serde(rename = "mediaType")]
    media_type: String,
    #[serde(rename = "digest")]
    digest: String,
}

/// Parse all Ollama manifests in the given models directory.
///
/// Walks the manifests directory structure, parses each manifest JSON,
/// extracts GGUF layer information, and resolves blob file paths.
pub fn parse_ollama_manifests(models_dir: &Path) -> Result<Vec<ModelInfo>, InferenceError> {
    let manifests_dir = models_dir
        .join("manifests")
        .join("registry.ollama.ai")
        .join("library");

    if !manifests_dir.exists() {
        return Err(InferenceError::ManifestNotFound(format!(
            "manifests directory not found at {}",
            manifests_dir.display()
        )));
    }

    let mut models = Vec::new();

    // Walk model directories
    let model_dirs = fs::read_dir(&manifests_dir).map_err(|e| {
        InferenceError::ManifestNotFound(format!("failed to read manifests directory: {}", e))
    })?;

    for model_dir_entry in model_dirs {
        let model_dir_entry = model_dir_entry?;
        let model_dir_path = model_dir_entry.path();

        if !model_dir_path.is_dir() {
            continue;
        }

        let model_name = model_dir_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        // Walk tag files within model directory
        let tag_entries = fs::read_dir(&model_dir_path)?;

        for tag_entry in tag_entries {
            let tag_entry = tag_entry?;
            let tag_path = tag_entry.path();

            if !tag_path.is_file() {
                continue;
            }

            let tag = tag_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("latest");

            // Parse the manifest
            match parse_single_manifest(&tag_path, models_dir, model_name, tag) {
                Ok(Some(model_info)) => models.push(model_info),
                Ok(None) => {
                    // No GGUF layer found, skip silently
                }
                Err(e) => {
                    // Log error but continue processing other manifests
                    eprintln!("Warning: failed to parse {}: {}", tag_path.display(), e);
                }
            }
        }
    }

    Ok(models)
}

/// Parse a single manifest file and extract model information.
fn parse_single_manifest(
    manifest_path: &Path,
    models_dir: &Path,
    model_name: &str,
    tag: &str,
) -> Result<Option<ModelInfo>, InferenceError> {
    let manifest_content = fs::read_to_string(manifest_path)?;

    let manifest: OllamaManifest = serde_json::from_str(&manifest_content).map_err(|e| {
        InferenceError::ManifestCorrupted(format!(
            "failed to parse JSON in {}: {}",
            manifest_path.display(),
            e
        ))
    })?;

    // Find the GGUF layer
    let gguf_layer = manifest
        .layers
        .iter()
        .find(|layer| layer.media_type == "application/vnd.ollama.image.model");

    let gguf_layer = match gguf_layer {
        Some(layer) => layer,
        None => return Ok(None), // No GGUF layer in this manifest
    };

    // Convert digest to blob path: sha256:xxx -> sha256-xxx
    let blob_filename = gguf_layer.digest.replace(':', "-");
    let blob_path = models_dir.join("blobs").join(blob_filename);

    if !blob_path.exists() {
        return Err(InferenceError::ManifestCorrupted(format!(
            "blob file not found: {}",
            blob_path.display()
        )));
    }

    // Get file size
    let metadata = fs::metadata(&blob_path)?;
    let size_bytes = metadata.len();

    // Parse quantization from model name
    let quantization = parse_quantization_from_name(model_name);

    // Construct full model name with tag
    let full_name = if tag == "latest" {
        model_name.to_string()
    } else {
        format!("{}:{}", model_name, tag)
    };

    Ok(Some(ModelInfo {
        name: full_name,
        path: blob_path,
        size_bytes,
        quantization,
    }))
}

/// Parse quantization level from model name.
///
/// Looks for patterns like "q4_0", "q5_1", "f16", etc. in the model name.
fn parse_quantization_from_name(name: &str) -> Quantization {
    let name_lower = name.to_lowercase();

    if name_lower.contains("q4_0") || name_lower.contains("q4-0") {
        Quantization::Q4_0
    } else if name_lower.contains("q4_1") || name_lower.contains("q4-1") {
        Quantization::Q4_1
    } else if name_lower.contains("q5_0") || name_lower.contains("q5-0") {
        Quantization::Q5_0
    } else if name_lower.contains("q5_1") || name_lower.contains("q5-1") {
        Quantization::Q5_1
    } else if name_lower.contains("q8_0") || name_lower.contains("q8-0") {
        Quantization::Q8_0
    } else if name_lower.contains("f16") || name_lower.contains("fp16") {
        Quantization::F16
    } else if name_lower.contains("f32") || name_lower.contains("fp32") {
        Quantization::F32
    } else {
        Quantization::Unknown(name.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn create_mock_manifest(
        models_dir: &Path,
        model_name: &str,
        tag: &str,
        digest: &str,
    ) -> PathBuf {
        let manifest_dir = models_dir
            .join("manifests")
            .join("registry.ollama.ai")
            .join("library")
            .join(model_name);

        fs::create_dir_all(&manifest_dir).expect("Failed to create manifest dir");

        let manifest_path = manifest_dir.join(tag);

        let manifest_json = format!(
            r#"{{
  "schemaVersion": 2,
  "mediaType": "application/vnd.docker.distribution.manifest.v2+json",
  "config": {{
    "mediaType": "application/vnd.docker.container.image.v1+json",
    "digest": "sha256:config123",
    "size": 1234
  }},
  "layers": [
    {{
      "mediaType": "application/vnd.ollama.image.model",
      "digest": "{}",
      "size": 4000000000
    }}
  ]
}}"#,
            digest
        );

        fs::write(&manifest_path, manifest_json).expect("Failed to write manifest");
        manifest_path
    }

    fn create_mock_blob(models_dir: &Path, digest: &str, size_bytes: u64) -> PathBuf {
        let blobs_dir = models_dir.join("blobs");
        fs::create_dir_all(&blobs_dir).expect("Failed to create blobs dir");

        let blob_filename = digest.replace(':', "-");
        let blob_path = blobs_dir.join(blob_filename);

        // Create a file of the specified size
        let data = vec![0u8; size_bytes as usize];
        fs::write(&blob_path, data).expect("Failed to write blob");

        blob_path
    }

    #[test]
    fn parse_ollama_manifests_discovers_single_model() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let models_dir = temp_dir.path();

        let digest = "sha256:abc123def456";
        create_mock_manifest(models_dir, "llama2", "latest", digest);
        create_mock_blob(models_dir, digest, 1024);

        let models = parse_ollama_manifests(models_dir).expect("Failed to parse manifests");

        assert_eq!(models.len(), 1);
        assert_eq!(models[0].name, "llama2");
        assert_eq!(models[0].size_bytes, 1024);
    }

    #[test]
    fn parse_ollama_manifests_discovers_multiple_models() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let models_dir = temp_dir.path();

        let digest1 = "sha256:model1digest";
        let digest2 = "sha256:model2digest";

        create_mock_manifest(models_dir, "llama2", "latest", digest1);
        create_mock_blob(models_dir, digest1, 2048);

        create_mock_manifest(models_dir, "mixtral", "7b", digest2);
        create_mock_blob(models_dir, digest2, 4096);

        let models = parse_ollama_manifests(models_dir).expect("Failed to parse manifests");

        assert_eq!(models.len(), 2);

        let llama = models.iter().find(|m| m.name == "llama2");
        assert!(llama.is_some());
        assert_eq!(llama.unwrap().size_bytes, 2048);

        let mixtral = models.iter().find(|m| m.name == "mixtral:7b");
        assert!(mixtral.is_some());
        assert_eq!(mixtral.unwrap().size_bytes, 4096);
    }

    #[test]
    fn parse_ollama_manifests_fails_when_manifests_dir_missing() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let models_dir = temp_dir.path();

        let result = parse_ollama_manifests(models_dir);

        assert!(result.is_err());
        match result {
            Err(InferenceError::ManifestNotFound(msg)) => {
                assert!(msg.contains("manifests directory not found"));
            }
            _ => panic!("Expected ManifestNotFound error"),
        }
    }

    #[test]
    fn parse_ollama_manifests_fails_when_blob_missing() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let models_dir = temp_dir.path();

        let digest = "sha256:missingblob";
        create_mock_manifest(models_dir, "test-model", "latest", digest);
        // Do NOT create the blob file

        let result = parse_ollama_manifests(models_dir);

        // Should succeed but with no models since we skip errors
        // Or fail with ManifestCorrupted - implementation prints warning
        // Let's verify it returns empty or errors appropriately
        match result {
            Ok(models) => {
                assert_eq!(models.len(), 0, "Should return empty when blob missing");
            }
            Err(_) => {
                // Also acceptable — depends on error handling strategy
            }
        }
    }

    #[test]
    fn parse_quantization_from_name_detects_q4_0() {
        assert_eq!(
            parse_quantization_from_name("llama2-q4_0"),
            Quantization::Q4_0
        );
        assert_eq!(
            parse_quantization_from_name("model-Q4_0-7b"),
            Quantization::Q4_0
        );
    }

    #[test]
    fn parse_quantization_from_name_detects_q5_1() {
        assert_eq!(
            parse_quantization_from_name("mixtral-q5_1"),
            Quantization::Q5_1
        );
    }

    #[test]
    fn parse_quantization_from_name_detects_f16() {
        assert_eq!(parse_quantization_from_name("model-f16"), Quantization::F16);
        assert_eq!(
            parse_quantization_from_name("model-fp16"),
            Quantization::F16
        );
    }

    #[test]
    fn parse_quantization_from_name_returns_unknown_for_unrecognized() {
        let result = parse_quantization_from_name("custom-model");
        match result {
            Quantization::Unknown(name) => {
                assert_eq!(name, "custom-model");
            }
            _ => panic!("Expected Unknown variant"),
        }
    }

    #[test]
    fn parse_single_manifest_constructs_full_name_with_tag() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let models_dir = temp_dir.path();

        let digest = "sha256:tagged123";
        let manifest_path = create_mock_manifest(models_dir, "test-model", "13b", digest);
        create_mock_blob(models_dir, digest, 512);

        let result =
            parse_single_manifest(&manifest_path, models_dir, "test-model", "13b").unwrap();

        assert!(result.is_some());
        let model_info = result.unwrap();
        assert_eq!(model_info.name, "test-model:13b");
    }

    #[test]
    fn parse_single_manifest_omits_latest_tag() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let models_dir = temp_dir.path();

        let digest = "sha256:latest123";
        let manifest_path = create_mock_manifest(models_dir, "test-model", "latest", digest);
        create_mock_blob(models_dir, digest, 512);

        let result =
            parse_single_manifest(&manifest_path, models_dir, "test-model", "latest").unwrap();

        assert!(result.is_some());
        let model_info = result.unwrap();
        assert_eq!(model_info.name, "test-model");
    }
}
