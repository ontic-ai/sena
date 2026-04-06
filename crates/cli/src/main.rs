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
            // Phase 6: CLI connects to running daemon over IPC.
            // Does NOT boot a runtime — daemon must be running.
            if !runtime::is_daemon_running() {
                eprintln!("Sena daemon is not running.");
                eprintln!("Start it first: sena");
                pause_before_exit();
                std::process::exit(1);
            }
            // Connect to daemon IPC and run TUI in IPC mode.
            let result = shell::run_with_ipc().await;
            if let Err(e) = result {
                eprintln!("CLI error: {}", e);
                pause_before_exit();
                std::process::exit(1);
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
