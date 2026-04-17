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

    /// Actor operation failed.
    #[error("actor operation failed: {0}")]
    ActorFailed(String),

    /// Bus communication error.
    #[error("bus error: {0}")]
    BusError(String),
}
