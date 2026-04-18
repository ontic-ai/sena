//! CLI error types.

use thiserror::Error;

/// Errors that can occur in the CLI.
#[derive(Debug, Error)]
pub enum CliError {
    /// IPC connection failed.
    #[error("IPC connection failed: {0}")]
    IpcConnectionFailed(String),

    /// IPC send failed.
    #[error("IPC send failed: {0}")]
    IpcSendFailed(String),

    /// Shell run error.
    #[error("shell run error: {0}")]
    ShellRunError(String),

    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
