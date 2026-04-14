//! Inference error types.

/// Errors that can occur during inference operations.
#[derive(Debug, thiserror::Error)]
pub enum InferenceError {
    /// Backend initialization failed.
    #[error("backend initialization failed: {0}")]
    BackendInit(String),

    /// Backend inference execution failed.
    #[error("inference execution failed: {0}")]
    ExecutionFailed(String),

    /// Invalid inference parameters.
    #[error("invalid parameters: {0}")]
    InvalidParams(String),

    /// Model not loaded.
    #[error("model not loaded")]
    ModelNotLoaded,

    /// Stream error.
    #[error("stream error: {0}")]
    StreamError(String),

    /// Actor error.
    #[error("actor error: {0}")]
    ActorError(String),

    /// Bus communication error.
    #[error("bus error: {0}")]
    BusError(String),
}

impl From<bus::BusError> for InferenceError {
    fn from(e: bus::BusError) -> Self {
        Self::BusError(e.to_string())
    }
}
