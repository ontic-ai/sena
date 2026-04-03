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

use bus::{Event, EventBus, SystemEvent};

use crate::boot::{BootError, Runtime};

/// Maximum consecutive failures per actor before the daemon requests shutdown.
const MAX_ACTOR_RETRIES: u32 = 3;

/// Block until all named actors have emitted `SystemEvent::ActorReady`.
///
/// Returns `BootError::ReadinessTimeout` if not all actors become ready within `timeout`.
/// A lagged broadcast channel (missed events) is treated as non-fatal; we continue polling.
pub async fn wait_for_readiness(
    bus: &std::sync::Arc<EventBus>,
    expected_actors: &[&'static str],
    timeout: Duration,
) -> Result<(), BootError> {
    if expected_actors.is_empty() {
        return Ok(());
    }

    let mut remaining: HashSet<&'static str> = expected_actors.iter().copied().collect();
    let mut rx = bus.subscribe_broadcast();
    let deadline = tokio::time::Instant::now() + timeout;

    while !remaining.is_empty() {
        tokio::select! {
            biased;
            _ = tokio::time::sleep_until(deadline) => {
                let mut unready: Vec<&str> = remaining.into_iter().collect();
                unready.sort_unstable();
                return Err(BootError::ReadinessTimeout(format!(
                    "actors not ready within {}s: {:?}",
                    timeout.as_secs(),
                    unready
                )));
            }
            result = rx.recv() => {
                match result {
                    Ok(Event::System(SystemEvent::ActorReady { actor_name })) => {
                        remaining.remove(actor_name);
                    }
                    Ok(Event::System(SystemEvent::ActorFailed(ref info))) => {
                        // If an actor failed before emitting ActorReady, remove it from
                        // the remaining set so we do not block forever. The supervision
                        // loop will handle retry after startup completes.
                        remaining.remove(info.actor_name);
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        // Some events were dropped. Resubscribe and continue; we may
                        // have missed an ActorReady that was already in the buffer.
                        rx = bus.subscribe_broadcast();
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
                            eprintln!(
                                "[sena] actor '{}' failed {} times — shutting down",
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
        Err(e) => eprintln!("[sena] could not locate executable for Open CLI: {}", e),
    }
}

#[cfg(target_os = "windows")]
fn open_cli_impl(exe_path: std::path::PathBuf) {
    let exe = exe_path.display().to_string();
    let result = std::process::Command::new("cmd")
        .args(["/c", "start", "cmd", "/k", &format!("{} cli", exe)])
        .spawn();
    if let Err(e) = result {
        eprintln!("[sena] failed to open CLI terminal: {}", e);
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
        eprintln!("[sena] failed to open CLI terminal: {}", e);
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
