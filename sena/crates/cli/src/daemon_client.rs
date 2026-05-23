use crate::error::CliError;
use ipc::IpcClient;
#[cfg(target_os = "windows")]
use std::process::Command;
use tokio::time::{Duration, sleep};
use tracing::{info, warn};

pub async fn ensure_daemon_running() -> Result<(), CliError> {
    if IpcClient::daemon_running().await {
        info!("Daemon already running");
        return Ok(());
    }

    info!("Daemon not running, auto-starting...");
    start_daemon(false)?;

    for attempt in 1..=50 {
        sleep(Duration::from_millis(200)).await;
        if IpcClient::daemon_running().await {
            info!("Daemon ready after {} attempts", attempt);
            return Ok(());
        }
    }

    Err(CliError::DaemonStartTimeout)
}

#[cfg(target_os = "windows")]
pub fn start_daemon(test_mode: bool) -> Result<(), CliError> {
    use std::os::windows::process::CommandExt;

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

    let mut command = Command::new(daemon_exe);
    if test_mode {
        command.arg("--test-mode");
    }

    command
        .creation_flags(0x00000008)
        .spawn()
        .map_err(|e| CliError::DaemonStartFailed(e.to_string()))?;

    info!("Daemon process spawned");
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn start_daemon(_test_mode: bool) -> Result<(), CliError> {
    Err(CliError::PlatformNotSupported)
}

pub async fn connect_to_daemon() -> Result<IpcClient, CliError> {
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

pub async fn wait_for_runtime_ready(ipc_client: &mut IpcClient) -> Result<(), CliError> {
    for _ in 0..300 {
        match ipc_client
            .send("runtime.status", serde_json::json!({}))
            .await
        {
            Ok(response)
                if response
                    .get("status")
                    .and_then(|value| value.as_str())
                    == Some("ready") =>
            {
                return Ok(());
            }
            Ok(_) | Err(_) => {
                sleep(Duration::from_millis(200)).await;
            }
        }
    }

    Err(CliError::DaemonStartTimeout)
}