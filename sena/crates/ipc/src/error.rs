use std::io;

/// Errors that can occur during IPC operations.
#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    /// I/O error during frame read/write.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    /// JSON serialization/deserialization error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Frame size exceeds maximum allowed (16MB).
    #[error("frame too large: {0} bytes (max 16MB)")]
    FrameTooLarge(usize),

    /// Connection closed unexpectedly.
    #[error("connection closed")]
    ConnectionClosed,

    /// Command not found in registry.
    #[error("unknown command: {0}")]
    UnknownCommand(String),

    /// Command handler returned an error.
    #[error("command failed: {0}")]
    CommandFailed(String),

    /// Daemon is not running.
    #[error("daemon not running")]
    DaemonNotRunning,

    /// Platform not supported for IPC.
    #[error("IPC not supported on this platform (Windows only in Phase 1)")]
    PlatformNotSupported,

    /// Invalid command payload.
    #[error("invalid payload: {0}")]
    InvalidPayload(String),

    /// Command not ready (daemon not fully booted or feature not implemented).
    #[error("command not ready: {0}")]
    CommandNotReady(String),

    /// Internal daemon error.
    #[error("internal error: {0}")]
    Internal(String),
}
