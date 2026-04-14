//! Memory subsystem errors.

/// Memory subsystem error types.
#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("ingestion failed: {0}")]
    IngestionFailed(String),

    #[error("query failed: {0}")]
    QueryFailed(String),

    #[error("invalid embedding: {0}")]
    InvalidEmbedding(String),

    #[error("backend error: {0}")]
    BackendError(String),
}
