//! Daemon error types.

use std::fmt;

/// Daemon-specific errors.
#[derive(Debug)]
pub enum DaemonError {
    /// Runtime boot failed.
    BootFailed(String),
    /// Logging initialization failed.
    LoggingFailed(String),
    /// IPC server error.
    #[allow(dead_code)]
    IpcServerError(String),
    /// Supervision loop error.
    #[allow(dead_code)]
    SupervisionError(String),
    /// Tray initialization or runtime error.
    #[allow(dead_code)]
    TrayError(String),
    /// CLI launch failed.
    CliLaunchFailed(String),
    /// Models folder access failed.
    ModelsFolderError(String),
}

impl fmt::Display for DaemonError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::BootFailed(msg) => write!(f, "runtime boot failed: {}", msg),
            Self::LoggingFailed(msg) => write!(f, "logging initialization failed: {}", msg),
            Self::IpcServerError(msg) => write!(f, "IPC server error: {}", msg),
            Self::SupervisionError(msg) => write!(f, "supervision loop error: {}", msg),
            Self::TrayError(msg) => write!(f, "tray error: {}", msg),
            Self::CliLaunchFailed(msg) => write!(f, "CLI launch failed: {}", msg),
            Self::ModelsFolderError(msg) => write!(f, "models folder error: {}", msg),
        }
    }
}

impl std::error::Error for DaemonError {}
