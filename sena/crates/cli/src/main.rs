//! Sena CLI binary entrypoint — pure IPC client for daemon communication.
//!
//! The CLI is a thin wrapper over the daemon's capabilities. It never boots
//! the runtime in-process. Instead, it:
//! 1. Checks if daemon is running
//! 2. Auto-starts daemon if needed
//! 3. Connects to daemon via IPC
//! 4. Runs the TUI shell with IPC connection

mod config_editor;
mod error;
mod shell;

use error::CliError;
use ipc::IpcClient;
use shell::Shell;
#[cfg(target_os = "windows")]
use std::process::Command;
use tokio::time::{Duration, sleep};
use tracing::{error, info, warn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Route INFO-level (and above) logs to stdout by default.
    // RUST_LOG overrides the level when set.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .with_writer(std::io::stdout)
        .init();

    info!("Sena CLI starting");

    // Ensure daemon is running
    ensure_daemon_running().await?;

    // Connect to daemon
    let ipc_client = connect_to_daemon().await?;

    // Run shell
    let shell = Shell::new(ipc_client).await?;
    if let Err(e) = shell.run().await {
        error!("Shell error: {}", e);
        return Err(anyhow::anyhow!("Shell failed: {}", e));
    }

    info!("Sena CLI exiting");
    Ok(())
}

/// Ensure daemon is running, auto-starting if necessary.
async fn ensure_daemon_running() -> Result<(), CliError> {
    if IpcClient::daemon_running().await {
        info!("Daemon already running");
        return Ok(());
    }

    info!("Daemon not running, auto-starting...");
    start_daemon()?;

    // Wait for daemon to become ready (max 30 seconds)
    for attempt in 1..=60 {
        sleep(Duration::from_millis(500)).await;
        if IpcClient::daemon_running().await {
            info!("Daemon ready after {} attempts", attempt);
            return Ok(());
        }
    }

    Err(CliError::DaemonStartTimeout)
}

/// Start the daemon as a background process.
#[cfg(target_os = "windows")]
fn start_daemon() -> Result<(), CliError> {
    // Find daemon binary relative to CLI binary
    let cli_exe =
        std::env::current_exe().map_err(|e| CliError::DaemonStartFailed(e.to_string()))?;
    let cli_dir = cli_exe
        .parent()
        .ok_or_else(|| CliError::DaemonStartFailed("cannot determine CLI directory".to_string()))?;
    let daemon_exe = cli_dir.join("sena.exe");

    if !daemon_exe.exists() {
        return Err(CliError::DaemonStartFailed(format!(
            "daemon binary not found at {}",
            daemon_exe.display()
        )));
    }

    // Spawn daemon in detached mode
    Command::new(daemon_exe)
        .spawn()
        .map_err(|e| CliError::DaemonStartFailed(e.to_string()))?;

    info!("Daemon process spawned");
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn start_daemon() -> Result<(), CliError> {
    Err(CliError::PlatformNotSupported)
}

/// Connect to daemon with retries.
async fn connect_to_daemon() -> Result<IpcClient, CliError> {
    for attempt in 1..=5 {
        match IpcClient::connect().await {
            Ok(client) => {
                info!("Connected to daemon on attempt {}", attempt);
                return Ok(client);
            }
            Err(e) if attempt < 5 => {
                warn!("Connection attempt {} failed: {}, retrying...", attempt, e);
                sleep(Duration::from_millis(500)).await;
            }
            Err(e) => {
                return Err(CliError::IpcConnectionFailed(e.to_string()));
            }
        }
    }

    Err(CliError::IpcConnectionFailed(
        "exhausted retries".to_string(),
    ))
}
