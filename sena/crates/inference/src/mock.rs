//! Mock inference backend for testing.
//!
//! `MockBackend` implements `InferenceBackend` and returns configurable canned
//! responses. Used in unit tests and CI where a real GGUF model is unavailable.

use crate::backend::InferenceBackend;
use crate::error::InferenceError;
use crate::stream::InferenceStream;
use crate::types::{BackendType, InferenceParams};
use async_trait::async_trait;
use tokio::sync::mpsc;
use tracing::debug;

/// Configuration for the mock backend.
#[derive(Debug, Clone)]
pub struct MockConfig {
    /// Canned response text emitted as a single token.
    pub response_text: String,
    /// Alias for `response_text` — used by actor test fixtures.
    pub response: String,
    /// Whether `is_loaded()` returns true.
    pub loaded: bool,
    /// If `Some(msg)`, `infer()` returns an `ExecutionFailed` error instead of a stream.
    pub fail_with: Option<String>,
    /// Simulated token count (unused in stub, present for test fixture compatibility).
    pub token_count: usize,
}

impl Default for MockConfig {
    fn default() -> Self {
        Self {
            response_text: "[MOCK] inference response".to_string(),
            response: "[MOCK] inference response".to_string(),
            loaded: true,
            fail_with: None,
            token_count: 0,
        }
    }
}

/// Mock inference backend for testing.
///
/// Returns a single-token stream containing the configured response text.
/// GPU/model state is simulated via the `loaded` flag.
pub struct MockBackend {
    config: MockConfig,
}

impl MockBackend {
    /// Create a new mock backend with the given configuration.
    /// Create a new mock backend with the given config (alias for `new`).
    pub fn with_config(config: MockConfig) -> Self {
        Self::new(config)
    }

    pub fn new(config: MockConfig) -> Self {
        Self { config }
    }

    /// Create a mock backend with default config (loaded = true, fixed response).
    pub fn default_loaded() -> Self {
        Self::new(MockConfig::default())
    }

    /// Create a mock backend that always responds with the given text.
    pub fn with_response(text: impl Into<String>) -> Self {
        let s = text.into();
        Self::new(MockConfig {
            response_text: s.clone(),
            response: s,
            loaded: true,
            fail_with: None,
            token_count: 0,
        })
    }

    /// Create a mock backend that reports as not loaded (no model).
    pub fn unloaded() -> Self {
        Self::new(MockConfig {
            response_text: String::new(),
            response: String::new(),
            loaded: false,
            fail_with: None,
            token_count: 0,
        })
    }

    /// Create a mock backend that always fails inference with the given error message.
    pub fn always_fail(reason: impl Into<String>) -> Self {
        Self::new(MockConfig {
            response_text: String::new(),
            response: String::new(),
            loaded: true,
            fail_with: Some(reason.into()),
            token_count: 0,
        })
    }
}

#[async_trait]
impl InferenceBackend for MockBackend {
    fn backend_type(&self) -> BackendType {
        BackendType::Mock
    }

    fn is_loaded(&self) -> bool {
        self.config.loaded
    }

    async fn infer(
        &self,
        _prompt: String,
        _params: InferenceParams,
    ) -> Result<InferenceStream, InferenceError> {
        if let Some(ref reason) = self.config.fail_with {
            return Err(InferenceError::ExecutionFailed(reason.clone()));
        }

        if !self.config.loaded {
            return Err(InferenceError::ModelNotLoaded);
        }

        debug!("MockBackend: returning canned response as token stream");

        let (tx, stream) = InferenceStream::channel(4);
        let response = self.config.response_text.clone();

        tokio::spawn(async move {
            // Emit the canned response as a single token, then close the channel.
            let _ = tx.send(Ok(response)).await;
        });

        Ok(stream)
    }

    fn complete(&self, _prompt: &str, _params: &InferenceParams) -> Result<String, InferenceError> {
        if let Some(ref reason) = self.config.fail_with {
            return Err(InferenceError::ExecutionFailed(reason.clone()));
        }
        if !self.config.loaded {
            return Err(InferenceError::ModelNotLoaded);
        }
        Ok(self.config.response_text.clone())
    }

    async fn embed(&self, _text: String) -> Result<Vec<f32>, InferenceError> {
        if !self.config.loaded {
            return Err(InferenceError::ModelNotLoaded);
        }
        // Return a fixed-dimension zero vector for testing
        Ok(vec![0.0f32; 384])
    }

    async fn extract(&self, _text: String) -> Result<String, InferenceError> {
        if !self.config.loaded {
            return Err(InferenceError::ModelNotLoaded);
        }
        Ok(r#"["mock fact"]"#.to_string())
    }

    async fn shutdown(&mut self) -> Result<(), InferenceError> {
        debug!("MockBackend: shutdown");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_backend_is_loaded() {
        let backend = MockBackend::default_loaded();
        assert!(backend.is_loaded());
        assert_eq!(backend.backend_type(), BackendType::Mock);
    }

    #[tokio::test]
    async fn mock_backend_unloaded_returns_error() {
        let backend = MockBackend::unloaded();
        assert!(!backend.is_loaded());
        let params = InferenceParams::default();
        let result = backend.infer("prompt".to_string(), params).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn mock_backend_returns_canned_response() {
        let backend = MockBackend::with_response("hello world");
        let params = InferenceParams::default();
        let stream = backend
            .infer("prompt".to_string(), params)
            .await
            .expect("infer should succeed");
        let text = stream.collect_all().await.expect("collect should succeed");
        assert_eq!(text, "hello world");
    }

    #[tokio::test]
    async fn mock_backend_complete_returns_text() {
        let backend = MockBackend::with_response("complete response");
        let params = InferenceParams::default();
        let result = backend.complete("prompt", &params).expect("complete should succeed");
        assert_eq!(result, "complete response");
    }

    #[tokio::test]
    async fn mock_backend_always_fail() {
        let backend = MockBackend::always_fail("injected error");
        let params = InferenceParams::default();
        let result = backend.infer("prompt".to_string(), params).await;
        assert!(result.is_err());
    }
}
