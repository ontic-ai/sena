use thiserror::Error;

#[derive(Debug, Error)]
pub enum InferenceError {
    #[error("Ollama not installed: expected directory not found at {0}")]
    OllamaNotInstalled(String),

    #[error("no models found: Ollama is installed but no models have been pulled")]
    NoModelsFound,

    #[error("manifest not found: expected file missing at {0}")]
    ManifestNotFound(String),

    #[error("manifest corrupted: file exists but cannot be parsed: {0}")]
    ManifestCorrupted(String),

    #[error("model load failed: {0}")]
    ModelLoadFailed(String),

    #[error("io error: {0}")]
    IoError(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_ollama_not_installed() {
        let err = InferenceError::OllamaNotInstalled("/path/to/ollama".to_string());
        assert_eq!(
            err.to_string(),
            "Ollama not installed: expected directory not found at /path/to/ollama"
        );
    }

    #[test]
    fn error_display_no_models_found() {
        let err = InferenceError::NoModelsFound;
        assert_eq!(
            err.to_string(),
            "no models found: Ollama is installed but no models have been pulled"
        );
    }

    #[test]
    fn error_display_manifest_not_found() {
        let err = InferenceError::ManifestNotFound("/path/to/manifest.json".to_string());
        assert_eq!(
            err.to_string(),
            "manifest not found: expected file missing at /path/to/manifest.json"
        );
    }

    #[test]
    fn error_display_manifest_corrupted() {
        let err = InferenceError::ManifestCorrupted("invalid JSON syntax".to_string());
        assert_eq!(
            err.to_string(),
            "manifest corrupted: file exists but cannot be parsed: invalid JSON syntax"
        );
    }

    #[test]
    fn error_display_model_load_failed() {
        let err = InferenceError::ModelLoadFailed("GGUF file corrupted".to_string());
        assert_eq!(err.to_string(), "model load failed: GGUF file corrupted");
    }

    #[test]
    fn error_display_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err = InferenceError::IoError(io_err);
        assert_eq!(err.to_string(), "io error: file not found");
    }

    #[test]
    fn error_from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "access denied");
        let err: InferenceError = io_err.into();
        assert!(err.to_string().contains("access denied"));
    }
}
