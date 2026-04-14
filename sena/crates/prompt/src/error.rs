//! Prompt composition error types.

use thiserror::Error;

/// Errors that can occur during prompt composition.
#[derive(Debug, Error)]
pub enum PromptError {
    /// A required segment was missing from the context.
    #[error("missing required segment: {0}")]
    MissingSegment(String),

    /// Segment assembly failed.
    #[error("segment assembly failed: {0}")]
    AssemblyFailed(String),

    /// Token limit exceeded during composition.
    #[error("token limit exceeded: {current} > {limit}")]
    TokenLimitExceeded { current: usize, limit: usize },

    /// Invalid prompt context provided.
    #[error("invalid context: {0}")]
    InvalidContext(String),

    /// Memory backend error during retrieval.
    #[error("memory error: {0}")]
    MemoryError(#[from] memory::MemoryError),

    /// Soul backend error during persona retrieval.
    #[error("soul error: {0}")]
    SoulError(#[from] soul::SoulError),
}
