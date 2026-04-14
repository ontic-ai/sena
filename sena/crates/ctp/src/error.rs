//! CTP-specific error types.

/// Errors from CTP subsystem operations.
#[derive(Debug, thiserror::Error)]
pub enum CtpError {
    /// Signal channel closed unexpectedly.
    #[error("signal channel closed")]
    ChannelClosed,

    /// Failed to assemble context snapshot.
    #[error("snapshot assembly failed: {0}")]
    SnapshotAssemblyFailed(String),

    /// Bus communication failure.
    #[error("bus error: {0}")]
    BusError(String),

    /// Platform backend error.
    #[error("platform error: {0}")]
    PlatformError(String),

    /// Soul store error.
    #[error("soul error: {0}")]
    SoulError(String),
}

impl From<bus::BusError> for CtpError {
    fn from(err: bus::BusError) -> Self {
        CtpError::BusError(err.to_string())
    }
}

impl From<platform::PlatformError> for CtpError {
    fn from(err: platform::PlatformError) -> Self {
        CtpError::PlatformError(err.to_string())
    }
}

impl From<soul::SoulError> for CtpError {
    fn from(err: soul::SoulError) -> Self {
        CtpError::SoulError(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_displays_correctly() {
        let err = CtpError::ChannelClosed;
        assert_eq!(err.to_string(), "signal channel closed");
    }

    #[test]
    fn snapshot_assembly_error_includes_context() {
        let err = CtpError::SnapshotAssemblyFailed("missing platform data".to_string());
        assert!(err.to_string().contains("missing platform data"));
    }
}
