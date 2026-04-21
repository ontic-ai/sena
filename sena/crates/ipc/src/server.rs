use crate::{CommandRegistry, IpcError};
#[cfg(target_os = "windows")]
use crate::{IpcRequest, IpcResponse, PIPE_NAME, framing};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast};

/// IPC server that accepts concurrent client connections over named pipe.
///
/// The server listens on `PIPE_NAME` and spawns a task for each connected client.
/// Requests are dispatched to registered command handlers via `CommandRegistry`.
/// Push events can be broadcast to all connected clients via the push channel.
pub struct IpcServer {
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    registry: Arc<RwLock<CommandRegistry>>,
    /// Push event broadcast channel — daemon forwards bus events here.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    push_tx: broadcast::Sender<Value>,
}

impl IpcServer {
    /// Create a new IPC server with the given command registry.
    ///
    /// Returns the server and a sender for broadcasting push events to all clients.
    pub fn new(registry: CommandRegistry) -> (Self, broadcast::Sender<Value>) {
        let (push_tx, _) = broadcast::channel(100);
        let server = Self {
            registry: Arc::new(RwLock::new(registry)),
            push_tx: push_tx.clone(),
        };
        (server, push_tx)
    }

    /// Start the IPC server and run until shutdown.
    ///
    /// # Platform Support
    ///
    /// - **Windows**: Listens on named pipe `\\.\pipe\sena`
    /// - **macOS/Linux**: Returns `IpcError::PlatformNotSupported` (Phase 1 limitation)
    ///
    /// # Errors
    ///
    /// Returns `IpcError::PlatformNotSupported` on non-Windows platforms.
    /// Returns `IpcError::Io` if pipe creation or accept fails.
    #[cfg(target_os = "windows")]
    pub async fn run(&self) -> Result<(), IpcError> {
        use tokio::net::windows::named_pipe::ServerOptions;

        loop {
            let server = ServerOptions::new()
                .first_pipe_instance(true)
                .create(PIPE_NAME)?;

            server.connect().await?;

            let registry = Arc::clone(&self.registry);
            let push_rx = self.push_tx.subscribe();
            tokio::spawn(async move {
                if let Err(e) = Self::handle_client(server, registry, push_rx).await {
                    eprintln!("IPC client error: {}", e);
                }
            });
        }
    }

    #[cfg(not(target_os = "windows"))]
    pub async fn run(&self) -> Result<(), IpcError> {
        Err(IpcError::PlatformNotSupported)
    }

    /// Handle a single client connection.
    ///
    /// Handles both request-response commands and push events forwarded from the bus.
    #[cfg(target_os = "windows")]
    async fn handle_client(
        mut stream: tokio::net::windows::named_pipe::NamedPipeServer,
        registry: Arc<RwLock<CommandRegistry>>,
        mut push_rx: broadcast::Receiver<Value>,
    ) -> Result<(), IpcError> {
        loop {
            tokio::select! {
                // Handle incoming requests from client
                request_result = framing::read_frame(&mut stream) => {
                    let request: IpcRequest = request_result?;
                    let registry = registry.read().await;
                    let response = match registry.dispatch(&request.command, request.payload).await {
                        Ok(result) => IpcResponse::success(request.id, result),
                        Err(e) => IpcResponse::error(request.id, e.to_string()),
                    };
                    framing::write_frame(&mut stream, &response).await?;
                }
                // Handle push events from daemon
                push_result = push_rx.recv() => {
                    match push_result {
                        Ok(event_payload) => {
                            let push_event = IpcResponse::push_event(event_payload);
                            framing::write_frame(&mut stream, &push_event).await?;
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            // Client fell behind — skip lagged events
                            continue;
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            // Push channel closed — daemon shutting down
                            break;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
