//! Daemon IPC layer — CLI ↔ daemon communication.

use crate::error::CliError;
use serde::{Deserialize, Serialize};

/// Events sent from daemon to CLI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DaemonEvent {
    /// Daemon is ready.
    DaemonReady,
    /// Daemon is shutting down.
    DaemonShuttingDown,
    /// Runtime status update.
    StatusUpdate { message: String },
    /// Actor lifecycle event.
    ActorEvent { actor_name: String, status: String },
}

/// Commands sent from CLI to daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CliCommand {
    /// Request daemon status.
    Status,
    /// Request graceful shutdown.
    Shutdown,
    /// Ping the daemon.
    Ping,
    /// Toggle a background loop.
    ToggleLoop { loop_name: String },
    /// List all background loops.
    ListLoops,
}

/// IPC client handle.
///
/// This is the CLI's connection to the daemon.
#[cfg(target_os = "windows")]
pub struct IpcClient {
    _placeholder: (),
}

#[cfg(target_os = "windows")]
impl IpcClient {
    /// Connect to the daemon.
    pub async fn connect() -> Result<Self, CliError> {
        // Windows IPC stub — no actual connection yet
        tracing::info!("IpcClient::connect() stub called (Windows)");
        Ok(Self { _placeholder: () })
    }

    /// Send a command to the daemon.
    pub async fn send_command(&self, _command: CliCommand) -> Result<(), CliError> {
        // Stub — no actual send yet
        tracing::info!("IpcClient::send_command() stub called");
        Ok(())
    }

    /// Receive the next daemon event.
    pub async fn recv_event(&mut self) -> Result<Option<DaemonEvent>, CliError> {
        // Stub — no actual receive yet
        // Return None to indicate no event available
        Ok(None)
    }
}

#[cfg(not(target_os = "windows"))]
compile_error!("CLI IPC is currently only implemented for Windows. Non-Windows IPC support is planned for a future release.");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_event_is_cloneable() {
        let event = DaemonEvent::DaemonReady;
        let cloned = event.clone();
        assert!(matches!(cloned, DaemonEvent::DaemonReady));
    }

    #[test]
    fn cli_command_is_cloneable() {
        let cmd = CliCommand::Status;
        let cloned = cmd.clone();
        assert!(matches!(cloned, CliCommand::Status));
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn ipc_client_connects() {
        let result = IpcClient::connect().await;
        assert!(result.is_ok());
    }
}
