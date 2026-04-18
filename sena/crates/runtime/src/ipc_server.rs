//! IPC server — accepts commands from CLI and processes them via the bus.

use crate::error::RuntimeError;
use bus::{Event, EventBus, SystemEvent};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
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
    /// List all background loops.
    ListLoops,
    /// Toggle a background loop by name.
    ToggleLoop { loop_name: String, enabled: bool },
    /// Request debug info.
    DebugInfo,
    /// Set verbose logging.
    SetVerbose { enabled: bool },
    /// Request memory stats.
    MemoryStats,
    /// Request config dump.
    ConfigDump,
}

/// IPC response types.
#[derive(Debug, Clone)]
pub enum IpcResponse {
    /// Status response with actor health.
    Status {
        actors: Vec<bus::events::system::ActorHealth>,
        uptime_seconds: u64,
    },
    /// Shutdown acknowledged.
    ShutdownAcknowledged,
    /// Pong response to Ping.
    Pong,
    /// Generic acknowledgment.
    Ok,
    /// List of all registered loops.
    LoopsList { loops: Vec<LoopInfo> },
    /// Debug info.
    DebugInfo { info: String },
    /// Verbose logging set.
    VerboseSet { enabled: bool },
    /// Memory stats.
    MemoryStats { stats: String },
    /// Config dump.
    ConfigDump { config: String },
    /// Lifecycle event forwarded from the bus.
    Event(IpcEvent),
}

/// Information about a registered background loop.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LoopInfo {
    /// Loop name.
    pub name: String,
    /// Loop description.
    pub description: String,
    /// Current enabled state.
    pub enabled: bool,
}

/// IPC event types — lifecycle events forwarded from the daemon to connected clients.
#[derive(Debug, Clone)]
pub enum IpcEvent {
    /// Boot sequence completed, all actors ready.
    BootComplete,
    /// Shutdown initiated, actors stopping.
    ShutdownInitiated,
    /// Loop status changed.
    LoopStatusChanged { loop_name: String, enabled: bool },
}

/// IPC client handle — provides in-process connection to the daemon.
///
/// The CLI uses this handle to send commands and receive responses/events
/// from the running daemon.
#[derive(Debug)]
pub struct IpcClientHandle {
    /// Send commands to the IPC server.
    pub command_tx: mpsc::UnboundedSender<IpcCommand>,
    /// Receive responses and events from the IPC server.
    pub response_rx: mpsc::UnboundedReceiver<IpcResponse>,
}

/// IPC server handle.
pub struct IpcServer {
    command_rx: mpsc::UnboundedReceiver<IpcCommand>,
    response_tx: mpsc::UnboundedSender<IpcResponse>,
    bus: Arc<EventBus>,
    loop_registry: LoopRegistry,
}

/// Background loop registry.
struct LoopRegistry {
    loops: std::collections::HashMap<String, LoopEntry>,
}

struct LoopEntry {
    description: String,
    enabled: bool,
}

impl LoopRegistry {
    fn new() -> Self {
        let mut loops = std::collections::HashMap::new();

        // Register canonical loops from governance §17.2
        loops.insert(
            "ctp".to_string(),
            LoopEntry {
                description: "Continuous thought processing — signal ingestion and proactive inference trigger".to_string(),
                enabled: true,
            },
        );
        loops.insert(
            "memory_consolidation".to_string(),
            LoopEntry {
                description:
                    "Periodic memory consolidation — moves working memory to long-term store"
                        .to_string(),
                enabled: true,
            },
        );
        loops.insert(
            "platform_polling".to_string(),
            LoopEntry {
                description:
                    "Platform signal polling — active window, clipboard, keystroke cadence"
                        .to_string(),
                enabled: true,
            },
        );
        loops.insert(
            "screen_capture".to_string(),
            LoopEntry {
                description:
                    "Screen capture for vision-capable models — periodic screenshot acquisition"
                        .to_string(),
                enabled: true,
            },
        );
        loops.insert(
            "speech".to_string(),
            LoopEntry {
                description: "Speech input loop — wakeword detection and/or continuous STT capture"
                    .to_string(),
                enabled: true,
            },
        );
        loops.insert(
            "vram_monitor".to_string(),
            LoopEntry {
                description: "Real-time VRAM usage monitoring — polls GPU memory every 10s"
                    .to_string(),
                enabled: true,
            },
        );

        Self { loops }
    }

    fn list(&self) -> Vec<LoopInfo> {
        self.loops
            .iter()
            .map(|(name, entry)| LoopInfo {
                name: name.clone(),
                description: entry.description.clone(),
                enabled: entry.enabled,
            })
            .collect()
    }

    fn toggle(&mut self, loop_name: &str, enabled: bool) -> bool {
        if let Some(entry) = self.loops.get_mut(loop_name) {
            entry.enabled = enabled;
            true
        } else {
            false
        }
    }
}

impl IpcServer {
    /// Create a new IPC server.
    ///
    /// Returns (server, client_handle) where client_handle can be used by
    /// the CLI to send commands and receive responses/events.
    pub fn new(bus: Arc<EventBus>) -> (Self, IpcClientHandle) {
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (response_tx, response_rx) = mpsc::unbounded_channel();
        let server = Self {
            command_rx,
            response_tx,
            bus,
            loop_registry: LoopRegistry::new(),
        };
        let client_handle = IpcClientHandle {
            command_tx,
            response_rx,
        };
        (server, client_handle)
    }

    /// Start the IPC server event loop.
    ///
    /// This implementation:
    /// - Receives commands from the command channel
    /// - Dispatches them via the bus
    /// - Sends responses back via the response channel
    /// - Forwards lifecycle events from the bus to connected clients
    /// - Exits when the command channel closes
    pub async fn run(mut self) -> Result<(), RuntimeError> {
        info!("IPC server starting");

        // Subscribe to bus events for lifecycle event forwarding
        let mut bus_rx = self.bus.subscribe_broadcast();

        loop {
            tokio::select! {
                // Handle incoming commands from CLI
                cmd = self.command_rx.recv() => {
                    match cmd {
                        Some(cmd) => {
                            if let Err(e) = self.handle_command(cmd).await {
                                warn!("IPC server: command handling failed: {}", e);
                            }
                        }
                        None => {
                            info!("IPC server: command channel closed, exiting");
                            break;
                        }
                    }
                }
                // Forward lifecycle events from bus to connected clients
                event = bus_rx.recv() => {
                    if let Ok(event) = event
                        && let Err(e) = self.forward_bus_event(event).await
                    {
                        warn!("IPC server: event forwarding failed: {}", e);
                    }
                }
            }
        }

        info!("IPC server stopped");
        Ok(())
    }

    /// Handle a single IPC command.
    async fn handle_command(&mut self, command: IpcCommand) -> Result<(), RuntimeError> {
        match command {
            IpcCommand::StatusRequest => {
                info!("IPC: StatusRequest received");
                // Subscribe BEFORE broadcasting to avoid race condition
                let mut rx = self.bus.subscribe_broadcast();

                // Now broadcast health check request
                let _ = self
                    .bus
                    .broadcast(Event::System(SystemEvent::HealthCheckRequest {
                        target: None,
                    }))
                    .await;

                // Wait for HealthCheckResponse
                tokio::time::timeout(tokio::time::Duration::from_secs(2), async {
                    while let Ok(event) = rx.recv().await {
                        if let Event::System(SystemEvent::HealthCheckResponse {
                            actors,
                            uptime_seconds,
                        }) = event
                        {
                            let _ = self.response_tx.send(IpcResponse::Status {
                                actors,
                                uptime_seconds,
                            });
                            break;
                        }
                    }
                })
                .await
                .map_err(|_| RuntimeError::IpcServerFailed("status timeout".to_string()))?;
            }
            IpcCommand::ShutdownRequest => {
                info!("IPC: ShutdownRequest received");
                let _ = self
                    .bus
                    .broadcast(Event::System(SystemEvent::ShutdownRequested))
                    .await;
                let _ = self.response_tx.send(IpcResponse::ShutdownAcknowledged);
            }
            IpcCommand::InferenceRequest { prompt } => {
                info!(prompt_len = prompt.len(), "IPC: InferenceRequest received");
                // TODO: dispatch inference request event when inference event type is ready
                let _ = self.response_tx.send(IpcResponse::Ok);
            }
            IpcCommand::Ping => {
                info!("IPC: Ping received");
                let _ = self.response_tx.send(IpcResponse::Pong);
            }
            IpcCommand::ListLoops => {
                info!("IPC: ListLoops received");
                let loops = self.loop_registry.list();
                let _ = self.response_tx.send(IpcResponse::LoopsList { loops });
            }
            IpcCommand::ToggleLoop { loop_name, enabled } => {
                info!(
                    loop_name = %loop_name,
                    enabled = enabled,
                    "IPC: ToggleLoop received"
                );

                // Update registry state
                if self.loop_registry.toggle(&loop_name, enabled) {
                    // Broadcast control request to actors
                    let _ = self
                        .bus
                        .broadcast(Event::System(SystemEvent::LoopControlRequested {
                            loop_name: loop_name.clone(),
                            enabled,
                        }))
                        .await;

                    // Emit status change for client observation
                    let _ = self
                        .bus
                        .broadcast(Event::System(SystemEvent::LoopStatusChanged {
                            loop_name,
                            enabled,
                        }))
                        .await;

                    let _ = self.response_tx.send(IpcResponse::Ok);
                } else {
                    // Unknown loop name — still send Ok, but log warning
                    warn!(loop_name = %loop_name, "ToggleLoop: unknown loop name");
                    let _ = self.response_tx.send(IpcResponse::Ok);
                }
            }
            IpcCommand::DebugInfo => {
                info!("IPC: DebugInfo received");
                // Minimal stub: acknowledge that debug info is not fully implemented
                let info = "Debug info not yet fully implemented in Phase 5".to_string();
                let _ = self.response_tx.send(IpcResponse::DebugInfo { info });
            }
            IpcCommand::SetVerbose { enabled } => {
                info!(enabled = enabled, "IPC: SetVerbose received");
                // Minimal stub: acknowledge verbose state (not yet wired to logger)
                let _ = self.response_tx.send(IpcResponse::VerboseSet { enabled });
            }
            IpcCommand::MemoryStats => {
                info!("IPC: MemoryStats received");
                // Minimal stub: acknowledge that memory stats are not fully implemented
                let stats = "Memory stats not yet fully implemented in Phase 5".to_string();
                let _ = self.response_tx.send(IpcResponse::MemoryStats { stats });
            }
            IpcCommand::ConfigDump => {
                info!("IPC: ConfigDump received");
                // Minimal stub: acknowledge that config dump is not fully implemented
                let config = "Config dump not yet fully implemented in Phase 5".to_string();
                let _ = self.response_tx.send(IpcResponse::ConfigDump { config });
            }
        }

        Ok(())
    }

    /// Forward relevant bus events to connected IPC clients.
    async fn forward_bus_event(&self, event: Event) -> Result<(), RuntimeError> {
        match event {
            Event::System(SystemEvent::BootComplete) => {
                info!("IPC server: forwarding BootComplete to client");
                let _ = self
                    .response_tx
                    .send(IpcResponse::Event(IpcEvent::BootComplete));
            }
            Event::System(SystemEvent::ShutdownInitiated) => {
                info!("IPC server: forwarding ShutdownInitiated to client");
                let _ = self
                    .response_tx
                    .send(IpcResponse::Event(IpcEvent::ShutdownInitiated));
            }
            Event::System(SystemEvent::LoopStatusChanged { loop_name, enabled }) => {
                info!(
                    loop_name = %loop_name,
                    enabled = enabled,
                    "IPC server: forwarding LoopStatusChanged to client"
                );
                let _ = self
                    .response_tx
                    .send(IpcResponse::Event(IpcEvent::LoopStatusChanged {
                        loop_name,
                        enabled,
                    }));
            }
            _ => {
                // Not a lifecycle event we forward
            }
        }
        Ok(())
    }
}

/// Spawn the IPC server in a background task.
///
/// Returns (join_handle, client_handle) where client_handle can be used by
/// the CLI to connect to the running daemon in-process.
pub fn spawn_ipc_server(bus: Arc<EventBus>) -> (JoinHandle<()>, IpcClientHandle) {
    let (server, client_handle) = IpcServer::new(bus.clone());

    let handle = tokio::spawn(async move {
        if let Err(e) = server.run().await {
            warn!("IPC server error: {}", e);
        }
    });

    info!("IPC server spawned in background task");

    (handle, client_handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ipc_server_constructs() {
        let bus = Arc::new(EventBus::new());
        let (_server, _client_handle) = IpcServer::new(bus);
        // Construction succeeds
    }

    #[tokio::test]
    async fn ipc_server_receives_commands() {
        let bus = Arc::new(EventBus::new());
        let (server, client_handle) = IpcServer::new(bus);

        // Spawn server in background
        let handle = tokio::spawn(async move { server.run().await });

        // Send a command
        client_handle
            .command_tx
            .send(IpcCommand::Ping)
            .expect("send failed");

        // Drop sender to close channel
        drop(client_handle);

        // Server should exit cleanly
        let result = handle.await.expect("task panicked");
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn spawn_ipc_server_works() {
        let bus = Arc::new(EventBus::new());
        let (handle, _client_handle) = spawn_ipc_server(bus);

        // Give the background task time to start
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Abort the handle
        handle.abort();

        // Give the background task time to exit
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }

    #[tokio::test]
    async fn ping_returns_pong() {
        let bus = Arc::new(EventBus::new());
        let (server, client_handle) = IpcServer::new(bus);

        // Spawn server
        let _handle = tokio::spawn(async move { server.run().await });

        // Send ping
        client_handle
            .command_tx
            .send(IpcCommand::Ping)
            .expect("send failed");

        // Receive pong
        let mut rx = client_handle.response_rx;
        let response =
            tokio::time::timeout(tokio::time::Duration::from_millis(100), rx.recv()).await;

        assert!(response.is_ok());
        let resp = response.unwrap();
        assert!(matches!(resp, Some(IpcResponse::Pong)));
    }

    #[tokio::test]
    async fn shutdown_request_broadcasts_event() {
        let bus = Arc::new(EventBus::new());
        let (server, client_handle) = IpcServer::new(bus.clone());

        // Subscribe to bus events
        let mut bus_rx = bus.subscribe_broadcast();

        // Spawn server
        let _handle = tokio::spawn(async move { server.run().await });

        // Send shutdown request
        client_handle
            .command_tx
            .send(IpcCommand::ShutdownRequest)
            .expect("send failed");

        // Verify ShutdownRequested event was broadcast
        let event = tokio::time::timeout(tokio::time::Duration::from_millis(100), bus_rx.recv())
            .await
            .expect("timeout")
            .expect("recv failed");

        assert!(matches!(
            event,
            Event::System(SystemEvent::ShutdownRequested)
        ));

        // Verify response was sent
        let mut response_rx = client_handle.response_rx;
        let response =
            tokio::time::timeout(tokio::time::Duration::from_millis(100), response_rx.recv())
                .await
                .expect("timeout");

        assert!(matches!(response, Some(IpcResponse::ShutdownAcknowledged)));
    }

    #[tokio::test]
    async fn toggle_loop_broadcasts_event() {
        let bus = Arc::new(EventBus::new());
        let (server, client_handle) = IpcServer::new(bus.clone());

        // Subscribe to bus events
        let mut bus_rx = bus.subscribe_broadcast();

        // Spawn server
        let _handle = tokio::spawn(async move { server.run().await });

        // Send toggle loop command
        client_handle
            .command_tx
            .send(IpcCommand::ToggleLoop {
                loop_name: "ctp".to_string(),
                enabled: false,
            })
            .expect("send failed");

        // Verify LoopControlRequested event was broadcast
        let event = tokio::time::timeout(tokio::time::Duration::from_millis(100), bus_rx.recv())
            .await
            .expect("timeout")
            .expect("recv failed");

        if let Event::System(SystemEvent::LoopControlRequested { loop_name, enabled }) = event {
            assert_eq!(loop_name, "ctp");
            assert_eq!(enabled, false);
        } else {
            panic!("Expected LoopControlRequested event");
        }
    }

    #[tokio::test]
    async fn ipc_server_forwards_boot_complete_event() {
        let bus = Arc::new(EventBus::new());
        let (server, client_handle) = IpcServer::new(bus.clone());

        // Spawn server
        let _handle = tokio::spawn(async move { server.run().await });

        // Give server time to subscribe to bus
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Broadcast BootComplete on the bus
        let _ = bus
            .broadcast(Event::System(SystemEvent::BootComplete))
            .await;

        // Verify IpcEvent::BootComplete was forwarded to client
        let mut response_rx = client_handle.response_rx;
        let response =
            tokio::time::timeout(tokio::time::Duration::from_millis(100), response_rx.recv())
                .await
                .expect("timeout");

        assert!(matches!(
            response,
            Some(IpcResponse::Event(IpcEvent::BootComplete))
        ));
    }

    #[tokio::test]
    async fn ipc_server_forwards_shutdown_initiated_event() {
        let bus = Arc::new(EventBus::new());
        let (server, client_handle) = IpcServer::new(bus.clone());

        // Spawn server
        let _handle = tokio::spawn(async move { server.run().await });

        // Give server time to subscribe to bus
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Broadcast ShutdownInitiated on the bus
        let _ = bus
            .broadcast(Event::System(SystemEvent::ShutdownInitiated))
            .await;

        // Verify IpcEvent::ShutdownInitiated was forwarded to client
        let mut response_rx = client_handle.response_rx;
        let response =
            tokio::time::timeout(tokio::time::Duration::from_millis(100), response_rx.recv())
                .await
                .expect("timeout");

        assert!(matches!(
            response,
            Some(IpcResponse::Event(IpcEvent::ShutdownInitiated))
        ));
    }

    #[tokio::test]
    async fn ipc_server_forwards_loop_status_changed_event() {
        let bus = Arc::new(EventBus::new());
        let (server, client_handle) = IpcServer::new(bus.clone());

        // Spawn server
        let _handle = tokio::spawn(async move { server.run().await });

        // Give server time to subscribe to bus
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Broadcast LoopStatusChanged on the bus
        let _ = bus
            .broadcast(Event::System(SystemEvent::LoopStatusChanged {
                loop_name: "ctp".to_string(),
                enabled: false,
            }))
            .await;

        // Verify IpcEvent::LoopStatusChanged was forwarded to client
        let mut response_rx = client_handle.response_rx;
        let response =
            tokio::time::timeout(tokio::time::Duration::from_millis(100), response_rx.recv())
                .await
                .expect("timeout");

        if let Some(IpcResponse::Event(IpcEvent::LoopStatusChanged { loop_name, enabled })) =
            response
        {
            assert_eq!(loop_name, "ctp");
            assert_eq!(enabled, false);
        } else {
            panic!("Expected IpcEvent::LoopStatusChanged, got {:?}", response);
        }
    }

    #[tokio::test]
    async fn status_request_returns_status_response() {
        let bus = Arc::new(EventBus::new());
        let (server, client_handle) = IpcServer::new(bus.clone());

        // Spawn server
        let _handle = tokio::spawn(async move { server.run().await });

        // Spawn a task to respond to HealthCheckRequest
        let bus_clone = bus.clone();
        tokio::spawn(async move {
            let mut rx = bus_clone.subscribe_broadcast();
            while let Ok(event) = rx.recv().await {
                if let Event::System(SystemEvent::HealthCheckRequest { .. }) = event {
                    // Respond with health check
                    let _ = bus_clone
                        .broadcast(Event::System(SystemEvent::HealthCheckResponse {
                            actors: vec![],
                            uptime_seconds: 42,
                        }))
                        .await;
                    break;
                }
            }
        });

        // Give both server and responder time to subscribe
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;

        // Send status request
        client_handle
            .command_tx
            .send(IpcCommand::StatusRequest)
            .expect("send failed");

        // Verify we receive a Status response (proves no race condition)
        let mut response_rx = client_handle.response_rx;
        let response =
            tokio::time::timeout(tokio::time::Duration::from_secs(3), response_rx.recv())
                .await
                .expect("timeout waiting for status response");

        match response {
            Some(IpcResponse::Status {
                actors,
                uptime_seconds,
            }) => {
                assert_eq!(actors.len(), 0);
                assert_eq!(uptime_seconds, 42);
            }
            other => panic!("Expected Status response, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn list_loops_returns_registry() {
        let bus = Arc::new(EventBus::new());
        let (server, client_handle) = IpcServer::new(bus.clone());

        // Spawn server
        let _handle = tokio::spawn(async move { server.run().await });

        // Send list loops request
        client_handle
            .command_tx
            .send(IpcCommand::ListLoops)
            .expect("send failed");

        // Verify we receive a LoopsList response with canonical loops
        let mut response_rx = client_handle.response_rx;
        let response =
            tokio::time::timeout(tokio::time::Duration::from_millis(100), response_rx.recv())
                .await
                .expect("timeout");

        match response {
            Some(IpcResponse::LoopsList { loops }) => {
                // Verify all 6 canonical loops are present
                assert_eq!(loops.len(), 6);
                let loop_names: Vec<String> = loops.iter().map(|l| l.name.clone()).collect();
                assert!(loop_names.contains(&"ctp".to_string()));
                assert!(loop_names.contains(&"memory_consolidation".to_string()));
                assert!(loop_names.contains(&"platform_polling".to_string()));
                assert!(loop_names.contains(&"screen_capture".to_string()));
                assert!(loop_names.contains(&"speech".to_string()));
                assert!(loop_names.contains(&"vram_monitor".to_string()));

                // All should be enabled by default
                for loop_info in loops {
                    assert!(
                        loop_info.enabled,
                        "{} should be enabled by default",
                        loop_info.name
                    );
                }
            }
            other => panic!("Expected LoopsList response, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn toggle_loop_updates_registry_state() {
        let bus = Arc::new(EventBus::new());
        let (server, client_handle) = IpcServer::new(bus.clone());

        // Spawn server
        let _handle = tokio::spawn(async move { server.run().await });

        // Toggle ctp loop off
        client_handle
            .command_tx
            .send(IpcCommand::ToggleLoop {
                loop_name: "ctp".to_string(),
                enabled: false,
            })
            .expect("send failed");

        // Wait for acknowledgment
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // List loops to verify state changed
        client_handle
            .command_tx
            .send(IpcCommand::ListLoops)
            .expect("send failed");

        // Verify ctp is now disabled in registry
        let mut response_rx = client_handle.response_rx;

        // Skip any intermediate responses (Ok, events) until we get LoopsList
        let mut found_loops_list = false;
        for _ in 0..5 {
            match tokio::time::timeout(tokio::time::Duration::from_millis(100), response_rx.recv())
                .await
            {
                Ok(Some(IpcResponse::LoopsList { loops })) => {
                    let ctp_loop = loops
                        .iter()
                        .find(|l| l.name == "ctp")
                        .expect("ctp loop not found");
                    assert_eq!(
                        ctp_loop.enabled, false,
                        "ctp should be disabled after toggle"
                    );
                    found_loops_list = true;
                    break;
                }
                Ok(Some(_)) => continue, // Skip other responses
                Ok(None) => break,
                Err(_) => break,
            }
        }

        assert!(found_loops_list, "LoopsList response not received");
    }
}
