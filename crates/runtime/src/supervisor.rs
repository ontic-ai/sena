//! Supervisor — actor readiness gate and daemon lifetime management.
//!
//! After boot completes, the supervisor:
//!
//! 1. `wait_for_readiness()` — blocks until all expected actors have emitted `ActorReady`.
//! 2. `supervision_loop()` — keeps the daemon alive and handles:
//!    - `ShutdownSignal` → graceful shutdown
//!    - `CliAttachRequested` → spawn a new terminal running `sena cli`
//!    - `ActorFailed` → increment failure counter; shutdown after `MAX_ACTOR_RETRIES`
//!    - Ctrl+C → graceful shutdown

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use bus::{Event, SystemEvent};

use crate::boot::{BootError, Runtime};

/// Maximum consecutive failures per actor before the daemon requests shutdown.
const MAX_ACTOR_RETRIES: u32 = 3;

/// Block until all named actors have emitted `SystemEvent::ActorReady`.
///
/// `rx` must be a broadcast receiver that was subscribed BEFORE any actor was
/// spawned — otherwise `ActorReady` events emitted by fast-starting actors will
/// be missed (broadcast channels do not buffer for late subscribers).
///
/// Returns `BootError::ReadinessTimeout` if not all actors become ready within `timeout`.
pub async fn wait_for_readiness(
    mut rx: tokio::sync::broadcast::Receiver<Event>,
    expected_actors: &[&'static str],
    timeout: Duration,
) -> Result<(), BootError> {
    if expected_actors.is_empty() {
        return Ok(());
    }

    let mut remaining: HashSet<&'static str> = expected_actors.iter().copied().collect();
    let deadline = tokio::time::Instant::now() + timeout;

    tracing::info!("waiting for actors: {:?}", expected_actors);

    while !remaining.is_empty() {
        tokio::select! {
            biased;
            _ = tokio::time::sleep_until(deadline) => {
                let mut unready: Vec<&str> = remaining.into_iter().collect();
                unready.sort_unstable();
                tracing::error!("readiness timeout — actors not ready: {:?}", unready);
                return Err(BootError::ReadinessTimeout(format!(
                    "actors not ready within {}s: {:?}",
                    timeout.as_secs(),
                    unready
                )));
            }
            result = rx.recv() => {
                match result {
                    Ok(Event::System(SystemEvent::ActorReady { actor_name })) => {
                        tracing::info!("actor ready: {}", actor_name);
                        remaining.remove(actor_name);
                    }
                    Ok(Event::System(SystemEvent::ActorFailed(ref info))) => {
                        tracing::warn!("actor failed during startup: {} — {}", info.actor_name, info.error_msg);
                        // If an actor failed before emitting ActorReady, remove it from
                        // the remaining set so we do not block forever. The supervision
                        // loop will handle retry after startup completes.
                        remaining.remove(info.actor_name);
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        // Some events were dropped. This means the bus is under heavy load.
                        // Resubscribe to resume receiving, but log a warning — we may have
                        // missed an ActorReady already delivered before catching up.
                        tracing::warn!("readiness receiver lagged by {} events — resubscribing", n);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        return Err(BootError::ReadinessTimeout(
                            "event bus closed during readiness wait".to_string(),
                        ));
                    }
                }
            }
        }
    }

    tracing::info!("all actors ready");
    Ok(())
}

/// Run the supervision loop — serves as the process lifetime owner for daemon mode.
///
/// Blocks until shutdown is complete. Handles:
/// - `ShutdownSignal`: initiates graceful shutdown.
/// - `CliAttachRequested`: spawns a new terminal running `sena cli`.
/// - `ActorFailed`: increments failure count; triggers shutdown after `MAX_ACTOR_RETRIES`.
/// - Ctrl+C: initiates graceful shutdown.
pub async fn supervision_loop(runtime: Runtime) -> Result<(), crate::shutdown::ShutdownError> {
    let mut bus_rx = runtime.bus.subscribe_broadcast();
    let mut failure_counts: HashMap<&'static str, u32> = HashMap::new();

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                break;
            }
            event = bus_rx.recv() => {
                match event {
                    Ok(Event::System(SystemEvent::ShutdownSignal)) => {
                        break;
                    }
                    Ok(Event::System(SystemEvent::CliAttachRequested)) => {
                        open_cli_in_new_terminal();
                    }
                    Ok(Event::System(SystemEvent::ActorFailed(info))) => {
                        let count = failure_counts.entry(info.actor_name).or_insert(0);
                        *count += 1;
                        if *count >= MAX_ACTOR_RETRIES {
                            tracing::error!(
                                "actor '{}' failed {} times — shutting down",
                                info.actor_name, count
                            );
                            // Best-effort: request shutdown via bus before dropping the loop.
                            let _ = runtime
                                .bus
                                .broadcast(Event::System(SystemEvent::ShutdownSignal))
                                .await;
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }
            }
        }
    }

    drop(bus_rx);
    let timeout = Duration::from_secs(runtime.config.shutdown_timeout_secs);
    crate::shutdown::shutdown(runtime, timeout).await
}

/// Open a new terminal window running `sena cli`.
///
/// Non-fatal — errors are logged but do not affect the running daemon.
fn open_cli_in_new_terminal() {
    match std::env::current_exe() {
        Ok(exe_path) => open_cli_impl(exe_path),
        Err(e) => tracing::error!("could not locate executable for Open CLI: {}", e),
    }
}

#[cfg(target_os = "windows")]
fn open_cli_impl(exe_path: std::path::PathBuf) {
    let exe = exe_path.display().to_string();
    let result = std::process::Command::new("cmd")
        .args(["/c", "start", "cmd", "/k", &format!("{} cli", exe)])
        .spawn();
    if let Err(e) = result {
        tracing::error!("failed to open CLI terminal: {}", e);
    }
}

#[cfg(target_os = "macos")]
fn open_cli_impl(exe_path: std::path::PathBuf) {
    let exe = exe_path.display().to_string();
    let script = format!(r#"tell application "Terminal" to do script "{exe} cli""#);
    let result = std::process::Command::new("osascript")
        .args(["-e", &script])
        .spawn();
    if let Err(e) = result {
        tracing::error!("failed to open CLI terminal: {}", e);
    }
}

#[cfg(target_os = "linux")]
fn open_cli_impl(exe_path: std::path::PathBuf) {
    let exe = exe_path.display().to_string();
    let cmd = format!("{exe} cli");
    for term in &["x-terminal-emulator", "gnome-terminal", "xterm", "konsole"] {
        if std::process::Command::new(term)
            .args(["-e", &cmd])
            .spawn()
            .is_ok()
        {
            return;
        }
    }
    eprintln!("[sena] no terminal emulator found to open CLI terminal");
}
