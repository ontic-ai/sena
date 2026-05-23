//! Runtime-related IPC command handlers.

use async_trait::async_trait;
use ipc::{CommandHandler, IpcError};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use tokio::sync::{Mutex, RwLock, oneshot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DaemonControlMessage {
    Shutdown,
    RestartInTestMode,
}

/// Shared daemon state for runtime commands.
#[derive(Clone)]
pub struct RuntimeState {
    pub boot_time: Instant,
    pub is_ready: Arc<AtomicBool>,
    pending_test_mode: Arc<AtomicBool>,
    selected_actors: Arc<RwLock<BTreeSet<String>>>,
    boot_selection_tx: Arc<Mutex<Option<oneshot::Sender<runtime::ActorSelection>>>>,
}

impl RuntimeState {
    pub fn new() -> Self {
        Self {
            boot_time: Instant::now(),
            is_ready: Arc::new(AtomicBool::new(false)),
            pending_test_mode: Arc::new(AtomicBool::new(false)),
            selected_actors: Arc::new(RwLock::new(BTreeSet::new())),
            boot_selection_tx: Arc::new(Mutex::new(None)),
        }
    }

    pub fn mark_ready(&self) {
        self.is_ready.store(true, Ordering::SeqCst);
    }

    pub fn set_test_mode_pending(&self, pending: bool) {
        self.pending_test_mode.store(pending, Ordering::SeqCst);
    }

    pub fn test_mode_pending(&self) -> bool {
        self.pending_test_mode.load(Ordering::SeqCst)
    }

    pub async fn set_selected_actors<I, S>(&self, actors: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut selected = self.selected_actors.write().await;
        selected.clear();
        selected.extend(actors.into_iter().map(|actor| actor.as_ref().to_string()));
    }

    pub async fn selected_actor_ids(&self) -> Vec<String> {
        self.selected_actors.read().await.iter().cloned().collect()
    }

    pub async fn ensure_actors_running(&self, required_actors: &[&str]) -> Result<(), IpcError> {
        let selected = self.selected_actors.read().await;
        let missing = required_actors
            .iter()
            .copied()
            .filter(|actor| !selected.contains(*actor))
            .collect::<Vec<_>>();

        if missing.is_empty() {
            return Ok(());
        }

        let label = if missing.len() == 1 { "actor" } else { "actors" };
        Err(IpcError::CommandFailed(format!(
            "{} not running in this session: {}",
            label,
            missing.join(", ")
        )))
    }

    pub async fn install_boot_selection_sender(
        &self,
        sender: oneshot::Sender<runtime::ActorSelection>,
    ) {
        let mut slot = self.boot_selection_tx.lock().await;
        *slot = Some(sender);
    }

    pub async fn clear_boot_selection_sender(&self) {
        let mut slot = self.boot_selection_tx.lock().await;
        slot.take();
    }

    pub async fn submit_boot_selection(
        &self,
        selection: runtime::ActorSelection,
    ) -> Result<(), IpcError> {
        let mut slot = self.boot_selection_tx.lock().await;
        let Some(sender) = slot.take() else {
            return Err(IpcError::CommandFailed(
                "test mode selection is not currently pending".to_string(),
            ));
        };

        sender.send(selection).map_err(|_| {
            IpcError::CommandFailed("test mode selection receiver dropped".to_string())
        })?;
        self.set_test_mode_pending(false);
        Ok(())
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
    bus: Option<std::sync::Arc<bus::EventBus>>,
}

impl StatusHandler {
    pub fn new(state: RuntimeState, bus: Option<std::sync::Arc<bus::EventBus>>) -> Self {
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
        let selected_actors = self.state.selected_actor_ids().await;

        let Some(bus) = &self.bus else {
            return Ok(json!({
                "status": if is_ready { "ready" } else { "booting" },
                "uptime_seconds": uptime_secs,
                "actors": [],
                "selected_actors": selected_actors,
                "test_mode_pending": self.state.test_mode_pending(),
            }));
        };

        // Query supervisor for actor health
        let _ = bus
            .broadcast(bus::Event::System(bus::SystemEvent::HealthCheckRequest {
                target: None,
            }))
            .await;

        // Wait for HealthCheckResponse (with 1s timeout)
        let health_future = async {
            let mut rx = bus.subscribe_broadcast();
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
                "selected_actors": selected_actors,
                "test_mode_pending": self.state.test_mode_pending(),
            })),
            Ok(None) | Err(_) => {
                // Timeout or channel error — return basic status without actor details
                Ok(json!({
                    "status": if is_ready { "ready" } else { "booting" },
                    "uptime_seconds": uptime_secs,
                    "actors": [],
                    "selected_actors": selected_actors,
                    "test_mode_pending": self.state.test_mode_pending(),
                }))
            }
        }
    }
}

/// Handler for "runtime.shutdown" command.
pub struct ShutdownHandler {
    control_tx: tokio::sync::mpsc::UnboundedSender<DaemonControlMessage>,
    bus: Option<std::sync::Arc<bus::EventBus>>,
}

impl ShutdownHandler {
    pub fn new(
        control_tx: tokio::sync::mpsc::UnboundedSender<DaemonControlMessage>,
        bus: Option<std::sync::Arc<bus::EventBus>>,
    ) -> Self {
        Self { control_tx, bus }
    }
}

pub struct TestModeStatusHandler {
    state: RuntimeState,
}

impl TestModeStatusHandler {
    pub fn new(state: RuntimeState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl CommandHandler for TestModeStatusHandler {
    fn name(&self) -> &'static str {
        "runtime.test_mode_status"
    }

    fn description(&self) -> &'static str {
        "Report whether test-mode actor selection is pending"
    }

    fn requires_boot(&self) -> bool {
        false
    }

    async fn handle(&self, _payload: Value) -> Result<Value, IpcError> {
        let selected_actors = self.state.selected_actor_ids().await;
        let actors = runtime::actor_specs()
            .iter()
            .map(|actor| {
                json!({
                    "id": actor.id,
                    "display_name": actor.display_name,
                    "description": actor.description,
                    "dependencies": actor.dependencies,
                    "can_start_without_dependencies": actor.can_start_without_dependencies,
                })
            })
            .collect::<Vec<_>>();

        Ok(json!({
            "pending": self.state.test_mode_pending(),
            "actors": actors,
            "selected_actors": selected_actors,
        }))
    }
}

pub struct BootWithSelectionHandler {
    state: RuntimeState,
}

impl BootWithSelectionHandler {
    pub fn new(state: RuntimeState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl CommandHandler for BootWithSelectionHandler {
    fn name(&self) -> &'static str {
        "runtime.boot_with_selection"
    }

    fn description(&self) -> &'static str {
        "Submit test-mode actor selection and continue runtime boot"
    }

    fn requires_boot(&self) -> bool {
        false
    }

    async fn handle(&self, payload: Value) -> Result<Value, IpcError> {
        let actor_ids = payload
            .get("actors")
            .and_then(|value| value.as_array())
            .ok_or_else(|| IpcError::InvalidPayload("missing 'actors' array".to_string()))?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| IpcError::InvalidPayload("actor ids must be strings".to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;

        let selection = runtime::ActorSelection::try_from_ids(actor_ids.iter())
            .map_err(|error| IpcError::InvalidPayload(error.to_string()))?;
        selection
            .validate()
            .map_err(|error| IpcError::InvalidPayload(error.to_string()))?;

        let selected_ids = selection.selected_ids();
        self.state.submit_boot_selection(selection).await?;

        Ok(json!({
            "accepted": true,
            "actors": selected_ids,
        }))
    }
}

pub struct TestModeRestartHandler {
    control_tx: tokio::sync::mpsc::UnboundedSender<DaemonControlMessage>,
}

impl TestModeRestartHandler {
    pub fn new(control_tx: tokio::sync::mpsc::UnboundedSender<DaemonControlMessage>) -> Self {
        Self { control_tx }
    }
}

#[async_trait]
impl CommandHandler for TestModeRestartHandler {
    fn name(&self) -> &'static str {
        "runtime.test_mode_restart"
    }

    fn description(&self) -> &'static str {
        "Restart the daemon and re-enter test mode actor selection"
    }

    async fn handle(&self, _payload: Value) -> Result<Value, IpcError> {
        self.control_tx
            .send(DaemonControlMessage::RestartInTestMode)
            .map_err(|_| IpcError::Internal("control channel closed".to_string()))?;

        Ok(json!({
            "restart_requested": true,
            "mode": "test_mode"
        }))
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
        if let Some(bus) = &self.bus {
            let _ = bus
                .broadcast(bus::Event::System(bus::SystemEvent::ShutdownRequested))
                .await;
        }

        // Send to private shutdown channel to trigger daemon shutdown
        self.control_tx
            .send(DaemonControlMessage::Shutdown)
            .map_err(|_| IpcError::Internal("control channel closed".to_string()))?;

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
