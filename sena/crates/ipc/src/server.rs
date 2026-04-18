use crate::{CommandRegistry, IpcError, IpcRequest, IpcResponse, PIPE_NAME, framing};
use std::sync::Arc;
use tokio::sync::RwLock;

/// IPC server that accepts concurrent client connections over named pipe.
///
/// The server listens on `PIPE_NAME` and spawns a task for each connected client.
/// Requests are dispatched to registered command handlers via `CommandRegistry`.
pub struct IpcServer {
    registry: Arc<RwLock<CommandRegistry>>,
}

impl IpcServer {
    /// Create a new IPC server with the given command registry.
    pub fn new(registry: CommandRegistry) -> Self {
        Self {
            registry: Arc::new(RwLock::new(registry)),
        }
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
            tokio::spawn(async move {
                if let Err(e) = Self::handle_client(server, registry).await {
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
    #[cfg(target_os = "windows")]
    async fn handle_client(
        mut stream: tokio::net::windows::named_pipe::NamedPipeServer,
        registry: Arc<RwLock<CommandRegistry>>,
    ) -> Result<(), IpcError> {
        loop {
            let request: IpcRequest = framing::read_frame(&mut stream).await?;

            let registry = registry.read().await;
            let response = match registry.dispatch(&request.command, request.payload).await {
                Ok(result) => IpcResponse::success(request.id, result),
                Err(e) => IpcResponse::error(request.id, e.to_string()),
            };

            framing::write_frame(&mut stream, &response).await?;
        }
    }
}
