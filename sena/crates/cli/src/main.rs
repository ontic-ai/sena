//! Sena CLI binary entrypoint — pure IPC client for daemon communication.
//!
//! The CLI is a thin wrapper over the daemon's capabilities. It never boots
//! the runtime in-process. Instead, it:
//! 1. Checks if daemon is running
//! 2. Auto-starts daemon if needed
//! 3. Connects to daemon via IPC
//! 4. Runs the TUI shell with IPC connection

mod commands;
mod daemon_client;
mod config_editor;
mod error;
mod logging;
mod onboarding;
mod shell;
mod terminal_window;
mod test_mode;
mod theme;
mod transparency_format;

use daemon_client::{connect_to_daemon, ensure_daemon_running, wait_for_runtime_ready};
use error::CliError;
use ipc::IpcClient;
use shell::Shell;
use tracing::{debug, error, info};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let config_mode = args.iter().any(|arg| arg == "--config");

    let log_path = logging::init_tracing()?;

    info!(log_path = %log_path.display(), "Sena CLI starting");
    debug!(config_mode, "CLI arguments parsed");

    // Ensure daemon is running
    ensure_daemon_running().await?;

    // Connect to daemon
    let mut ipc_client = connect_to_daemon().await?;

    test_mode::complete_pending_selection(&mut ipc_client).await?;
    wait_for_runtime_ready(&mut ipc_client).await?;

    let onboarding_required = check_onboarding_status(&mut ipc_client).await?;

    if onboarding_required {
        info!("Onboarding required — running wizard");
        if let Err(e) = onboarding::run_wizard(&mut ipc_client).await {
            error!("Onboarding wizard error: {}", e);
            return Err(anyhow::anyhow!("Onboarding failed: {}", e));
        }
    }

    if config_mode {
        let mut ipc_client = ipc_client;
        let mut editor = crate::config_editor::ConfigEditor::new(&mut ipc_client);
        if let Err(e) = editor.run().await {
            error!("Config editor error: {}", e);
            return Err(anyhow::anyhow!("Config editor failed: {}", e));
        }
    } else {
        // Run shell
        let shell = Shell::new(ipc_client).await?;
        if let Err(e) = shell.run().await {
            error!("Shell error: {}", e);
            return Err(anyhow::anyhow!("Shell failed: {}", e));
        }
    }

    info!("Sena CLI exiting");
    Ok(())
}

/// Check whether onboarding is required through the daemon-owned runtime state.
async fn check_onboarding_status(ipc_client: &mut IpcClient) -> Result<bool, CliError> {
    let response = ipc_client
        .send("runtime.onboarding_status", serde_json::json!({}))
        .await?;

    Ok(response
        .get("onboarding_required")
        .and_then(|value| value.as_bool())
        .unwrap_or(false))
}
