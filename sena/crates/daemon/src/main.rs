//! Sena daemon process — owns all actors, runs IPC server, provides system tray.

mod commands {
    pub mod config_commands;
    pub mod events_commands;
    pub mod handlers;
    pub mod inference_commands;
    pub mod memory_commands;
    pub mod runtime_commands;
    pub mod speech_commands;
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
    commands::handlers::register_all(
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

    // Create tray action channel (std::sync::mpsc for main thread)
    let (tray_action_tx, tray_action_rx) = mpsc::channel();

    // Create tooltip update channel (std::sync::mpsc for cross-thread send from tokio)
    let (tooltip_tx, tooltip_rx) = mpsc::channel();

    // Clone bus references before moving boot_result into supervision loop
    let boot_complete_bus = boot_result.bus.clone();
    let supervision_bus = boot_result.bus.clone();

    // Spawn supervision loop in background
    let supervision_handle = tokio::spawn(async move {
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
                        text: "Sena — Ready".to_string(),
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
        }
        tray::TrayLoopResult::Error(e) => {
            warn!("Tray loop error: {}", e);
        }
    }

    // Wait for shutdown signal or supervision loop exit
    tokio::select! {
        _ = shutdown_rx.recv() => {
            info!("Shutdown signal received");
        }
        _ = supervision_handle => {
            info!("Supervision loop exited");
        }
    }

    // Broadcast shutdown event
    let _ = supervision_bus
        .broadcast(bus::Event::System(bus::SystemEvent::ShutdownInitiated))
        .await;

    // Wait briefly for actors to shut down gracefully
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

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
            // Download lifecycle events
            Event::Download(bus::DownloadEvent::Started {
                model_name,
                total_bytes,
                request_id,
            }) => Some(json!({
                "type": "download_started",
                "model_name": model_name,
                "total_bytes": total_bytes,
                "request_id": request_id,
            })),
            Event::Download(bus::DownloadEvent::Progress {
                model_name,
                bytes_downloaded,
                total_bytes,
                request_id,
            }) => Some(json!({
                "type": "download_progress",
                "model_name": model_name,
                "bytes_downloaded": bytes_downloaded,
                "total_bytes": total_bytes,
                "percent": if total_bytes > 0 { (bytes_downloaded as f64 / total_bytes as f64 * 100.0) as u8 } else { 0 },
                "request_id": request_id,
            })),
            Event::Download(bus::DownloadEvent::Completed {
                model_name,
                cached_path,
                request_id,
            }) => Some(json!({
                "type": "download_completed",
                "model_name": model_name,
                "cached_path": cached_path,
                "request_id": request_id,
            })),
            Event::Download(bus::DownloadEvent::Failed {
                model_name,
                reason,
                request_id,
            }) => Some(json!({
                "type": "download_failed",
                "model_name": model_name,
                "reason": reason,
                "request_id": request_id,
            })),
            // Onboarding events
            Event::System(bus::SystemEvent::OnboardingRequired) => Some(json!({
                "type": "onboarding_required",
            })),
            Event::System(bus::SystemEvent::OnboardingCompleted) => Some(json!({
                "type": "onboarding_completed",
            })),
            // Boot failed event
            Event::System(bus::SystemEvent::BootFailed { reason }) => Some(json!({
                "type": "boot_failed",
                "reason": reason,
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
                if let Err(e) = launch_cli() {
                    error!("Failed to launch CLI: {}", e);
                }
            }
            tray::TrayAction::ConfigEditor => {
                info!("Tray action: Config Editor");
                warn!("Config editor not yet implemented");
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
fn launch_cli() -> Result<(), DaemonError> {
    use std::process::Command;

    // Get path to current executable
    let exe_path = std::env::current_exe().map_err(|e| {
        DaemonError::CliLaunchFailed(format!("failed to get current exe path: {}", e))
    })?;
    let exe_dir = exe_path.parent().ok_or_else(|| {
        DaemonError::CliLaunchFailed("no parent directory for executable".to_string())
    })?;

    // Look for CLI binary (Phase 4+: CLI is separate binary named "sena-cli")
    let cli_path = exe_dir.join("sena-cli.exe");

    if !cli_path.exists() {
        return Err(DaemonError::CliLaunchFailed(format!(
            "CLI binary not found at {:?}",
            cli_path
        )));
    }

    // Convert path to string without unwrap — gracefully handle non-UTF8 paths
    let cli_path_str = cli_path.to_str().ok_or_else(|| {
        DaemonError::CliLaunchFailed("CLI path contains invalid UTF-8".to_string())
    })?;

    // Launch in new console window
    Command::new("cmd")
        .args(["/c", "start", "cmd", "/k", cli_path_str])
        .spawn()
        .map_err(|e| DaemonError::CliLaunchFailed(format!("failed to spawn CLI process: {}", e)))?;

    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn launch_cli() -> Result<(), DaemonError> {
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
