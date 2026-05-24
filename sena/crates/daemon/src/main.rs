//! Sena daemon process — owns all actors, runs IPC server, provides system tray.

mod commands {
    pub mod config_commands;
    pub mod events_commands;
    pub mod handlers;
    pub mod inference_commands;
    pub mod loops_commands;
    pub mod memory_commands;
    pub mod runtime_commands;
    pub mod speech_commands;
    pub mod sri_commands;
    pub mod transparency_commands;
}
mod error;
mod tray;

use commands::runtime_commands::RuntimeState;
use commands::runtime_commands::DaemonControlMessage;
use error::DaemonError;
use ipc::{CommandRegistry, IpcServer};
use sri::SriActor;
use std::path::PathBuf;
use std::sync::mpsc;
use tokio::sync::oneshot;
use tracing::{error, info, warn};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

fn inference_source_label(source: bus::InferenceSource) -> &'static str {
    match source {
        bus::InferenceSource::UserVoice => "user_voice",
        bus::InferenceSource::UserText => "user_text",
        bus::InferenceSource::ProactiveCTP => "proactive_ctp",
        bus::InferenceSource::Iterative => "iterative",
    }
}

fn main() -> Result<(), DaemonError> {
    // Initialize logging
    init_logging()?;

    info!("Sena daemon starting");
    let test_mode_requested = std::env::args().any(|arg| arg == "--test-mode");

    // Create shared shutdown channel up front so the tray is available while the
    // runtime boots on a background worker.
    let (control_tx, control_rx) = tokio::sync::mpsc::unbounded_channel();

    // Create tray channels for the main-thread loop.
    let (tray_action_tx, tray_action_rx) = mpsc::channel();
    let (tooltip_tx, tooltip_rx) = mpsc::channel();
    let (tray_shutdown_tx, tray_shutdown_rx) = mpsc::channel();

    tooltip_tx
        .send(tray::TooltipUpdate {
            text: "Sena — Booting...".to_string(),
        })
        .ok();

    // Spawn tray action handler task.
    let action_handler_shutdown_tx = control_tx.clone();
    let tray_action_handle = std::thread::spawn(move || {
        handle_tray_actions(tray_action_rx, action_handler_shutdown_tx);
    });

    let daemon_tooltip_tx = tooltip_tx.clone();
    let daemon_control_tx = control_tx.clone();
    let daemon_tray_shutdown_tx = tray_shutdown_tx.clone();
    let daemon_thread = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| {
                DaemonError::SupervisionError(format!("failed to build daemon runtime: {}", e))
            })?;

        runtime.block_on(run_daemon_services(
            daemon_tooltip_tx,
            control_rx,
            daemon_control_tx,
            daemon_tray_shutdown_tx,
            test_mode_requested,
        ))
    });

    info!("Tray initialized, entering tray loop");

    // Run tray loop on main thread (blocking)
    // This is required on Windows for proper message pump handling
    let tray_result = tray::run_tray_loop(tooltip_rx, tray_action_tx, tray_shutdown_rx);

    let tray_error = match tray_result {
        tray::TrayLoopResult::Shutdown => {
            info!("Tray loop requested shutdown");
            let _ = control_tx.send(DaemonControlMessage::Shutdown);
            None
        }
        tray::TrayLoopResult::Error(e) => {
            warn!("Tray loop error: {}", e);
            let _ = control_tx.send(DaemonControlMessage::Shutdown);
            Some(e)
        }
    };

    drop(control_tx);
    drop(tooltip_tx);
    drop(tray_shutdown_tx);

    tray_action_handle.join().ok();

    let daemon_result = match daemon_thread.join() {
        Ok(result) => result,
        Err(_) => {
            return Err(DaemonError::SupervisionError(
                "daemon runtime worker thread panicked".to_string(),
            ));
        }
    };

    daemon_result?;

    if let Some(error) = tray_error {
        return Err(DaemonError::TrayError(error));
    }

    Ok(())
}

async fn run_daemon_services(
    tooltip_tx: mpsc::Sender<tray::TooltipUpdate>,
    mut control_rx: tokio::sync::mpsc::UnboundedReceiver<DaemonControlMessage>,
    control_tx: tokio::sync::mpsc::UnboundedSender<DaemonControlMessage>,
    tray_shutdown_tx: mpsc::Sender<()>,
    test_mode_requested: bool,
) -> Result<(), DaemonError> {
    // Create runtime state for command handlers
    let runtime_state = RuntimeState::new();
    let inference_diagnostics: commands::inference_commands::InferenceDiagnosticsState =
        std::sync::Arc::new(tokio::sync::Mutex::new(None));

    // Start IPC server before runtime boot so test mode can submit a selection.
    let mut preboot_registry = CommandRegistry::new();
    commands::handlers::register_preboot(&mut preboot_registry, runtime_state.clone(), control_tx.clone());
    let (ipc_server, push_tx) = IpcServer::new(preboot_registry);
    let ipc_server_handle = ipc_server.clone();
    let ipc_handle = tokio::spawn(async move {
        if let Err(e) = ipc_server.run().await {
            error!("IPC server error: {}", e);
        }
    });

    info!("IPC server started");

    if test_mode_requested {
        runtime_state.set_test_mode_pending(true);
        create_test_mode_marker().await?;
        tooltip_tx
            .send(tray::TooltipUpdate {
                text: "Sena — Waiting for test mode selection".to_string(),
            })
            .ok();
    }

    let explicit_selection = if test_mode_requested {
        Some(wait_for_test_mode_selection(&runtime_state).await?)
    } else {
        None
    };

    clear_test_mode_marker().await.ok();

    info!("Booting runtime...");
    let boot_result = match explicit_selection.as_ref() {
        Some(selection) => runtime::boot_with_selection(selection).await,
        None => runtime::boot().await,
    };
    let mut boot_result = match boot_result {
        Ok(boot_result) => boot_result,
        Err(e) => {
            tooltip_tx
                .send(tray::TooltipUpdate {
                    text: "Sena — Boot failed".to_string(),
                })
                .ok();
            let _ = tray_shutdown_tx.send(());
            return Err(DaemonError::BootFailed(e.to_string()));
        }
    };

    let sri_enabled = explicit_selection
        .as_ref()
        .map(|selection| selection.contains("sri"))
        .unwrap_or(true);
    let (sri_state, sri_events) = if sri_enabled {
        let sri_actor = SriActor::new(sri::SriRegistry::new());
        let sri_state = sri_actor.state();
        let sri_events = sri_actor.event_sender();
        sri_actor.start(boot_result.bus.clone());
        boot_result.expected_actors.push("sri");
        boot_result.selected_actors.insert("sri");
        let _ = boot_result
            .bus
            .broadcast(bus::Event::System(bus::SystemEvent::ActorReady {
                actor_name: "sri".to_string(),
            }))
            .await;
        (Some(sri_state), Some(sri_events))
    } else {
        (None, None)
    };

    runtime_state
        .set_selected_actors(boot_result.selected_actors.iter().copied())
        .await;

    if let Some(selection) = explicit_selection.as_ref() {
        info!(
            actors = ?selection.selected_ids(),
            "test mode: running with actors"
        );
        info!(
            actors = ?selection.skipped_ids(),
            "test mode: skipped actors"
        );
    }

    info!("Runtime boot complete, starting supervision and full IPC registry");

    let mut registry = CommandRegistry::new();
    let loop_registry = commands::handlers::register_all(
        &mut registry,
        &boot_result,
        runtime_state.clone(),
        sri_state.clone(),
        inference_diagnostics.clone(),
        control_tx.clone(),
    );
    ipc_server_handle.replace_registry(registry).await;

    // Spawn event forwarding task — forwards bus events to IPC clients
    let event_forwarding_bus = boot_result.bus.clone();
    let push_tx_events = push_tx.clone();
    let diagnostics_state = inference_diagnostics.clone();
    tokio::spawn(async move {
        forward_bus_events_to_ipc(event_forwarding_bus, push_tx_events, diagnostics_state).await;
    });

    if let Some(sri_events) = sri_events {
        let push_tx_sri = push_tx.clone();
        tokio::spawn(async move {
            forward_sri_events_to_ipc(sri_events.subscribe(), push_tx_sri).await;
        });
    }

    // Spawn loop status tracking task — updates loop registry when actors report status changes
    let loop_status_bus = boot_result.bus.clone();
    let loop_registry_clone = loop_registry.clone();
    tokio::spawn(async move {
        track_loop_status_changes(loop_status_bus, loop_registry_clone).await;
    });

    // Clone bus references before moving boot_result into supervision loop
    let boot_complete_bus = boot_result.bus.clone();
    let supervision_bus = boot_result.bus.clone();

    // Spawn supervision loop in background
    let mut supervision_handle = tokio::spawn(async move {
        if let Err(e) = runtime::supervision_loop(boot_result).await {
            error!("Supervision loop error: {}", e);
        }
    });

    // Subscribe to BootComplete event to know when runtime is ready
    let runtime_state_clone = runtime_state.clone();
    let tooltip_tx_clone = tooltip_tx.clone();
    tokio::spawn(async move {
        let mut rx = boot_complete_bus.subscribe_broadcast();
        while let Ok(event) = rx.recv().await {
            if matches!(event, bus::Event::System(bus::SystemEvent::BootComplete)) {
                info!("BootComplete received — runtime is ready");
                runtime_state_clone.mark_ready();
                tooltip_tx_clone
                    .send(tray::TooltipUpdate {
                        text: "Sena — Running".to_string(),
                    })
                    .ok();
                break;
            }
        }
    });

    // Wait for shutdown signal or supervision loop exit
    let control_message = tokio::select! {
        message = control_rx.recv() => {
            if let Some(message) = message {
                info!(?message, "Control message received");
            }
            message
        }
        result = &mut supervision_handle => {
            if let Err(e) = result {
                error!("Supervision loop join error: {}", e);
            } else {
                info!("Supervision loop exited");
            }
            None
        }
    };

    if let Some(_message) = control_message {
        let _ = supervision_bus
            .broadcast(bus::Event::System(bus::SystemEvent::ShutdownRequested))
            .await;

        if let Err(e) = supervision_handle.await {
            error!("Supervision loop join error: {}", e);
        }
    }

    // Abort IPC server (it will exit when pipe closes)
    ipc_handle.abort();
    let _ = tray_shutdown_tx.send(());

    info!("Sena daemon shutdown complete");
    Ok(())
}

/// Forward relevant bus events to IPC clients as push events.
///
/// This task subscribes to the broadcast bus and forwards download lifecycle,
/// onboarding, and boot-failed events to all connected IPC clients.
async fn forward_bus_events_to_ipc(
    bus: std::sync::Arc<bus::EventBus>,
    push_tx: tokio::sync::broadcast::Sender<serde_json::Value>,
    inference_diagnostics: commands::inference_commands::InferenceDiagnosticsState,
) {
    use bus::Event;
    use serde_json::json;

    let mut rx = bus.subscribe_broadcast();

    info!("Event forwarding task started");

    while let Ok(event) = rx.recv().await {
        let push_event = match event {
            Event::Speech(bus::SpeechEvent::TranscriptionCompleted {
                text, confidence, ..
            }) => Some(json!({
                "type": "TranscriptionCompleted",
                "data": {
                    "text": text,
                    "confidence": confidence,
                }
            })),
            Event::Speech(bus::SpeechEvent::ListenModeTranscription { text, .. }) => Some(json!({
                "type": "ListenModeTranscription",
                "data": {
                    "text": text,
                }
            })),
            Event::Speech(bus::SpeechEvent::LowConfidenceTranscription {
                text,
                confidence,
                ..
            }) => Some(json!({
                "type": "LowConfidenceTranscription",
                "data": {
                    "text": text,
                    "confidence": confidence,
                }
            })),
            Event::Speech(bus::SpeechEvent::WakewordDetected { confidence }) => Some(json!({
                "type": "WakewordDetected",
                "data": {
                    "confidence": confidence,
                }
            })),
            Event::Speech(bus::SpeechEvent::WakewordSuppressed { reason, .. }) => Some(json!({
                "type": "WakewordSuppressed",
                "data": {
                    "reason": reason,
                }
            })),
            Event::Speech(bus::SpeechEvent::WakewordResumed { .. }) => Some(json!({
                "type": "WakewordResumed",
                "data": {}
            })),
            Event::Speech(bus::SpeechEvent::ListenModeTranscriptFinalized { text, .. }) => {
                Some(json!({
                    "type": "ListenModeTranscriptFinalized",
                    "data": {
                        "text": text,
                    }
                }))
            }
            Event::Speech(bus::SpeechEvent::SpeakingStarted { .. }) => Some(json!({
                "type": "SpeakingStarted",
                "data": {}
            })),
            Event::Speech(bus::SpeechEvent::SpeakingCompleted { .. }) => Some(json!({
                "type": "SpeakingCompleted",
                "data": {}
            })),

            Event::Inference(bus::InferenceEvent::InferenceSentenceReady { text, .. }) => {
                Some(json!({
                    "type": "InferenceSentenceReady",
                    "data": {
                        "text": text,
                    }
                }))
            }
            Event::Inference(bus::InferenceEvent::InferenceStreamCompleted {
                token_count,
                source,
                ..
            }) if !matches!(source, bus::InferenceSource::ProactiveCTP) => Some(json!({
                "type": "InferenceStreamCompleted",
                "data": {
                    "token_count": token_count,
                }
            })),
            Event::Inference(bus::InferenceEvent::InferenceCompleted {
                source,
                token_count,
                causal_id,
                ..
            }) if !matches!(source, bus::InferenceSource::ProactiveCTP) => Some(json!({
                "type": "InferenceCompleted",
                "data": {
                    "token_count": token_count,
                    "causal_id": causal_id.as_u64(),
                }
            })),
            Event::Inference(bus::InferenceEvent::InferenceDiagnosticsReady { snapshot }) => {
                let payload = json!({
                    "type": "InferenceDiagnosticsUpdated",
                    "data": {
                        "prompt": snapshot.prompt,
                        "source": inference_source_label(snapshot.source),
                        "full_text": snapshot.full_text,
                        "generated_token_count": snapshot.generated_token_count,
                        "stop_condition": snapshot.stop_condition,
                        "raw_generated_text": snapshot.raw_generated_text,
                        "max_tokens": snapshot.max_tokens,
                        "temperature": snapshot.temperature,
                        "repeat_penalty": snapshot.repeat_penalty,
                        "top_k": snapshot.top_k,
                        "top_p": snapshot.top_p,
                        "stop_sequences": snapshot.stop_sequences,
                        "causal_id": snapshot.causal_id.as_u64(),
                    }
                });
                *inference_diagnostics.lock().await = payload.get("data").cloned();
                Some(payload)
            }
            Event::Inference(bus::InferenceEvent::ModelLoaded {
                model_path,
                model_name,
                ..
            }) => Some(json!({
                "type": "ModelLoaded",
                "data": {
                    "model_path": model_path,
                    "model_name": model_name,
                }
            })),
            Event::Inference(bus::InferenceEvent::ModelLoadFailed {
                model_path, reason, ..
            }) => Some(json!({
                "type": "ModelLoadFailed",
                "data": {
                    "model_path": model_path,
                    "reason": reason,
                }
            })),

            Event::Memory(bus::MemoryEvent::MemoryWriteCompleted { .. })
            | Event::Memory(bus::MemoryEvent::IngestCompleted { .. }) => Some(json!({
                "type": "MemoryWriteCompleted",
                "data": {}
            })),
            Event::Memory(bus::MemoryEvent::MemoryWriteFailed { reason, .. })
            | Event::Memory(bus::MemoryEvent::IngestFailed { reason, .. }) => Some(json!({
                "type": "MemoryWriteFailed",
                "data": {
                    "reason": reason,
                }
            })),

            Event::System(bus::SystemEvent::ActorFailed { actor, reason }) => Some(json!({
                "type": "ActorFailed",
                "data": {
                    "actor": actor,
                    "reason": reason,
                }
            })),
            Event::System(bus::SystemEvent::BootComplete) => Some(json!({
                "type": "BootComplete",
                "data": {}
            })),
            Event::System(bus::SystemEvent::ConfigUpdated { path }) => Some(json!({
                "type": "ConfigUpdated",
                "data": {
                    "path": path,
                }
            })),
            Event::System(bus::SystemEvent::VramUsageUpdated {
                used_mb,
                total_mb,
                percent,
            }) => Some(json!({
                "type": "VramUsageUpdated",
                "data": {
                    "used_mb": used_mb,
                    "total_mb": total_mb,
                    "percent": percent,
                }
            })),
            Event::System(bus::SystemEvent::OnboardingRequired) => Some(json!({
                "type": "OnboardingRequired",
                "data": {}
            })),
            Event::System(bus::SystemEvent::OnboardingCompleted) => Some(json!({
                "type": "OnboardingCompleted",
                "data": {}
            })),

            Event::CTP(ctp_event) => match ctp_event.as_ref() {
                bus::CTPEvent::ThoughtEventTriggered(snapshot) => Some(json!({
                    "type": "ThoughtEventTriggered",
                    "data": {
                        "app": snapshot.active_app.app_name,
                    }
                })),
                _ => None,
            },

            // Platform stream is preserved for acceptance checks.
            Event::Platform(bus::PlatformEvent::ActiveWindowChanged(ctx)) => Some(json!({
                "type": "PlatformWindowChanged",
                "data": {
                    "app": ctx.app_name,
                    "title": ctx.window_title,
                }
            })),
            Event::Platform(bus::PlatformEvent::ClipboardChanged(digest)) => Some(json!({
                "type": "PlatformClipboardChanged",
                "data": {
                    "char_count": digest.char_count,
                }
            })),
            Event::Platform(bus::PlatformEvent::FileEvent(fe)) => Some(json!({
                "type": "PlatformFileEvent",
                "data": {
                    "path": fe.path.to_string_lossy(),
                    "kind": format!("{:?}", fe.event_kind),
                }
            })),

            _ => None,
        };

        if let Some(payload) = push_event {
            // Ignore send errors — no clients connected is fine
            let _ = push_tx.send(serde_json::json!({
                "stream": "events",
                "type": payload.get("type").cloned().unwrap_or(serde_json::Value::Null),
                "data": payload.get("data").cloned().unwrap_or(serde_json::Value::Null),
            }));
        }
    }

    warn!("Event forwarding task exited");
}

async fn forward_sri_events_to_ipc(
    mut rx: tokio::sync::broadcast::Receiver<sri::SriEvent>,
    push_tx: tokio::sync::broadcast::Sender<serde_json::Value>,
) {
    info!("SRI forwarding task started");

    loop {
        match rx.recv().await {
            Ok(event) => match serde_json::to_value(&event) {
                Ok(event_value) => {
                    let _ = push_tx.send(serde_json::json!({
                        "stream": "sri",
                        "event": event_value,
                    }));
                }
                Err(error) => {
                    error!(error = %error, "failed to serialize SRI event");
                }
            },
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }

    warn!("SRI forwarding task exited");
}

/// Track loop status changes from actors and update the loop registry.
///
/// This task subscribes to LoopStatusChanged events on the bus and updates
/// the loop registry to reflect actual loop states reported by actors.
async fn track_loop_status_changes(
    bus: std::sync::Arc<bus::EventBus>,
    registry: commands::loops_commands::LoopRegistry,
) {
    use bus::Event;

    let mut rx = bus.subscribe_broadcast();

    info!("Loop status tracking task started");

    while let Ok(event) = rx.recv().await {
        if let Event::System(bus::SystemEvent::LoopStatusChanged { loop_name, enabled }) = event {
            info!(
                loop_name = %loop_name,
                enabled = enabled,
                "Loop status changed, updating registry"
            );
            registry.handle_status_changed(&loop_name, enabled).await;
        }
    }

    warn!("Loop status tracking task exited");
}

/// Initialize logging subsystem.
fn init_logging() -> Result<(), DaemonError> {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new("info"))
        .map_err(|e| DaemonError::LoggingFailed(e.to_string()))?;

    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(filter)
        .init();

    Ok(())
}

/// Handle tray action events from the tray loop.
fn handle_tray_actions(
    rx: mpsc::Receiver<tray::TrayAction>,
    shutdown_tx: tokio::sync::mpsc::UnboundedSender<DaemonControlMessage>,
) {
    while let Ok(action) = rx.recv() {
        match action {
            tray::TrayAction::LaunchCli => {
                info!("Tray action: Launch CLI");
                if let Err(e) = launch_cli(false) {
                    error!("Failed to launch CLI: {}", e);
                }
            }
            tray::TrayAction::ConfigEditor => {
                info!("Tray action: Config Editor");
                if let Err(e) = launch_cli(true) {
                    error!("Failed to launch config editor: {}", e);
                }
            }
            tray::TrayAction::OpenModels => {
                info!("Tray action: Open Models Folder");
                if let Err(e) = open_models_folder() {
                    error!("Failed to open models folder: {}", e);
                }
            }
            tray::TrayAction::Shutdown => {
                info!("Tray action: Shutdown");
                shutdown_tx.send(DaemonControlMessage::Shutdown).ok();
                break;
            }
        }
    }
}

fn test_mode_marker_path() -> Result<PathBuf, DaemonError> {
    Ok(runtime::config::config_path()
        .map_err(|e| DaemonError::BootFailed(format!("failed to resolve config path: {}", e)))?
        .parent()
        .ok_or_else(|| DaemonError::BootFailed("config path has no parent".to_string()))?
        .join("test_mode_pending"))
}

async fn create_test_mode_marker() -> Result<(), DaemonError> {
    let marker_path = test_mode_marker_path()?;
    if let Some(parent) = marker_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| DaemonError::BootFailed(format!("failed to create marker dir: {}", e)))?;
    }
    tokio::fs::write(&marker_path, b"pending")
        .await
        .map_err(|e| DaemonError::BootFailed(format!("failed to write test mode marker: {}", e)))
}

async fn clear_test_mode_marker() -> Result<(), DaemonError> {
    let marker_path = test_mode_marker_path()?;
    match tokio::fs::remove_file(marker_path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(DaemonError::BootFailed(format!(
            "failed to clear test mode marker: {}",
            error
        ))),
    }
}

async fn wait_for_test_mode_selection(
    runtime_state: &RuntimeState,
) -> Result<runtime::ActorSelection, DaemonError> {
    let (selection_tx, selection_rx) = oneshot::channel();
    runtime_state.install_boot_selection_sender(selection_tx).await;

    let selection = match tokio::time::timeout(std::time::Duration::from_secs(60), selection_rx).await {
        Ok(Ok(selection)) => selection,
        Ok(Err(_)) => {
            runtime_state.clear_boot_selection_sender().await;
            return Err(DaemonError::BootFailed(
                "test mode selection channel closed before a selection arrived".to_string(),
            ));
        }
        Err(_) => {
            runtime_state.clear_boot_selection_sender().await;
            return Err(DaemonError::BootFailed(
                "timed out waiting for test mode actor selection".to_string(),
            ));
        }
    };

    Ok(selection)
}

/// Launch CLI in a new terminal window.
///
/// # Phase 4 Behavior
///
/// In Phase 4+, daemon and CLI are separate binaries. The daemon binary is `sena.exe`
/// and the CLI binary is `sena-cli.exe`. This function launches the CLI in a new
/// terminal window by spawning `sena-cli.exe`.
#[cfg(target_os = "windows")]
fn launch_cli(config_mode: bool) -> Result<(), DaemonError> {
    use std::process::Command;

    // Get path to current executable
    let exe_path = std::env::current_exe().map_err(|e| {
        DaemonError::CliLaunchFailed(format!("failed to get current exe path: {}", e))
    })?;
    let exe_dir = exe_path.parent().ok_or_else(|| {
        DaemonError::CliLaunchFailed("no parent directory for executable".to_string())
    })?;

    // Look for CLI binary in common dev/runtime layouts.
    let mut candidates = vec![exe_dir.join("sena-cli.exe")];
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("target").join("debug").join("sena-cli.exe"));
        candidates.push(
            cwd.join("sena")
                .join("target")
                .join("debug")
                .join("sena-cli.exe"),
        );
    }

    let cli_path = candidates.into_iter().find(|p| p.exists()).ok_or_else(|| {
        DaemonError::CliLaunchFailed("CLI binary not found in expected locations".to_string())
    })?;

    // Convert path to string without unwrap — gracefully handle non-UTF8 paths
    let cli_path_str = cli_path.to_str().ok_or_else(|| {
        DaemonError::CliLaunchFailed("CLI path contains invalid UTF-8".to_string())
    })?;

    // Launch in new console window via `start` with explicit title argument.
    let mut command = Command::new("cmd");
    command.args(["/c", "start", "", cli_path_str]);
    if config_mode {
        command.arg("--config");
    }
    command
        .spawn()
        .map_err(|e| DaemonError::CliLaunchFailed(format!("failed to spawn CLI process: {}", e)))?;

    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn launch_cli(_config_mode: bool) -> Result<(), DaemonError> {
    Err(DaemonError::CliLaunchFailed(
        "CLI launch not yet implemented on this platform".to_string(),
    ))
}

/// Open models folder in file explorer.
#[cfg(target_os = "windows")]
fn open_models_folder() -> Result<(), DaemonError> {
    use std::process::Command;

    // Get models folder path (using standard AppData location)
    let app_data = std::env::var("APPDATA").map_err(|_| {
        DaemonError::ModelsFolderError("APPDATA environment variable not set".to_string())
    })?;
    let models_path = std::path::Path::new(&app_data).join("sena").join("models");

    // Create directory if it doesn't exist
    std::fs::create_dir_all(&models_path).map_err(|e| {
        DaemonError::ModelsFolderError(format!("failed to create models directory: {}", e))
    })?;

    // Open in Explorer
    Command::new("explorer")
        .arg(models_path)
        .spawn()
        .map_err(|e| {
            DaemonError::ModelsFolderError(format!("failed to open models folder: {}", e))
        })?;

    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn open_models_folder() -> Result<(), DaemonError> {
    Err(DaemonError::ModelsFolderError(
        "Open models folder not yet implemented on this platform".to_string(),
    ))
}
