//! Sena CLI binary entrypoint — pure IPC client for daemon communication.
//!
//! The CLI is a thin wrapper over the daemon's capabilities. It never boots
//! the runtime in-process. Instead, it:
//! 1. Checks if daemon is running
//! 2. Auto-starts daemon if needed
//! 3. Connects to daemon via IPC
//! 4. Runs the TUI shell with IPC connection

use ipc::IpcClient;
use sena_cli::daemon_client::{connect_to_daemon, ensure_daemon_running, wait_for_runtime_ready};
use sena_cli::error::CliError;
use sena_cli::shell::Shell;
use sena_cli::tabs::{CliTabKind, CliWindowMode};
use sena_cli::{actors_tab, config_editor, diagnostics_tab, logging, onboarding, resources_tab, tabs, test_mode};
use tracing::{debug, error, info};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let window_mode = tabs::parse_window_mode(&args).map_err(anyhow::Error::msg)?;

    let log_path = logging::init_tracing()?;

    info!(log_path = %log_path.display(), "Sena CLI starting");
    debug!(?window_mode, "CLI arguments parsed");

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

    match window_mode {
        CliWindowMode::LegacyConfig => {
            let mut ipc_client = ipc_client;
            let mut editor = config_editor::ConfigEditor::new(&mut ipc_client);
            if let Err(e) = editor.run().await {
                error!("Config editor error: {}", e);
                return Err(anyhow::anyhow!("Config editor failed: {}", e));
            }
        }
        CliWindowMode::Tab(CliTabKind::Config) => {
            let mut ipc_client = ipc_client;
            let mut editor = config_editor::ConfigEditor::new(&mut ipc_client).tabbed();
            if let Err(e) = editor.run().await {
                error!("Config tab error: {}", e);
                return Err(anyhow::anyhow!("Config tab failed: {}", e));
            }
        }
        CliWindowMode::Tab(CliTabKind::Diag) => {
            if let Err(e) = diagnostics_tab::run(ipc_client).await {
                error!("Diagnostics tab error: {}", e);
                return Err(anyhow::anyhow!("Diagnostics tab failed: {}", e));
            }
        }
        CliWindowMode::Tab(CliTabKind::Resources) => {
            if let Err(e) = resources_tab::run(ipc_client).await {
                error!("Resources tab error: {}", e);
                return Err(anyhow::anyhow!("Resources tab failed: {}", e));
            }
        }
        CliWindowMode::Tab(CliTabKind::Actors) => {
            if let Err(e) = actors_tab::run(ipc_client).await {
                error!("Actors tab error: {}", e);
                return Err(anyhow::anyhow!("Actors tab failed: {}", e));
            }
        }
        CliWindowMode::Live => {
            let shell = Shell::new(ipc_client).await?;
            if let Err(e) = shell.run().await {
                error!("Shell error: {}", e);
                return Err(anyhow::anyhow!("Shell failed: {}", e));
            }
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
