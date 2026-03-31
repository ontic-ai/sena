//! Sena CLI — application entry point.
//!
//! Default mode: interactive REPL (`cargo run`).
//! Legacy scripting mode: `cargo run -- query <type>` (no REPL, no stdin).
//! Standalone model picker: `cargo run -- models`.

mod display;
mod model_selector;
mod onboarding;
mod query;
mod shell;
mod tui_state;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use bus::{Event, SystemEvent};

enum StartMode {
    Background,
    OpenCli,
    Query,
    Models,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    match parse_start_mode(&args)? {
        StartMode::Background => app_mode(false).await,
        StartMode::OpenCli => app_mode(true).await,
        StartMode::Query => query_mode(&args).await,
        StartMode::Models => model_selector::run().await,
    }
}

fn parse_start_mode(args: &[String]) -> Result<StartMode> {
    match args.get(1).map(String::as_str) {
        Some("query") => Ok(StartMode::Query),
        Some("models") => Ok(StartMode::Models),
        Some("cli") | Some("interactive") => Ok(StartMode::OpenCli),
        None => Ok(StartMode::Background),
        _ => {
            print_usage();
            anyhow::bail!(
                "Invalid arguments. Run with no args for tray mode or 'cli' for the shell."
            )
        }
    }
}

async fn app_mode(open_cli_on_start: bool) -> Result<()> {
    display::banner();

    display::info("Booting runtime...");
    let runtime = Arc::new(runtime::boot().await?);
    display::success("Runtime ready.");

    let mut needs_onboarding = runtime.is_first_boot;
    if needs_onboarding {
        display::info("First boot detected. Open the CLI to complete onboarding.");
    }

    if open_cli_on_start {
        let exit_reason = open_cli_session(Arc::clone(&runtime), &mut needs_onboarding).await?;
        if exit_reason == shell::ShellExitReason::Shutdown {
            return shutdown_runtime(runtime).await;
        }

        display::info("CLI closed. Tray/runtime still running.");
    }

    run_headless_loop(runtime, needs_onboarding).await
}

async fn open_cli_session(
    runtime: Arc<runtime::Runtime>,
    needs_onboarding: &mut bool,
) -> Result<shell::ShellExitReason> {
    maybe_run_onboarding(&runtime, needs_onboarding).await?;
    shell::run(runtime).await
}

async fn maybe_run_onboarding(
    runtime: &Arc<runtime::Runtime>,
    needs_onboarding: &mut bool,
) -> Result<()> {
    if !*needs_onboarding {
        return Ok(());
    }

    let models_available = platform::ollama_models_dir()
        .ok()
        .and_then(|models_dir| inference::discover_models(&models_dir).ok())
        .map(|registry| !registry.is_empty())
        .unwrap_or(false);

    let result = onboarding::run_wizard(&runtime.bus, models_available).await?;

    let user_name = result.user_name.clone();
    let mut updated_config = runtime.config.clone();
    updated_config.file_watch_paths = result.file_watch_paths;
    updated_config.clipboard_observation_enabled = result.clipboard_observation_enabled;

    runtime::save_config(&updated_config).await?;
    display::success(&format!("Onboarding saved for {}.", user_name));
    *needs_onboarding = false;

    Ok(())
}

async fn run_headless_loop(
    runtime: Arc<runtime::Runtime>,
    mut needs_onboarding: bool,
) -> Result<()> {
    let mut bus_rx = runtime.bus.subscribe_broadcast();
    display::info("Tray/runtime running. Use the tray menu to open the CLI.");

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                display::info("Ctrl+C received. Shutting down...");
                break;
            }
            event = bus_rx.recv() => {
                match event {
                    Ok(Event::System(SystemEvent::CliAttachRequested)) => {
                        let exit_reason = open_cli_session(Arc::clone(&runtime), &mut needs_onboarding).await?;
                        if exit_reason == shell::ShellExitReason::Shutdown {
                            break;
                        }

                        display::info("CLI closed. Tray/runtime still running.");
                    }
                    Ok(Event::System(SystemEvent::ShutdownSignal)) => break,
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }

    drop(bus_rx);
    shutdown_runtime(runtime).await
}

async fn shutdown_runtime(runtime: Arc<runtime::Runtime>) -> Result<()> {
    let timeout = Duration::from_secs(runtime.config.shutdown_timeout_secs);
    let runtime = Arc::try_unwrap(runtime)
        .map_err(|_| anyhow::anyhow!("runtime has remaining references at shutdown"))?;
    runtime::shutdown(runtime, timeout).await?;
    display::success("Sena stopped cleanly.");
    Ok(())
}

/// Legacy scripting mode: single transparency query, print result, exit.
async fn query_mode(args: &[String]) -> Result<()> {
    if args.len() < 3 {
        eprintln!("Error: 'query' requires a type argument");
        eprintln!("Usage: cargo run -- query <observation|memory|explanation>");
        anyhow::bail!("Missing query type")
    }

    let query = query::parse_query_type(&args[2])?;
    let output = query::execute_query(query).await?;
    println!("{output}");
    Ok(())
}

fn print_usage() {
    eprintln!("Sena CLI");
    eprintln!();
    eprintln!("Usage:");
    eprintln!("  cargo run                     Start Sena in tray/runtime mode");
    eprintln!("  cargo run -- cli             Start tray/runtime mode and open the CLI");
    eprintln!("  cargo run -- models           Pick an Ollama model (no REPL needed)");
    eprintln!("  cargo run -- query TYPE       Scripting: single query, print, exit");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_start_mode_is_background() {
        let args = vec!["sena".to_string()];
        assert!(matches!(
            parse_start_mode(&args).expect("mode should parse"),
            StartMode::Background
        ));
    }

    #[test]
    fn cli_argument_requests_shell_session() {
        let args = vec!["sena".to_string(), "cli".to_string()];
        assert!(matches!(
            parse_start_mode(&args).expect("mode should parse"),
            StartMode::OpenCli
        ));
    }
}
