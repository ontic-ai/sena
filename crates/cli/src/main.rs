//! Sena CLI — application entry point.

mod display;
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
/// - Stderr: also emitted in debug builds or when `SENA_LOG_STDERR=1` is set.
/// - Level override: `SENA_LOG` env var (default `info`).
///
/// Returns a `WorkerGuard` that must be kept alive for the duration of the process.
fn setup_logging() -> WorkerGuard {
    let log_dir = sena_log_dir();
    let _ = std::fs::create_dir_all(&log_dir);

    let appender = tracing_appender::rolling::daily(&log_dir, "sena.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(appender);

    let level_filter = std::env::var("SENA_LOG").unwrap_or_else(|_| "info".to_string());
    let filter = tracing_subscriber::EnvFilter::new(level_filter);

    let emit_stderr = cfg!(debug_assertions)
        || std::env::var("SENA_LOG_STDERR")
            .map(|v| v == "1")
            .unwrap_or(false);

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
            .or_else(|_| {
                std::env::var("HOME").map(|h| PathBuf::from(h).join(".config"))
            })
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("sena")
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let _log_guard = setup_logging();

    let args: Vec<String> = std::env::args().collect();

    match args.get(1).map(String::as_str) {
        Some("query") => query::run_from_args(&args).await,
        Some("models") => model_selector::run().await,
        Some("cli") | Some("interactive") => {
            let runtime = runtime::boot_ready()
                .await
                .map_err(anyhow::Error::from)?;
            let is_first_boot = runtime.is_first_boot;
            shell::run_with_runtime(runtime, is_first_boot).await
        }
        None => runtime::run_background()
            .await
            .map_err(anyhow::Error::from),
        _ => {
            tracing::error!("unknown argument: {}", args.get(1).map(String::as_str).unwrap_or(""));
            eprintln!("Usage: sena [cli|query <type>|models]");
            anyhow::bail!("unknown argument")
        }
    }
}

