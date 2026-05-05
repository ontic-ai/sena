//! Runtime-related IPC command handlers.

use async_trait::async_trait;
use ipc::{CommandHandler, IpcError};
use serde_json::{Value, json};
use std::path::PathBuf;
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

fn onboarding_marker_path() -> Result<PathBuf, IpcError> {
    Ok(runtime::config::config_path()
        .map_err(|e| IpcError::CommandFailed(format!("failed to resolve config path: {}", e)))?
        .parent()
        .ok_or_else(|| IpcError::CommandFailed("no parent directory for config".to_string()))?
        .join("onboarding_complete"))
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

/// Handler for "runtime.onboarding_status" command.
pub struct OnboardingStatusHandler;

impl OnboardingStatusHandler {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl CommandHandler for OnboardingStatusHandler {
    fn name(&self) -> &'static str {
        "runtime.onboarding_status"
    }

    fn description(&self) -> &'static str {
        "Check whether first-boot onboarding is still required"
    }

    fn requires_boot(&self) -> bool {
        false
    }

    async fn handle(&self, _payload: Value) -> Result<Value, IpcError> {
        let marker_path = onboarding_marker_path()?;
        let onboarding_required = tokio::fs::metadata(&marker_path).await.is_err();

        Ok(json!({
            "onboarding_required": onboarding_required
        }))
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

/// Handler for "runtime.submit_onboarding_name" command.
pub struct SubmitOnboardingNameHandler {
    bus: std::sync::Arc<bus::EventBus>,
}

impl SubmitOnboardingNameHandler {
    pub fn new(bus: std::sync::Arc<bus::EventBus>) -> Self {
        Self { bus }
    }
}

#[async_trait]
impl CommandHandler for SubmitOnboardingNameHandler {
    fn name(&self) -> &'static str {
        "runtime.submit_onboarding_name"
    }

    fn description(&self) -> &'static str {
        "Submit user name for first-time onboarding — emits SoulEvent::InitializeWithName"
    }

    fn requires_boot(&self) -> bool {
        true
    }

    async fn handle(&self, payload: Value) -> Result<Value, IpcError> {
        let name = payload
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| IpcError::InvalidPayload("missing 'name' field".to_string()))?;

        if name.trim().is_empty() {
            return Err(IpcError::InvalidPayload("name cannot be empty".to_string()));
        }

        if name.len() > 50 {
            return Err(IpcError::InvalidPayload(
                "name too long (max 50 characters)".to_string(),
            ));
        }

        tracing::info!("Submitting onboarding name");

        self.bus
            .broadcast(bus::Event::Soul(bus::SoulEvent::InitializeWithName {
                name: name.to_string(),
            }))
            .await
            .map_err(|e| IpcError::CommandFailed(format!("failed to emit name event: {}", e)))?;

        Ok(json!({ "success": true }))
    }
}

/// Handler for "runtime.submit_onboarding_config" command.
pub struct SubmitOnboardingConfigHandler {
    bus: std::sync::Arc<bus::EventBus>,
}

impl SubmitOnboardingConfigHandler {
    pub fn new(bus: std::sync::Arc<bus::EventBus>) -> Self {
        Self { bus }
    }
}

#[async_trait]
impl CommandHandler for SubmitOnboardingConfigHandler {
    fn name(&self) -> &'static str {
        "runtime.submit_onboarding_config"
    }

    fn description(&self) -> &'static str {
        "Submit config preferences for first-time onboarding — saves file watch paths and clipboard settings"
    }

    fn requires_boot(&self) -> bool {
        false
    }

    async fn handle(&self, payload: Value) -> Result<Value, IpcError> {
        let file_watch_paths: Vec<String> = payload
            .get("file_watch_paths")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let clipboard_observation_enabled = payload
            .get("clipboard_observation_enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        tracing::info!(
            "Submitting onboarding config: {} file watch paths, clipboard: {}",
            file_watch_paths.len(),
            clipboard_observation_enabled
        );

        // Load existing config
        let mut config = runtime::config::load_or_create_config()
            .await
            .map_err(|e| IpcError::CommandFailed(format!("failed to load config: {}", e)))?;

        // Update config with onboarding preferences
        config.file_watch_paths = file_watch_paths
            .into_iter()
            .map(std::path::PathBuf::from)
            .collect();
        config.clipboard_observation_enabled = clipboard_observation_enabled;

        // Save config
        runtime::save_config(&config)
            .await
            .map_err(|e| IpcError::CommandFailed(format!("failed to save config: {}", e)))?;

        tracing::info!("Onboarding config saved successfully");

        // Create onboarding_complete marker file
        let marker_path = onboarding_marker_path()?;

        tokio::fs::write(&marker_path, b"")
            .await
            .map_err(|e| IpcError::CommandFailed(format!("failed to write marker: {}", e)))?;

        tracing::info!(
            "Onboarding marker file created at {}",
            marker_path.display()
        );

        // Emit OnboardingCompleted event
        self.bus
            .broadcast(bus::Event::System(bus::SystemEvent::OnboardingCompleted))
            .await
            .ok();

        Ok(json!({ "success": true }))
    }
}
