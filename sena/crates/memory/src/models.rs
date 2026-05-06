//! Memory model metadata and path resolution.
//!
//! This module defines memory model metadata (URLs, checksums, filenames) and
//! provides path resolution and existence checking -- NO download functionality.
//! Downloads are handled by the runtime's DownloadManager.

use std::path::{Path, PathBuf};

/// Memory model type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelType {
    /// Nomic embedding model for memory vector store.
    NomicEmbed,
}

/// Memory model information.
#[derive(Debug, Clone)]
pub struct ModelInfo {
    /// Model name (human-readable).
    pub name: String,
    /// Filename on disk.
    pub filename: String,
    /// HuggingFace URL.
    pub url: String,
    /// Expected SHA-256 checksum.
    pub sha256: String,
    /// File size in bytes.
    pub size_bytes: u64,
    /// Model type.
    pub model_type: ModelType,
}

/// Known memory models with their HuggingFace metadata.
pub struct ModelManifest;

impl ModelManifest {
    /// Returns the Nomic embed-text v1.5 GGUF model for vector embeddings.
    ///
    /// Required for memory subsystem operation. Boot will fail if this model
    /// is missing and cannot be downloaded.
    pub fn nomic_embed_text() -> ModelInfo {
        ModelInfo {
            name: "nomic-embed-text-v1.5-Q8_0".to_string(),
            filename: "nomic-embed-text-v1.5.Q8_0.gguf".to_string(),
            url: "https://huggingface.co/nomic-ai/nomic-embed-text-v1.5-GGUF/resolve/main/nomic-embed-text-v1.5.Q8_0.gguf".to_string(),
            sha256: "3e24342164b3d94991ba9692fdc0dd08e3fd7362e0aacc396a9a5c54a544c3b7".to_string(),
            size_bytes: 146_000_000, // ~146MB
            model_type: ModelType::NomicEmbed,
        }
    }

    /// Returns all memory models.
    pub fn all_models() -> Vec<ModelInfo> {
        vec![Self::nomic_embed_text()]
    }

    /// Returns the required embedding model.
    pub fn required_embed_model() -> ModelInfo {
        Self::nomic_embed_text()
    }
}

/// Model cache operations (path resolution and existence checking only).
pub struct ModelCache;

impl ModelCache {
    /// Checks if a model file exists on disk.
    ///
    /// NOTE: This does NOT verify checksums. Checksum verification is the runtime
    /// DownloadManager's responsibility.
    pub async fn is_cached(model_dir: &Path, model: &ModelInfo) -> bool {
        let path = Self::cached_path(model_dir, model);
        path.exists()
    }

    /// Returns the expected path for a cached model.
    pub fn cached_path(model_dir: &Path, model: &ModelInfo) -> PathBuf {
        model_dir.join(&model.filename)
    }

    /// Lists all cached models in the directory.
    pub async fn list_cached(model_dir: &Path) -> Vec<ModelInfo> {
        let mut cached = Vec::new();

        for model in ModelManifest::all_models() {
            if Self::is_cached(model_dir, &model).await {
                cached.push(model);
            }
        }

        cached
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn model_manifest_contains_required_embed_model() {
        let models = ModelManifest::all_models();
        assert_eq!(models.len(), 1);

        let embed_model = &models[0];
        assert_eq!(embed_model.model_type, ModelType::NomicEmbed);
        assert_eq!(embed_model.filename, "nomic-embed-text-v1.5.Q8_0.gguf");
        assert!(embed_model.size_bytes > 0);
    }

    #[test]
    fn cached_path_returns_correct_path() {
        let model = ModelManifest::required_embed_model();
        let model_dir = Path::new("/tmp/models");
        let path = ModelCache::cached_path(model_dir, &model);

        assert_eq!(path, model_dir.join(&model.filename));
    }

    #[tokio::test]
    async fn is_cached_returns_false_for_nonexistent_file() {
        let temp_dir = tempdir().expect("tempdir creation");
        let model = ModelManifest::required_embed_model();

        let cached = ModelCache::is_cached(temp_dir.path(), &model).await;
        assert!(!cached);
    }

    #[tokio::test]
    async fn is_cached_returns_true_for_existing_file() {
        let temp_dir = tempdir().expect("tempdir creation");
        let model = ModelManifest::required_embed_model();

        let model_path = ModelCache::cached_path(temp_dir.path(), &model);
        std::fs::write(&model_path, b"dummy model data").expect("write model file");

        let cached = ModelCache::is_cached(temp_dir.path(), &model).await;
        assert!(cached);
    }

    #[tokio::test]
    async fn list_cached_returns_empty_for_new_directory() {
        let temp_dir = tempdir().expect("tempdir creation");
        let cached = ModelCache::list_cached(temp_dir.path()).await;
        assert_eq!(cached.len(), 0);
    }

    #[tokio::test]
    async fn list_cached_returns_models_that_exist() {
        let temp_dir = tempdir().expect("tempdir creation");
        let model = ModelManifest::required_embed_model();
        let model_path = ModelCache::cached_path(temp_dir.path(), &model);
        std::fs::write(&model_path, b"dummy embed model data").expect("write embed model file");

        let cached = ModelCache::list_cached(temp_dir.path()).await;
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].filename, model.filename);
    }
}