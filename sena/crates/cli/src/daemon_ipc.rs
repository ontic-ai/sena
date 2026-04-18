//! Daemon IPC layer — CLI ↔ daemon communication.

use crate::error::CliError;
use runtime::{IpcClientHandle, IpcCommand as RuntimeCommand, IpcEvent, IpcResponse};

/// Internal daemon event wrapper for CLI — private formatting helper.
#[derive(Debug, Clone)]
pub(crate) enum DaemonEvent {
    DaemonReady,
    DaemonShuttingDown,
    StatusUpdate {
        actors: Vec<bus::events::system::ActorHealth>,
        uptime_seconds: u64,
    },
    Pong,
    Acknowledged,
    LoopStatusChanged {
        loop_name: String,
        enabled: bool,
    },
    LoopsListed {
        loops: Vec<runtime::LoopInfo>,
    },
    DebugInfo {
        info: String,
    },
    VerboseSet {
        enabled: bool,
    },
    MemoryStats {
        stats: String,
    },
    ConfigDump {
        config: String,
    },
}

/// Internal CLI command wrapper — private, delegates to runtime::IpcCommand.
#[derive(Debug, Clone)]
pub(crate) enum CliCommand {
    Status,
    Shutdown,
    Ping,
    ToggleLoop { loop_name: String, enabled: bool },
    ListLoops,
    DebugInfo,
    SetVerbose { enabled: bool },
    MemoryStats,
    ConfigDump,
}

/// IPC client wrapping the runtime's in-process IPC handle.
pub struct IpcClient {
    handle: IpcClientHandle,
}

impl IpcClient {
    /// Create a new IPC client from the runtime's client handle.
    pub fn new(handle: IpcClientHandle) -> Self {
        Self { handle }
    }

    /// Send a command to the daemon.
    pub(crate) async fn send_command(&self, command: CliCommand) -> Result<(), CliError> {
        let runtime_cmd = match command {
            CliCommand::Status => RuntimeCommand::StatusRequest,
            CliCommand::Shutdown => RuntimeCommand::ShutdownRequest,
            CliCommand::Ping => RuntimeCommand::Ping,
            CliCommand::ToggleLoop { loop_name, enabled } => {
                RuntimeCommand::ToggleLoop { loop_name, enabled }
            }
            CliCommand::ListLoops => RuntimeCommand::ListLoops,
            CliCommand::DebugInfo => RuntimeCommand::DebugInfo,
            CliCommand::SetVerbose { enabled } => RuntimeCommand::SetVerbose { enabled },
            CliCommand::MemoryStats => RuntimeCommand::MemoryStats,
            CliCommand::ConfigDump => RuntimeCommand::ConfigDump,
        };

        self.handle
            .command_tx
            .send(runtime_cmd)
            .map_err(|e| CliError::IpcSendFailed(e.to_string()))?;

        Ok(())
    }

    /// Receive the next daemon event.
    pub(crate) async fn recv_event(&mut self) -> Result<Option<DaemonEvent>, CliError> {
        match self.handle.response_rx.try_recv() {
            Ok(response) => Ok(Some(map_response_to_event(response))),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => Ok(None),
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                Err(CliError::IpcConnectionFailed("channel closed".to_string()))
            }
        }
    }
}

/// Map runtime IpcResponse to CLI DaemonEvent.
fn map_response_to_event(response: IpcResponse) -> DaemonEvent {
    match response {
        IpcResponse::Status {
            actors,
            uptime_seconds,
        } => DaemonEvent::StatusUpdate {
            actors,
            uptime_seconds,
        },
        IpcResponse::ShutdownAcknowledged => DaemonEvent::DaemonShuttingDown,
        IpcResponse::Pong => DaemonEvent::Pong,
        IpcResponse::Ok => DaemonEvent::Acknowledged,
        IpcResponse::LoopsList { loops } => DaemonEvent::LoopsListed { loops },
        IpcResponse::DebugInfo { info } => DaemonEvent::DebugInfo { info },
        IpcResponse::VerboseSet { enabled } => DaemonEvent::VerboseSet { enabled },
        IpcResponse::MemoryStats { stats } => DaemonEvent::MemoryStats { stats },
        IpcResponse::ConfigDump { config } => DaemonEvent::ConfigDump { config },
        IpcResponse::Event(event) => match event {
            IpcEvent::BootComplete => DaemonEvent::DaemonReady,
            IpcEvent::ShutdownInitiated => DaemonEvent::DaemonShuttingDown,
            IpcEvent::LoopStatusChanged { loop_name, enabled } => {
                DaemonEvent::LoopStatusChanged { loop_name, enabled }
            }
        },
    }
}

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
}
