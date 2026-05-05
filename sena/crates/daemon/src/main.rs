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
    pub mod transparency_commands;
}
mod error;
mod tray;

use commands::runtime_commands::RuntimeState;
use error::DaemonError;
use ipc::{CommandRegistry, IpcServer};
use std::sync::mpsc;
use tracing::{error, info, warn};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

#[tokio::main]
async fn main() -> Result<(), DaemonError> {
    // Initialize logging
    init_logging()?;

    info!("Sena daemon starting");

    // Boot runtime
    info!("Booting runtime...");
    let boot_result = runtime::boot()
        .await
        .map_err(|e| DaemonError::BootFailed(e.to_string()))?;

    info!("Runtime boot complete, starting supervision and IPC server");

    // Create runtime state for command handlers
    let runtime_state = RuntimeState::new();

    // Create shutdown channel
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::mpsc::unbounded_channel();

    // Create command registry and register all handlers
    let mut registry = CommandRegistry::new();
    let loop_registry = commands::handlers::register_all(
        &mut registry,
        &boot_result,
        runtime_state.clone(),
        shutdown_tx.clone(),
    );

    info!("Registered {} IPC commands", registry.list().len());

    // Start IPC server
    let (ipc_server, push_tx) = IpcServer::new(registry);
    let ipc_handle = tokio::spawn(async move {
        if let Err(e) = ipc_server.run().await {
            error!("IPC server error: {}", e);
        }
    });

    info!("IPC server started");

    // Spawn event forwarding task — forwards bus events to IPC clients
    let event_forwarding_bus = boot_result.bus.clone();
    tokio::spawn(async move {
        forward_bus_events_to_ipc(event_forwarding_bus, push_tx).await;
    });

    // Spawn loop status tracking task — updates loop registry when actors report status changes
    let loop_status_bus = boot_result.bus.clone();
    let loop_registry_clone = loop_registry.clone();
    tokio::spawn(async move {
        track_loop_status_changes(loop_status_bus, loop_registry_clone).await;
    });

    // Create tray action channel (std::sync::mpsc for main thread)
    let (tray_action_tx, tray_action_rx) = mpsc::channel();

    // Create tooltip update channel (std::sync::mpsc for cross-thread send from tokio)
    let (tooltip_tx, tooltip_rx) = mpsc::channel();

    // Clone bus references before moving boot_result into supervision loop
    let boot_complete_bus = boot_result.bus.clone();
    let supervision_bus = boot_result.bus.clone();

    // Spawn supervision loop in background
    let mut supervision_handle = tokio::spawn(async move {
        if let Err(e) = runtime::supervision_loop(boot_result).await {
            error!("Supervision loop error: {}", e);
        }
    });

    // Update tray tooltip after boot
    tooltip_tx
        .send(tray::TooltipUpdate {
            text: "Sena — Booting...".to_string(),
        })
        .ok();

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

    // Spawn tray action handler task
    let action_handler_shutdown_tx = shutdown_tx.clone();
    let tray_action_handle = std::thread::spawn(move || {
        handle_tray_actions(tray_action_rx, action_handler_shutdown_tx);
    });

    info!("Daemon ready, entering tray loop");

    // Run tray loop on main thread (blocking)
    // This is required on Windows for proper message pump handling
    let tray_result = tray::run_tray_loop(tooltip_rx, tray_action_tx);

    // Tray loop exited — initiate shutdown
    match tray_result {
        tray::TrayLoopResult::Shutdown => {
            info!("Tray loop requested shutdown");
            // Unblock the select! below so shutdown proceeds immediately.
            let _ = shutdown_tx.send(());
        }
        tray::TrayLoopResult::Error(e) => {
            warn!("Tray loop error: {}", e);
        }
    }

    // Wait for shutdown signal or supervision loop exit
    let shutdown_requested = tokio::select! {
        _ = shutdown_rx.recv() => {
            info!("Shutdown signal received");
            true
        }
        result = &mut supervision_handle => {
            if let Err(e) = result {
                error!("Supervision loop join error: {}", e);
            } else {
                info!("Supervision loop exited");
            }
            false
        }
    };

    if shutdown_requested {
        let _ = supervision_bus
            .broadcast(bus::Event::System(bus::SystemEvent::ShutdownRequested))
            .await;

        if let Err(e) = supervision_handle.await {
            error!("Supervision loop join error: {}", e);
        }
    }

    // Join tray action handler
    tray_action_handle.join().ok();

    // Abort IPC server (it will exit when pipe closes)
    ipc_handle.abort();

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
                        "task": snapshot
                            .inferred_task
                            .as_ref()
                            .map(|t| t.semantic_description.clone()),
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
            let _ = push_tx.send(payload);
        }
    }

    warn!("Event forwarding task exited");
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
    shutdown_tx: tokio::sync::mpsc::UnboundedSender<()>,
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
                shutdown_tx.send(()).ok();
                break;
            }
        }
    }
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
