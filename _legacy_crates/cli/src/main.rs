//! Sena CLI — application entry point.

mod display;
mod ipc_client;
mod model_selector;
mod onboarding;
mod query;
mod shell;
mod tui_state;

use std::path::PathBuf;

use anyhow::Result;
use tracing_appender::non_blocking::WorkerGuard;

/// Set up the `tracing` subscriber.
///
/// - File: `INFO`+ written to `<config_dir>/sena/sena.<date>.log` always.
/// - Stderr: also emitted in debug builds or when `SENA_LOG_STDERR=1` is set,
///   BUT only when `allow_stderr` is true. TUI mode passes `false` because
///   tracing output written to stderr while ratatui owns the alternate screen
///   corrupts terminal state and can cause the TUI to exit unexpectedly.
/// - Level override: `SENA_LOG` env var (default `info`).
///
/// Returns a `WorkerGuard` that must be kept alive for the duration of the process.
fn setup_logging(allow_stderr: bool) -> WorkerGuard {
    let log_dir = sena_log_dir();
    let _ = std::fs::create_dir_all(&log_dir);

    let appender = tracing_appender::rolling::daily(&log_dir, "sena.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(appender);

    let level_filter = std::env::var("SENA_LOG").unwrap_or_else(|_| "info".to_string());
    let filter = tracing_subscriber::EnvFilter::new(level_filter);

    let emit_stderr = allow_stderr
        && (cfg!(debug_assertions)
            || std::env::var("SENA_LOG_STDERR")
                .map(|v| v == "1")
                .unwrap_or(false));

    use tracing_subscriber::prelude::*;
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_target(false)
        .with_ansi(false)
        .compact();

    if emit_stderr {
        let stderr_layer = tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .with_target(false)
            .compact();
        tracing_subscriber::registry()
            .with(filter)
            .with(file_layer)
            .with(stderr_layer)
            .init();
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(file_layer)
            .init();
    }

    guard
}

/// Compute the log directory using the same convention as `platform::config_dir()`.
fn sena_log_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("sena")
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("Library")
            .join("Application Support")
            .join("sena")
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("sena")
    }
}

/// Pause before exit on Windows when spawned from a GUI/tray menu.
/// This prevents the console window from closing before the user sees error messages.
/// On Windows CREATE_NEW_CONSOLE spawn, the window would close immediately on exit(1).
#[cfg(target_os = "windows")]
fn pause_before_exit() {
    eprintln!("\nPress Enter to close...");
    let _ = std::io::stdin().read_line(&mut String::new());
}

#[cfg(not(target_os = "windows"))]
fn pause_before_exit() {
    // macOS/Linux: if launched from Terminal.app or an existing terminal, no pause needed.
    // The terminal remains open after process exits.
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    // In TUI/CLI mode, suppress stderr logging to prevent tracing output from
    // corrupting ratatui's alternate screen buffer. File logging continues normally.
    let is_tui_mode = args
        .get(1)
        .map(|a| a == "cli" || a == "interactive")
        .unwrap_or(false);
    let _log_guard = setup_logging(!is_tui_mode);

    match args.get(1).map(String::as_str) {
        Some("query") => query::run_from_args(&args).await,
        Some("models") => model_selector::run().await,
        Some("cli") | Some("interactive") => {
            // Suppress llama.cpp's direct-to-stderr logs before any TUI rendering.
            // Model loading happens asynchronously after the TUI starts and would
            // otherwise corrupt ratatui's alternate screen buffer.
            runtime::suppress_llama_logs();

            // G3: Check for first boot before auto-starting daemon.
            // If first boot, run onboarding wizard to collect user preferences.
            let is_first_boot = runtime::is_first_boot().unwrap_or(false);
            let onboarding_user_name = if is_first_boot {
                tracing::info!("First boot detected — running onboarding wizard");

                let models_available = runtime::ollama_models_dir()
                    .ok()
                    .and_then(|d| runtime::discover_models(&d).ok())
                    .map(|r| !r.is_empty())
                    .unwrap_or(false);

                // Run onboarding wizard standalone (no bus yet — daemon not started).
                let onboarding_result = match onboarding::run_wizard(None, models_available).await {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("Onboarding failed: {}", e);
                        pause_before_exit();
                        std::process::exit(1);
                    }
                };

                // Save config with onboarding preferences.
                let mut config = runtime::config::load_or_create_config().await?;
                config.file_watch_paths = onboarding_result.file_watch_paths;
                config.clipboard_observation_enabled =
                    onboarding_result.clipboard_observation_enabled;
                if let Err(e) = runtime::save_config(&config).await {
                    eprintln!("Failed to save config: {}", e);
                    pause_before_exit();
                    std::process::exit(1);
                }

                tracing::info!("Onboarding complete — config saved");
                Some(onboarding_result.user_name)
            } else {
                None
            };

            // Phase 6: CLI connects to running daemon over IPC.
            // If daemon is not running, auto-start it and shut it down on exit.
            let cli_started_daemon = if !runtime::is_daemon_running() {
                tracing::info!("Daemon not running — auto-starting...");
                tokio::spawn(async {
                    if let Err(e) = runtime::run_background().await {
                        tracing::error!("auto-started daemon exited with error: {}", e);
                    }
                });
                // Poll until the IPC server is ready (max 30 seconds to allow full boot).
                // We poll IpcClient::connect() directly — the IPC server starts at boot
                // Step 13, well after the single-instance lock (Step 0), so checking
                // is_daemon_running() would be a false-positive readiness signal.
                let mut ready = false;
                for _ in 0..300 {
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    if ipc_client::IpcClient::connect().await.is_ok() {
                        ready = true;
                        break;
                    }
                }
                if !ready {
                    eprintln!("Sena daemon IPC server failed to become ready within 30 seconds.");
                    pause_before_exit();
                    std::process::exit(1);
                }
                true
            } else {
                false
            };

            // If first boot, send InitializeName to Soul via IPC now that daemon is up.
            if let Some(user_name) = onboarding_user_name {
                tracing::info!("Sending InitializeName to Soul via IPC");
                if let Ok(mut client) = ipc_client::IpcClient::connect().await {
                    if let Err(e) = client
                        .send(bus::IpcPayload::InitializeName {
                            name: user_name.clone(),
                        })
                        .await
                    {
                        tracing::warn!("Failed to send InitializeName via IPC: {}", e);
                        eprintln!("Warning: Failed to set user name in Soul: {}", e);
                    } else {
                        tracing::info!("InitializeName sent successfully");
                    }
                } else {
                    tracing::warn!("Failed to connect to IPC after daemon start");
                }
            }

            // Connect to daemon IPC and run TUI in IPC mode.
            let result = shell::run_with_ipc().await;
            if let Err(e) = result {
                eprintln!("CLI error: {}", e);
                pause_before_exit();
                std::process::exit(1);
            }

            // If we auto-started the daemon, shut it down now.
            if cli_started_daemon {
                tracing::info!("CLI exited — shutting down auto-started daemon");
                if let Ok(mut client) = ipc_client::IpcClient::connect().await {
                    let _ = client.send(bus::IpcPayload::ShutdownRequested).await;
                    // Give daemon a moment to process the shutdown.
                    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                }
            }

            Ok(())
        }
        None => runtime::run_background().await.map_err(anyhow::Error::from),
        _ => {
            tracing::error!(
                "unknown argument: {}",
                args.get(1).map(String::as_str).unwrap_or("")
            );
            eprintln!("Usage: sena [cli|query <type>|models]");
            anyhow::bail!("unknown argument")
        }
    }
}
