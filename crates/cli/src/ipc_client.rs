//! IPC client for CLI → daemon communication.
//!
//! Connects to the running Sena daemon over Unix socket (macOS/Linux) or
//! named pipe (Windows). Sends IpcMessage requests, receives IpcMessage responses
//! including pushed events from the daemon bus.

use bus::{IpcMessage, IpcPayload};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[cfg(unix)]
use std::path::PathBuf;
#[cfg(unix)]
use tokio::net::UnixStream;

#[cfg(windows)]
use tokio::net::windows::named_pipe::ClientOptions;

/// IPC client for CLI → daemon communication.
pub struct IpcClient {
    /// Read half (receives IpcMessage from daemon).
    #[cfg(unix)]
    read_half: BufReader<tokio::io::ReadHalf<UnixStream>>,
    #[cfg(windows)]
    read_half: BufReader<tokio::io::ReadHalf<tokio::net::windows::named_pipe::NamedPipeClient>>,

    /// Write half (sends IpcMessage to daemon).
    #[cfg(unix)]
    write_half: tokio::io::WriteHalf<UnixStream>,
    #[cfg(windows)]
    write_half: tokio::io::WriteHalf<tokio::net::windows::named_pipe::NamedPipeClient>,

    /// Auto-incrementing request ID counter.
    next_id: AtomicU64,
}

impl IpcClient {
    /// Connect to the running daemon IPC endpoint.
    /// Returns Err if daemon is not running or connection fails.
    pub async fn connect() -> Result<Self, IpcClientError> {
        // Check for daemon presence first.
        if !runtime::is_daemon_running() {
            return Err(IpcClientError::DaemonNotRunning);
        }

        #[cfg(unix)]
        {
            let socket_path = ipc_endpoint();
            let stream = UnixStream::connect(&socket_path)
                .await
                .map_err(|e| IpcClientError::ConnectionFailed(e.to_string()))?;
            let (read_half, write_half) = tokio::io::split(stream);
            let read_half = BufReader::new(read_half);

            tracing::info!("IPC client connected to {:?}", socket_path);

            Ok(Self {
                read_half,
                write_half,
                next_id: AtomicU64::new(1),
            })
        }

        #[cfg(windows)]
        {
            let pipe_name = ipc_endpoint();
            let client = ClientOptions::new()
                .open(pipe_name)
                .map_err(|e| IpcClientError::ConnectionFailed(e.to_string()))?;
            let (read_half, write_half) = tokio::io::split(client);
            let read_half = BufReader::new(read_half);

            tracing::info!("IPC client connected to {}", pipe_name);

            Ok(Self {
                read_half,
                write_half,
                next_id: AtomicU64::new(1),
            })
        }
    }

    /// Send a command to the daemon. Returns the request id.
    pub async fn send(&mut self, payload: IpcPayload) -> Result<u64, IpcClientError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let msg = IpcMessage { id, payload };

        let json = serde_json::to_string(&msg)
            .map_err(|e| IpcClientError::Protocol(format!("serialize failed: {}", e)))?;

        let mut line = json;
        line.push('\n');

        self.write_half
            .write_all(line.as_bytes())
            .await
            .map_err(|e| IpcClientError::Protocol(format!("write failed: {}", e)))?;

        Ok(id)
    }

    /// Receive the next message from the daemon (blocking until arrival or disconnect).
    pub async fn recv(&mut self) -> Option<IpcMessage> {
        let mut line = String::new();
        match self.read_half.read_line(&mut line).await {
            Ok(0) => {
                tracing::info!("IPC daemon disconnected (EOF)");
                None
            }
            Ok(_) => match serde_json::from_str::<IpcMessage>(&line) {
                Ok(msg) => Some(msg),
                Err(e) => {
                    tracing::warn!("IPC malformed JSON: {}", e);
                    None
                }
            },
            Err(e) => {
                tracing::error!("IPC recv error: {}", e);
                None
            }
        }
    }

    /// Send Ping and wait for Pong (basic health check).
    #[allow(dead_code)]
    pub async fn ping(&mut self) -> Result<(), IpcClientError> {
        let id = self.send(IpcPayload::Ping).await?;

        // Wait for Pong response.
        while let Some(msg) = self.recv().await {
            if msg.id == id && matches!(msg.payload, IpcPayload::Pong) {
                return Ok(());
            }
        }

        Err(IpcClientError::Disconnected)
    }
}

#[cfg(unix)]
fn ipc_endpoint() -> PathBuf {
    let user = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());
    std::env::temp_dir().join(format!("sena-ipc-{}.sock", user))
}

#[cfg(windows)]
fn ipc_endpoint() -> &'static str {
    r"\\.\pipe\sena_ipc"
}

#[derive(Debug, thiserror::Error)]
pub enum IpcClientError {
    #[error("daemon is not running — start `sena` first")]
    DaemonNotRunning,

    #[error("connection to daemon failed: {0}")]
    ConnectionFailed(String),

    #[error("IPC protocol error: {0}")]
    Protocol(String),

    #[error("daemon disconnected")]
    #[allow(dead_code)]
    Disconnected,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_endpoint_differs_from_lock_path() {
        #[cfg(unix)]
        {
            let lock_path = runtime::single_instance::ipc_socket_path();
            let ipc_path = ipc_endpoint();
            assert_ne!(
                lock_path, ipc_path,
                "IPC endpoint must be different from single-instance lock socket"
            );
        }

        // Windows uses different pipe names, so they are inherently distinct.
        // Single instance: \\.\pipe\sena_single_instance
        // IPC: \\.\pipe\sena_ipc
    }

    #[tokio::test]
    async fn ipc_client_errors_when_daemon_not_running() {
        // This test assumes no daemon is running.
        // If a daemon is running, this test will fail — that's expected.
        let result = IpcClient::connect().await;
        assert!(
            result.is_err(),
            "connect() should fail when daemon is not running"
        );
        if let Err(e) = result {
            assert!(
                matches!(e, IpcClientError::DaemonNotRunning)
                    || matches!(e, IpcClientError::ConnectionFailed(_)),
                "expected DaemonNotRunning or ConnectionFailed, got: {:?}",
                e
            );
        }
    }
}
