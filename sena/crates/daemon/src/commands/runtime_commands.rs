//! Runtime-related IPC command handlers.

use async_trait::async_trait;
use ipc::{CommandHandler, IpcError};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// Shared daemon state for runtime commands.
#[derive(Clone)]
pub struct RuntimeState {
    pub boot_time: Instant,
    pub is_ready: Arc<AtomicBool>,
}

impl RuntimeState {
    pub fn new() -> Self {
        Self {
            boot_time: Instant::now(),
            is_ready: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn mark_ready(&self) {
        self.is_ready.store(true, Ordering::SeqCst);
    }
}

/// Handler for "runtime.ping" command.
pub struct PingHandler {
    state: RuntimeState,
}

impl PingHandler {
    pub fn new(state: RuntimeState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl CommandHandler for PingHandler {
    fn name(&self) -> &'static str {
        "runtime.ping"
    }

    fn description(&self) -> &'static str {
        "Check daemon connectivity"
    }

    fn requires_boot(&self) -> bool {
        false
    }

    async fn handle(&self, _payload: Value) -> Result<Value, IpcError> {
        let uptime_secs = self.state.boot_time.elapsed().as_secs();
        Ok(json!({
            "pong": true,
            "uptime_seconds": uptime_secs
        }))
    }
}

/// Handler for "runtime.status" command.
pub struct StatusHandler {
    state: RuntimeState,
    bus: std::sync::Arc<bus::EventBus>,
}

impl StatusHandler {
    pub fn new(state: RuntimeState, bus: std::sync::Arc<bus::EventBus>) -> Self {
        Self { state, bus }
    }
}

#[async_trait]
impl CommandHandler for StatusHandler {
    fn name(&self) -> &'static str {
        "runtime.status"
    }

    fn description(&self) -> &'static str {
        "Get daemon runtime status with per-actor health"
    }

    fn requires_boot(&self) -> bool {
        false
    }

    async fn handle(&self, _payload: Value) -> Result<Value, IpcError> {
        let uptime_secs = self.state.boot_time.elapsed().as_secs();
        let is_ready = self.state.is_ready.load(Ordering::SeqCst);

        // Query supervisor for actor health
        let _ = self
            .bus
            .broadcast(bus::Event::System(bus::SystemEvent::HealthCheckRequest {
                target: None,
            }))
            .await;

        // Wait for HealthCheckResponse (with 1s timeout)
        let health_future = async {
            let mut rx = self.bus.subscribe_broadcast();
            while let Ok(event) = rx.recv().await {
                if let bus::Event::System(bus::SystemEvent::HealthCheckResponse {
                    actors,
                    uptime_seconds,
                }) = event
                {
                    return Some((actors, uptime_seconds));
                }
            }
            None
        };

        let health_result =
            tokio::time::timeout(std::time::Duration::from_secs(1), health_future).await;

        match health_result {
            Ok(Some((actors, supervisor_uptime))) => Ok(json!({
                "status": if is_ready { "ready" } else { "booting" },
                "uptime_seconds": uptime_secs,
                "supervisor_uptime_seconds": supervisor_uptime,
                "actors": actors,
            })),
            Ok(None) | Err(_) => {
                // Timeout or channel error — return basic status without actor details
                Ok(json!({
                    "status": if is_ready { "ready" } else { "booting" },
                    "uptime_seconds": uptime_secs,
                    "actors": [],
                }))
            }
        }
    }
}

/// Handler for "runtime.shutdown" command.
pub struct ShutdownHandler {
    shutdown_tx: tokio::sync::mpsc::UnboundedSender<()>,
    bus: std::sync::Arc<bus::EventBus>,
}

impl ShutdownHandler {
    pub fn new(
        shutdown_tx: tokio::sync::mpsc::UnboundedSender<()>,
        bus: std::sync::Arc<bus::EventBus>,
    ) -> Self {
        Self { shutdown_tx, bus }
    }
}

#[async_trait]
impl CommandHandler for ShutdownHandler {
    fn name(&self) -> &'static str {
        "runtime.shutdown"
    }

    fn description(&self) -> &'static str {
        "Request graceful daemon shutdown"
    }

    fn requires_boot(&self) -> bool {
        false
    }

    async fn handle(&self, _payload: Value) -> Result<Value, IpcError> {
        // Broadcast ShutdownRequested on the bus for observability
        let _ = self
            .bus
            .broadcast(bus::Event::System(bus::SystemEvent::ShutdownRequested))
            .await;

        // Send to private shutdown channel to trigger daemon shutdown
        self.shutdown_tx
            .send(())
            .map_err(|_| IpcError::Internal("shutdown channel closed".to_string()))?;

        Ok(json!({ "status": "shutdown initiated" }))
    }
}
