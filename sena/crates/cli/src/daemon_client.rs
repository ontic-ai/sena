use crate::error::CliError;
use crate::tabs::CliTabKind;
use ipc::IpcClient;
#[cfg(target_os = "windows")]
use std::process::Command;
use tokio::time::{Duration, sleep};
use tracing::{debug, info, warn};

pub const DAEMON_CONNECT_ERROR_MESSAGE: &str = "Could not connect to Sena. Is the daemon running?";

const PIPE_RETRY_DELAYS: [Duration; 5] = [
    Duration::from_millis(200),
    Duration::from_millis(400),
    Duration::from_millis(800),
    Duration::from_millis(1600),
    Duration::from_millis(3200),
];

const CONNECT_RETRY_DELAYS: [Duration; 6] = [
    Duration::from_millis(0),
    Duration::from_millis(200),
    Duration::from_millis(400),
    Duration::from_millis(800),
    Duration::from_millis(1600),
    Duration::from_millis(3200),
];

pub async fn ensure_daemon_running() -> Result<(), CliError> {
    if IpcClient::daemon_running().await {
        info!("Daemon already running");
        return Ok(());
    }

    info!("Daemon not running, auto-starting...");
    start_daemon()?;

    if IpcClient::daemon_running().await {
        info!("Daemon pipe became ready immediately after spawn");
        return Ok(());
    }

    for (attempt, delay) in PIPE_RETRY_DELAYS.iter().enumerate() {
        sleep(*delay).await;
        if IpcClient::daemon_running().await {
            info!(attempt = attempt + 2, ?delay, "Daemon pipe is ready");
            return Ok(());
        }
    }

    Err(CliError::DaemonStartTimeout)
}

#[cfg(target_os = "windows")]
pub fn start_daemon() -> Result<(), CliError> {
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

    Command::new(daemon_exe)
        .creation_flags(0x00000008)
        .spawn()
        .map_err(|e| CliError::DaemonStartFailed(e.to_string()))?;

    info!("Daemon process spawned");
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn start_daemon() -> Result<(), CliError> {
    Err(CliError::PlatformNotSupported)
}

pub async fn connect_to_daemon() -> Result<IpcClient, CliError> {
    for (attempt, delay) in CONNECT_RETRY_DELAYS.iter().enumerate() {
        if !delay.is_zero() {
            sleep(*delay).await;
        }

        match IpcClient::connect().await {
            Ok(client) => {
                info!(attempt = attempt + 1, "Connected to daemon");
                return Ok(client);
            }
            Err(e) if attempt + 1 < CONNECT_RETRY_DELAYS.len() => {
                warn!(attempt = attempt + 1, error = %e, ?delay, "Daemon connection attempt failed");
            }
            Err(e) => {
                warn!(attempt = attempt + 1, error = %e, "Daemon connection attempts exhausted");
                return Err(CliError::IpcConnectionFailed(
                    DAEMON_CONNECT_ERROR_MESSAGE.to_string(),
                ));
            }
        }
    }

    Err(CliError::IpcConnectionFailed(
        DAEMON_CONNECT_ERROR_MESSAGE.to_string(),
    ))
}

pub async fn wait_for_daemon_exit() -> Result<(), CliError> {
    for _ in 0..100 {
        if !IpcClient::daemon_running().await {
            return Ok(());
        }

        sleep(Duration::from_millis(100)).await;
    }

    Err(CliError::DaemonStartTimeout)
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

#[cfg(target_os = "windows")]
pub(crate) fn launch_cli_tab(tab: CliTabKind) -> Result<(), CliError> {
    let cli_exe =
        std::env::current_exe().map_err(|e| CliError::DaemonStartFailed(e.to_string()))?;
    let cli_path = cli_exe
        .to_str()
        .ok_or_else(|| CliError::DaemonStartFailed("CLI path contains invalid UTF-8".to_string()))?;

    if let Some(windows_terminal) = find_windows_terminal() {
        Command::new(&windows_terminal)
            .args(windows_terminal_args(cli_path, tab))
            .spawn()
            .map_err(|e| CliError::DaemonStartFailed(e.to_string()))?;
        debug!(launcher = "wt.exe", tab = tab.as_arg(), path = %windows_terminal.display(), "Opened CLI tab in Windows Terminal");
        return Ok(());
    }

    Command::new("cmd")
        .args(conhost_args(cli_path, tab))
        .spawn()
        .map_err(|e| CliError::DaemonStartFailed(e.to_string()))?;
    debug!(launcher = "conhost", tab = tab.as_arg(), "Opened CLI tab in conhost fallback window");
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn launch_cli_tab(_tab: CliTabKind) -> Result<(), CliError> {
    Err(CliError::PlatformNotSupported)
}

#[cfg(target_os = "windows")]
fn find_windows_terminal() -> Option<std::path::PathBuf> {
    find_on_path("wt.exe").or_else(|| {
        std::env::var_os("LOCALAPPDATA")
            .map(std::path::PathBuf::from)
            .map(|path| path.join("Microsoft").join("WindowsApps").join("wt.exe"))
            .filter(|path| path.exists())
    })
}

#[cfg(target_os = "windows")]
fn find_on_path(file_name: &str) -> Option<std::path::PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var)
        .map(|directory| directory.join(file_name))
        .find(|candidate| candidate.exists())
}

#[cfg(target_os = "windows")]
fn windows_terminal_args(cli_path: &str, tab: CliTabKind) -> Vec<String> {
    ["-w", "new", "--size", "220,55", "--", cli_path, "--tab", tab.as_arg()]
        .into_iter()
        .map(str::to_string)
        .collect()
}

#[cfg(target_os = "windows")]
fn conhost_args(cli_path: &str, tab: CliTabKind) -> Vec<String> {
    ["/c", "start", "", cli_path, "--tab", tab.as_arg()]
        .into_iter()
        .map(str::to_string)
        .collect()
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::{CliTabKind, conhost_args, windows_terminal_args};

    #[test]
    fn windows_terminal_args_include_requested_tab_and_size() {
        let args = windows_terminal_args("C:/sena/sena-cli.exe", CliTabKind::Diag);

        assert_eq!(
            args,
            vec![
                "-w",
                "new",
                "--size",
                "220,55",
                "--",
                "C:/sena/sena-cli.exe",
                "--tab",
                "diag",
            ]
        );
    }

    #[test]
    fn conhost_args_include_requested_tab() {
        let args = conhost_args("C:/sena/sena-cli.exe", CliTabKind::Resources);

        assert_eq!(
            args,
            vec![
                "/c",
                "start",
                "",
                "C:/sena/sena-cli.exe",
                "--tab",
                "resources",
            ]
        );
    }
}