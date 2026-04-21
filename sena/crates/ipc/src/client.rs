use crate::{IpcError, IpcRequest};
#[cfg(target_os = "windows")]
use crate::{IpcResponse, PIPE_NAME, framing};
use serde_json::Value;
#[cfg(target_os = "windows")]
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{Mutex, mpsc, oneshot};

/// IPC client for connecting to Sena daemon.
///
/// Supports sending requests, receiving responses, and subscribing to push events.
///
/// The client spawns a background task to receive responses and push events,
/// enabling concurrent operations and event subscriptions.
pub struct IpcClient {
    send_tx: mpsc::UnboundedSender<ClientMessage>,
    push_rx: Arc<Mutex<mpsc::UnboundedReceiver<Value>>>,
    next_id: AtomicU64,
}

/// Internal message types for client communication.
enum ClientMessage {
    /// Send a request and expect a response.
    Request {
        id: u64,
        request: IpcRequest,
        response_tx: oneshot::Sender<Result<Value, IpcError>>,
    },
}

impl IpcClient {
    /// Connect to the Sena daemon.
    ///
    /// Spawns a background task to handle bidirectional communication.
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

        let (send_tx, send_rx) = mpsc::unbounded_channel();
        let (push_tx, push_rx) = mpsc::unbounded_channel();

        // Spawn background task for bidirectional communication
        tokio::spawn(Self::background_task(stream, send_rx, push_tx));

        Ok(Self {
            send_tx,
            push_rx: Arc::new(Mutex::new(push_rx)),
            next_id: AtomicU64::new(1),
        })
    }

    #[cfg(not(target_os = "windows"))]
    pub async fn connect() -> Result<Self, IpcError> {
        Err(IpcError::PlatformNotSupported)
    }

    /// Background task for handling bidirectional IPC communication.
    ///
    /// Receives responses and push events from the daemon, routing them appropriately:
    /// - Responses (id != 0) are sent to the matching pending request
    /// - Push events (id == 0) are broadcast to all subscribers
    #[cfg(target_os = "windows")]
    async fn background_task(
        mut stream: tokio::net::windows::named_pipe::NamedPipeClient,
        mut send_rx: mpsc::UnboundedReceiver<ClientMessage>,
        push_tx: mpsc::UnboundedSender<Value>,
    ) {
        let mut pending_requests: HashMap<u64, oneshot::Sender<Result<Value, IpcError>>> =
            HashMap::new();

        loop {
            tokio::select! {
                // Handle outgoing requests
                Some(msg) = send_rx.recv() => {
                    match msg {
                        ClientMessage::Request { id, request, response_tx } => {
                            pending_requests.insert(id, response_tx);
                            if framing::write_frame(&mut stream, &request).await.is_err() {
                                // Write failed — notify all pending requests and exit
                                for (_, tx) in pending_requests.drain() {
                                    let _ = tx.send(Err(IpcError::ConnectionClosed));
                                }
                                break;
                            }
                        }
                    }
                }
                // Handle incoming responses and push events
                response_result = framing::read_frame(&mut stream) => {
                    let response: IpcResponse = match response_result {
                        Ok(r) => r,
                        Err(_) => {
                            // Connection closed — notify all pending requests and exit
                            for (_, tx) in pending_requests.drain() {
                                let _ = tx.send(Err(IpcError::ConnectionClosed));
                            }
                            break;
                        }
                    };

                    if response.id == 0 {
                        // Push event
                        if let crate::protocol::ResponseStatus::Success { result } = response.status {
                            let _ = push_tx.send(result);
                        }
                    } else {
                        // Response to a pending request
                        if let Some(response_tx) = pending_requests.remove(&response.id) {
                            let result = match response.status {
                                crate::protocol::ResponseStatus::Success { result } => Ok(result),
                                crate::protocol::ResponseStatus::Error { error } => {
                                    Err(IpcError::CommandFailed(error))
                                }
                            };
                            let _ = response_tx.send(result);
                        }
                    }
                }
            }
        }
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
    pub async fn send(&mut self, command: &str, payload: Value) -> Result<Value, IpcError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let request = IpcRequest {
            id,
            command: command.to_string(),
            payload,
        };

        let (response_tx, response_rx) = oneshot::channel();

        self.send_tx
            .send(ClientMessage::Request {
                id,
                request,
                response_tx,
            })
            .map_err(|_| IpcError::ConnectionClosed)?;

        response_rx.await.map_err(|_| IpcError::ConnectionClosed)?
    }

    /// Subscribe to push events from the daemon.
    ///
    /// Returns a receiver that yields push event payloads.
    /// Multiple calls to this method share the same underlying channel,
    /// so only one subscriber should be active to avoid missing events.
    pub fn subscribe_events(&self) -> mpsc::UnboundedReceiver<Value> {
        let (tx, rx) = mpsc::unbounded_channel();
        let push_rx = Arc::clone(&self.push_rx);

        // Spawn a task to forward all push events to the new subscriber
        tokio::spawn(async move {
            let mut locked_rx = push_rx.lock().await;
            while let Some(event) = locked_rx.recv().await {
                if tx.send(event).is_err() {
                    // Subscriber dropped — exit forwarding task
                    break;
                }
            }
        });

        rx
    }
}
