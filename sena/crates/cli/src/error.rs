//! CLI error types.

use thiserror::Error;

/// Errors that can occur in the CLI.
#[derive(Debug, Error)]
#[allow(dead_code)]
pub enum CliError {
    /// IPC connection failed.
    #[error("IPC connection failed: {0}")]
    IpcConnectionFailed(String),

    /// Daemon start failed.
    #[error("daemon start failed: {0}")]
    DaemonStartFailed(String),

    /// Daemon start timeout.
    #[error("daemon failed to become ready within 10 seconds")]
    DaemonStartTimeout,

    /// Platform not supported.
    #[error("platform not supported")]
    PlatformNotSupported,

    /// Shell run error.
    #[error("shell run error: {0}")]
    ShellRunError(String),

    /// TUI render error.
    #[error("TUI render error: {0}")]
    TuiRenderError(String),

    /// IPC send error.
    #[error("IPC send error: {0}")]
    IpcSendError(String),

    /// IPC receive error.
    #[error("IPC receive error: {0}")]
    IpcReceiveError(String),

    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// IPC error.
    #[error("IPC error: {0}")]
    Ipc(#[from] ipc::IpcError),

    /// Onboarding failed.
    #[error("onboarding failed: {0}")]
    OnboardingFailed(String),
}
