use crate::{CommandRegistry, IpcError};
#[cfg(target_os = "windows")]
use crate::{IpcRequest, IpcResponse, PIPE_NAME, framing};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast};
use tracing::error;

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
        self.run_on_pipe(PIPE_NAME).await
    }

    #[cfg(target_os = "windows")]
    async fn run_on_pipe(&self, pipe_name: &str) -> Result<(), IpcError> {
        use tokio::net::windows::named_pipe::ServerOptions;

        // Claim this pipe name as the first and only server process.
        let mut server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(pipe_name)?;

        loop {
            // Block until a client connects.
            server.connect().await?;

            // Prepare the next idle instance *before* spawning the handler so
            // new clients can connect immediately without waiting for the current
            // handler task to finish.  Subsequent instances do NOT use
            // first_pipe_instance — that flag would fail because the first
            // instance is still alive.
            let next = ServerOptions::new().create(pipe_name)?;

            let registry = Arc::clone(&self.registry);
            let push_rx = self.push_tx.subscribe();
            let connected = std::mem::replace(&mut server, next);
            tokio::spawn(async move {
                if let Err(e) = Self::handle_client(connected, registry, push_rx).await {
                    error!(error = %e, "IPC client error");
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
        let mut events_subscribed = false;

        loop {
            tokio::select! {
                // Handle incoming requests from client
                request_result = framing::read_frame(&mut stream) => {
                    let request: IpcRequest = request_result?;
                    let registry = registry.read().await;

                    if request.command == "events.subscribe" {
                        events_subscribed = true;
                    } else if request.command == "events.unsubscribe" {
                        events_subscribed = false;
                    }

                    let response = match registry.dispatch(&request.command, request.payload).await {
                        Ok(result) => IpcResponse::success(request.id, result),
                        Err(e) => IpcResponse::error(request.id, e.to_string()),
                    };
                    framing::write_frame(&mut stream, &response).await?;
                }
                // Handle push events from daemon
                push_result = push_rx.recv() => {
                    if !events_subscribed {
                        continue;
                    }

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

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;
    use crate::{CommandHandler, IpcRequest, IpcResponse, framing};
    use async_trait::async_trait;
    use serde_json::{Value, json};
    use std::sync::Arc;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};

    struct EchoHandler;

    #[async_trait]
    impl CommandHandler for EchoHandler {
        fn name(&self) -> &'static str {
            "echo"
        }

        fn description(&self) -> &'static str {
            "Echo payload"
        }

        async fn handle(&self, payload: Value) -> Result<Value, IpcError> {
            Ok(json!({"echo": payload}))
        }
    }

    fn unique_pipe_name() -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        format!(r"\\.\pipe\sena-ipc-test-{}-{}", std::process::id(), nanos)
    }

    async fn connect_client(pipe_name: &str) -> NamedPipeClient {
        for _ in 0..50 {
            match ClientOptions::new().open(pipe_name) {
                Ok(client) => return client,
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound
                        || error.raw_os_error() == Some(231) =>
                {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(error) => panic!("failed to connect to test pipe: {error}"),
            }
        }

        panic!("timed out waiting for test pipe to accept clients")
    }

    fn test_registry() -> CommandRegistry {
        let mut registry = CommandRegistry::new();
        registry.register(Arc::new(EchoHandler));
        registry
    }

    #[tokio::test]
    async fn ipc_server_survives_client_disconnect() {
        let pipe_name = unique_pipe_name();
        let (server, _push_tx) = IpcServer::new(test_registry());
        let server_task = tokio::spawn({
            let pipe_name = pipe_name.clone();
            async move {
                let _ = server.run_on_pipe(&pipe_name).await;
            }
        });

        let first_client = connect_client(&pipe_name).await;
        drop(first_client);

        let mut second_client = connect_client(&pipe_name).await;
        let request = IpcRequest {
            id: 1,
            command: "echo".to_string(),
            payload: json!({"message": "still alive"}),
        };

        framing::write_frame(&mut second_client, &request)
            .await
            .expect("write request to second client");
        let response: IpcResponse = framing::read_frame(&mut second_client)
            .await
            .expect("read response from second client");

        assert!(response.success);
        assert_eq!(response.payload, json!({"echo": {"message": "still alive"}}));

        server_task.abort();
    }

    #[tokio::test]
    async fn ipc_multiple_clients_connect_simultaneously() {
        let pipe_name = unique_pipe_name();
        let (server, _push_tx) = IpcServer::new(test_registry());
        let server_task = tokio::spawn({
            let pipe_name = pipe_name.clone();
            async move {
                let _ = server.run_on_pipe(&pipe_name).await;
            }
        });

        let (mut first_client, mut second_client) =
            tokio::join!(connect_client(&pipe_name), connect_client(&pipe_name));

        let first_request = IpcRequest {
            id: 1,
            command: "echo".to_string(),
            payload: json!({"client": 1}),
        };
        let second_request = IpcRequest {
            id: 2,
            command: "echo".to_string(),
            payload: json!({"client": 2}),
        };

        let first_exchange = async {
            framing::write_frame(&mut first_client, &first_request)
                .await
                .expect("write request to first client");
            let response: IpcResponse = framing::read_frame(&mut first_client)
                .await
                .expect("read response from first client");
            response
        };

        let second_exchange = async {
            framing::write_frame(&mut second_client, &second_request)
                .await
                .expect("write request to second client");
            let response: IpcResponse = framing::read_frame(&mut second_client)
                .await
                .expect("read response from second client");
            response
        };

        let (first_response, second_response) = tokio::join!(first_exchange, second_exchange);

        assert!(first_response.success);
        assert!(second_response.success);
        assert_eq!(first_response.payload, json!({"echo": {"client": 1}}));
        assert_eq!(second_response.payload, json!({"echo": {"client": 2}}));

        server_task.abort();
    }
}
