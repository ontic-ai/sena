//! Platform adapter error types.

/// Errors that can occur when interacting with platform backends.
#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    /// Platform backend not available for this OS.
    #[error("platform backend not available: {0}")]
    BackendUnavailable(String),

    /// Failed to retrieve active window context.
    #[error("failed to retrieve window context: {0}")]
    WindowContextFailed(String),

    /// Failed to retrieve clipboard content.
    #[error("failed to retrieve clipboard digest: {0}")]
    ClipboardFailed(String),

    /// Failed to retrieve keystroke cadence.
    #[error("failed to retrieve keystroke cadence: {0}")]
    KeystrokeCadenceFailed(String),

    /// Failed to capture screen frame.
    #[error("failed to capture screen frame: {0}")]
    ScreenCaptureFailed(String),

    /// Platform-specific OS error.
    #[error("platform OS error: {0}")]
    OsError(String),

    /// Actor has been shut down.
    #[error("platform actor shutdown")]
    ActorShutdown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_formats_correctly() {
        let err = PlatformError::BackendUnavailable("Linux backend not implemented".to_string());
        assert!(err.to_string().contains("not available"));

        let err = PlatformError::ActorShutdown;
        assert_eq!(err.to_string(), "platform actor shutdown");
    }

    #[test]
    fn error_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<PlatformError>();
    }
}
