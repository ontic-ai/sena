//! Platform adapter error types.

/// Platform adapter operation errors.
#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    /// Platform feature not available on this OS.
    #[error("platform feature not available: {0}")]
    NotAvailable(String),

    /// Platform I/O error.
    #[error("platform I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Channel send error.
    #[error("channel send error: {0}")]
    ChannelError(String),
}
