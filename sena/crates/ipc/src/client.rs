use crate::{IpcError, IpcRequest, IpcResponse, PIPE_NAME, framing};
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};

/// IPC client for connecting to Sena daemon.
///
/// Supports sending requests, receiving responses, and subscribing to push events.
pub struct IpcClient {
    #[cfg(target_os = "windows")]
    stream: tokio::net::windows::named_pipe::NamedPipeClient,
    #[cfg(not(target_os = "windows"))]
    _unsupported: (),
    next_id: AtomicU64,
}

impl IpcClient {
    /// Connect to the Sena daemon.
    ///
    /// # Platform Support
    ///
    /// - **Windows**: Connects to named pipe `\\.\pipe\sena`
    /// - **macOS/Linux**: Returns `IpcError::PlatformNotSupported` (Phase 1 limitation)
    ///
    /// # Errors
    ///
    /// Returns `IpcError::DaemonNotRunning` if pipe does not exist.
    /// Returns `IpcError::PlatformNotSupported` on non-Windows platforms.
    #[cfg(target_os = "windows")]
    pub async fn connect() -> Result<Self, IpcError> {
        use tokio::net::windows::named_pipe::ClientOptions;

        let stream = ClientOptions::new().open(PIPE_NAME).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                IpcError::DaemonNotRunning
            } else {
                IpcError::Io(e)
            }
        })?;

        Ok(Self {
            stream,
            next_id: AtomicU64::new(1),
        })
    }

    #[cfg(not(target_os = "windows"))]
    pub async fn connect() -> Result<Self, IpcError> {
        Err(IpcError::PlatformNotSupported)
    }

    /// Check if the daemon is running by attempting to connect.
    ///
    /// Returns `true` if connection succeeds, `false` if daemon is not running.
    pub async fn daemon_running() -> bool {
        #[cfg(target_os = "windows")]
        {
            use tokio::net::windows::named_pipe::ClientOptions;
            ClientOptions::new().open(PIPE_NAME).is_ok()
        }

        #[cfg(not(target_os = "windows"))]
        false
    }

    /// Send a command request and wait for the response.
    ///
    /// # Errors
    ///
    /// Returns `IpcError::ConnectionClosed` if daemon disconnects.
    /// Returns command-specific errors propagated from the handler.
    #[cfg(target_os = "windows")]
    pub async fn send(&mut self, command: &str, payload: Value) -> Result<Value, IpcError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let request = IpcRequest {
            id,
            command: command.to_string(),
            payload,
        };

        framing::write_frame(&mut self.stream, &request).await?;

        let response: IpcResponse = framing::read_frame(&mut self.stream).await?;

        match response.status {
            crate::protocol::ResponseStatus::Success { result } => Ok(result),
            crate::protocol::ResponseStatus::Error { error } => Err(IpcError::CommandFailed(error)),
        }
    }

    #[cfg(not(target_os = "windows"))]
    pub async fn send(&mut self, _command: &str, _payload: Value) -> Result<Value, IpcError> {
        Err(IpcError::PlatformNotSupported)
    }

    /// Subscribe to push events from the daemon.
    ///
    /// Returns a receiver that yields `IpcResponse` frames with `id == 0`.
    ///
    /// # Phase 1 Limitation
    ///
    /// This is a placeholder API. Full implementation requires server-side subscription
    /// management, which is deferred to Phase 2+.
    pub async fn subscribe_events(&mut self) -> Result<(), IpcError> {
        // Phase 1: No-op placeholder
        // Phase 2+: Send Subscribe IPC command, spawn background task to read push frames
        Ok(())
    }
}
