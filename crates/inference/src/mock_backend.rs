//! Mock LLM backend for testing.
//!
//! Implements LlmBackend with configurable canned responses.
//! Used exclusively in tests — never in production code paths.

use std::path::Path;

use crate::backend::{BackendError, BackendType, InferenceParams, LlmBackend};

/// Configuration for mock backend responses.
#[derive(Debug, Clone)]
pub struct MockConfig {
    /// Response text returned by infer().
    pub infer_response: String,
    /// Token count reported for infer() responses.
    pub token_count: usize,
    /// Embedding vector returned by embed().
    pub embed_vector: Vec<f32>,
    /// Facts returned by extract().
    pub extract_facts: Vec<String>,
    /// If true, load_model will fail.
    pub fail_load: bool,
    /// If true, infer/embed/extract will fail.
    pub fail_inference: bool,
}

impl Default for MockConfig {
    fn default() -> Self {
        Self {
            infer_response: "Mock inference response".to_string(),
            token_count: 10,
            embed_vector: vec![0.1; 384],
            extract_facts: vec!["fact1".to_string(), "fact2".to_string()],
            fail_load: false,
            fail_inference: false,
        }
    }
}

/// Mock LLM backend for testing.
pub struct MockBackend {
    config: MockConfig,
    loaded: bool,
    infer_call_count: usize,
    embed_call_count: usize,
    extract_call_count: usize,
}

impl MockBackend {
    /// Create a new mock backend with default config.
    pub fn new() -> Self {
        Self::with_config(MockConfig::default())
    }

    /// Create a new mock backend with custom config.
    pub fn with_config(config: MockConfig) -> Self {
        Self {
            config,
            loaded: false,
            infer_call_count: 0,
            embed_call_count: 0,
            extract_call_count: 0,
        }
    }

    /// Number of times infer() was called.
    pub fn infer_call_count(&self) -> usize {
        self.infer_call_count
    }

    /// Number of times embed() was called.
    pub fn embed_call_count(&self) -> usize {
        self.embed_call_count
    }

    /// Number of times extract() was called.
    pub fn extract_call_count(&self) -> usize {
        self.extract_call_count
    }
}

impl Default for MockBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl LlmBackend for MockBackend {
    fn load_model(
        &mut self,
        _model_path: &Path,
        _backend_type: BackendType,
    ) -> Result<(), BackendError> {
        if self.config.fail_load {
            return Err(BackendError::ModelLoadFailed(
                "mock load failure".to_string(),
            ));
        }
        self.loaded = true;
        Ok(())
    }

    fn infer(&self, _prompt: &str, _params: &InferenceParams) -> Result<String, BackendError> {
        if !self.loaded {
            return Err(BackendError::NotInitialized);
        }
        if self.config.fail_inference {
            return Err(BackendError::InferenceFailed(
                "mock inference failure".to_string(),
            ));
        }
        Ok(self.config.infer_response.clone())
    }

    fn embed(&self, _text: &str) -> Result<Vec<f32>, BackendError> {
        if !self.loaded {
            return Err(BackendError::NotInitialized);
        }
        if self.config.fail_inference {
            return Err(BackendError::EmbeddingFailed(
                "mock embedding failure".to_string(),
            ));
        }
        Ok(self.config.embed_vector.clone())
    }

    fn extract(&self, _text: &str) -> Result<Vec<String>, BackendError> {
        if !self.loaded {
            return Err(BackendError::NotInitialized);
        }
        if self.config.fail_inference {
            return Err(BackendError::ExtractionFailed(
                "mock extraction failure".to_string(),
            ));
        }
        Ok(self.config.extract_facts.clone())
    }

    fn is_loaded(&self) -> bool {
        self.loaded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn mock_backend_default_config() {
        let mock = MockBackend::new();
        assert!(!mock.is_loaded());
        assert_eq!(mock.infer_call_count(), 0);
    }

    #[test]
    fn mock_backend_load_and_infer() {
        let mut mock = MockBackend::new();
        mock.load_model(&PathBuf::from("/test.gguf"), BackendType::Cpu)
            .expect("load should succeed");
        assert!(mock.is_loaded());

        let result = mock
            .infer("test prompt", &InferenceParams::default())
            .expect("infer should succeed");
        assert_eq!(result, "Mock inference response");
    }

    #[test]
    fn mock_backend_load_and_embed() {
        let mut mock = MockBackend::new();
        mock.load_model(&PathBuf::from("/test.gguf"), BackendType::Cpu)
            .expect("load should succeed");

        let vector = mock.embed("test text").expect("embed should succeed");
        assert_eq!(vector.len(), 384);
    }

    #[test]
    fn mock_backend_load_and_extract() {
        let mut mock = MockBackend::new();
        mock.load_model(&PathBuf::from("/test.gguf"), BackendType::Cpu)
            .expect("load should succeed");

        let facts = mock.extract("test text").expect("extract should succeed");
        assert_eq!(facts, vec!["fact1", "fact2"]);
    }

    #[test]
    fn mock_backend_fails_before_load() {
        let mock = MockBackend::new();
        assert!(mock.infer("test", &InferenceParams::default()).is_err());
        assert!(mock.embed("test").is_err());
        assert!(mock.extract("test").is_err());
    }

    #[test]
    fn mock_backend_configurable_failure() {
        let config = MockConfig {
            fail_load: true,
            ..Default::default()
        };
        let mut mock = MockBackend::with_config(config);
        assert!(mock
            .load_model(&PathBuf::from("/test.gguf"), BackendType::Cpu)
            .is_err());
    }

    #[test]
    fn mock_backend_inference_failure() {
        let config = MockConfig {
            fail_inference: true,
            ..Default::default()
        };
        let mut mock = MockBackend::with_config(config);
        mock.load_model(&PathBuf::from("/test.gguf"), BackendType::Cpu)
            .expect("load should succeed");

        assert!(mock.infer("test", &InferenceParams::default()).is_err());
        assert!(mock.embed("test").is_err());
        assert!(mock.extract("test").is_err());
    }
}
