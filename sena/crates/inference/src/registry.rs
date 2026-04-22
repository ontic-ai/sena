//! Model registry — discover and catalogue GGUF model files on disk.
//!
//! `discover_models` scans a directory for `.gguf` files and returns a
//! `ModelRegistry` with metadata for each discovered model.

use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// Metadata for a single model file.
#[derive(Debug, Clone)]
pub struct ModelInfo {
    /// Absolute path to the `.gguf` file.
    pub path: PathBuf,
    /// Filename stem (model name without extension).
    pub name: String,
    /// File size in bytes.
    pub size_bytes: u64,
}

/// Ordered collection of discovered model files.
#[derive(Debug, Default)]
pub struct ModelRegistry {
    /// All discovered models, in directory-scan order.
    pub models: Vec<ModelInfo>,
}

impl ModelRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self { models: Vec::new() }
    }

    /// Number of models in the registry.
    pub fn len(&self) -> usize {
        self.models.len()
    }

    /// Return `true` if no models are registered.
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    /// Find a model by its name (case-insensitive stem match).
    pub fn find_by_name(&self, name: &str) -> Option<&ModelInfo> {
        let target = name.to_lowercase();
        self.models.iter().find(|m| m.name.to_lowercase() == target)
    }
}

/// Scan `dir` for `.gguf` files and return a populated `ModelRegistry`.
///
/// Non-existent or unreadable directories return an empty registry (no panic).
/// Individual file metadata errors are logged and skipped.
pub fn discover_models(dir: &Path) -> ModelRegistry {
    let mut registry = ModelRegistry::new();

    if !dir.exists() {
        debug!(path = ?dir, "model directory does not exist — returning empty registry");
        return registry;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(err) => {
            warn!(path = ?dir, error = %err, "failed to read model directory");
            return registry;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.extension().and_then(|e| e.to_str()) != Some("gguf") {
            continue;
        }

        let name = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => {
                warn!(path = ?path, "model file has non-UTF-8 stem — skipping");
                continue;
            }
        };

        let size_bytes = match entry.metadata() {
            Ok(m) => m.len(),
            Err(err) => {
                warn!(path = ?path, error = %err, "failed to read model file metadata — using 0");
                0
            }
        };

        debug!(name = %name, size_bytes = size_bytes, "discovered model");
        registry.models.push(ModelInfo {
            path,
            name,
            size_bytes,
        });
    }

    debug!(count = registry.len(), "model discovery complete");
    registry
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn discover_models_empty_dir() {
        let dir = tempdir().expect("tempdir should create");
        let registry = discover_models(dir.path());
        assert!(registry.is_empty());
    }

    #[test]
    fn discover_models_finds_gguf_files() {
        let dir = tempdir().expect("tempdir should create");
        fs::write(dir.path().join("my-model.gguf"), b"fake").expect("write should succeed");
        fs::write(dir.path().join("other.txt"), b"not a model").expect("write should succeed");
        let registry = discover_models(dir.path());
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.models[0].name, "my-model");
    }

    #[test]
    fn discover_models_finds_by_name() {
        let dir = tempdir().expect("tempdir should create");
        fs::write(dir.path().join("llama-3.gguf"), b"fake").expect("write should succeed");
        let registry = discover_models(dir.path());
        assert!(registry.find_by_name("llama-3").is_some());
        assert!(registry.find_by_name("nonexistent").is_none());
    }

    #[test]
    fn discover_models_nonexistent_dir() {
        let registry = discover_models(Path::new("/nonexistent/path/to/models"));
        assert!(registry.is_empty());
    }
}
