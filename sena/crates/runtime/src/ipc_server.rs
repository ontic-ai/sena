//! IPC server stub — accepts commands from CLI and logs them.

use crate::error::RuntimeError;
use tokio::sync::mpsc;
use tracing::{info, warn};

/// IPC command types that the CLI can send to the daemon.
#[derive(Debug, Clone)]
pub enum IpcCommand {
    /// Request current runtime status.
    StatusRequest,
    /// Request graceful shutdown.
    ShutdownRequest,
    /// Request inference run.
    InferenceRequest { prompt: String },
    /// Ping command.
    Ping,
}

/// IPC server handle.
pub struct IpcServer {
    command_rx: mpsc::UnboundedReceiver<IpcCommand>,
}

impl IpcServer {
    /// Create a new IPC server.
    ///
    /// Returns (server, command_tx) where command_tx can be used to send commands.
    pub fn new() -> (Self, mpsc::UnboundedSender<IpcCommand>) {
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let server = Self { command_rx };
        (server, command_tx)
    }

    /// Start the IPC server event loop.
    ///
    /// This stub implementation:
    /// - Logs received commands
    /// - Does not actually process them (actor dispatch will come later)
    /// - Exits when the command channel closes
    pub async fn run(mut self) -> Result<(), RuntimeError> {
        info!("IPC server starting");

        loop {
            match self.command_rx.recv().await {
                Some(cmd) => {
                    self.handle_command(cmd).await?;
                }
                None => {
                    info!("IPC server: command channel closed, exiting");
                    break;
                }
            }
        }

        info!("IPC server stopped");
        Ok(())
    }

    /// Handle a single IPC command.
    async fn handle_command(&self, command: IpcCommand) -> Result<(), RuntimeError> {
        match command {
            IpcCommand::StatusRequest => {
                info!("IPC: StatusRequest received (stub: no response yet)");
            }
            IpcCommand::ShutdownRequest => {
                info!("IPC: ShutdownRequest received (stub: no action yet)");
            }
            IpcCommand::InferenceRequest { prompt } => {
                info!(
                    prompt_len = prompt.len(),
                    "IPC: InferenceRequest received (stub: no dispatch yet)"
                );
            }
            IpcCommand::Ping => {
                info!("IPC: Ping received (stub: no pong yet)");
            }
        }

        Ok(())
    }
}

impl Default for IpcServer {
    fn default() -> Self {
        Self::new().0
    }
}

/// Spawn the IPC server in a background task.
///
/// Returns the command sender that can be used to send commands to the server.
pub fn spawn_ipc_server() -> mpsc::UnboundedSender<IpcCommand> {
    let (server, command_tx) = IpcServer::new();

    tokio::spawn(async move {
        if let Err(e) = server.run().await {
            warn!("IPC server error: {}", e);
        }
    });

    info!("IPC server spawned in background task");
    command_tx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ipc_server_constructs() {
        let (_server, _tx) = IpcServer::new();
        // Construction succeeds
    }

    #[tokio::test]
    async fn ipc_server_receives_commands() {
        let (server, tx) = IpcServer::new();

        // Spawn server in background
        let handle = tokio::spawn(async move { server.run().await });

        // Send a command
        tx.send(IpcCommand::Ping).expect("send failed");

        // Drop sender to close channel
        drop(tx);

        // Server should exit cleanly
        let result = handle.await.expect("task panicked");
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn spawn_ipc_server_works() {
        let tx = spawn_ipc_server();

        // Send a command
        tx.send(IpcCommand::StatusRequest).expect("send failed");

        // Give the background task time to process
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Drop sender
        drop(tx);

        // Give the background task time to exit
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }
}
