//! Prompt composition errors.

/// Errors that can occur during prompt assembly.
#[derive(Debug, thiserror::Error)]
pub enum PromptError {
    /// Caller passed an empty segment list or all segments were empty.
    #[error("no non-empty segments provided")]
    NoSegments,

    /// A segment failed to produce renderable text.
    #[error("segment assembly failed: {0}")]
    AssemblyFailed(String),
}
