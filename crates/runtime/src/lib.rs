//! Boot sequence, actor registry, shutdown orchestration.
//!
//! Public API for daemon and CLI mode:
//! - `run_background()` — boot + readiness gate + supervision loop (daemon mode).
//! - `boot_ready()` — boot + readiness gate, returning the live `Runtime` (CLI mode).

pub mod boot;
pub mod config;
pub mod models;
pub mod registry;
pub mod shutdown;
pub mod supervisor;
pub mod tray;

pub use boot::{boot, BootError, Runtime};
pub use config::{save_config, ConfigError, SenaConfig};
pub use models::{discover_models, ollama_models_dir, InferenceError, ModelRegistry};
pub use registry::ActorRegistry;
pub use shutdown::{shutdown, wait_for_sigint, ShutdownError};
pub use tray::TrayManager;

use std::time::Duration;

/// Unified error type for top-level runtime operations.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("{0}")]
    Boot(#[from] BootError),
    #[error("{0}")]
    Shutdown(#[from] ShutdownError),
}

/// Boot Sena and run in background (daemon) mode.
///
/// Performs the full boot sequence, waits for all actors to become ready,
/// broadcasts `BootComplete`, then enters the supervision loop.
/// Blocks until shutdown completes.
pub async fn run_background() -> Result<(), RuntimeError> {
    let runtime = boot_ready_impl().await?;
    supervisor::supervision_loop(runtime).await?;
    Ok(())
}

/// Boot Sena and wait for all actors to become ready.
///
/// Broadcasts `BootComplete` once the readiness gate passes.
/// Returns the live `Runtime` for the caller to use (CLI mode).
pub async fn boot_ready() -> Result<Runtime, BootError> {
    boot_ready_impl().await
}

/// Shared implementation: boot → readiness gate → BootComplete → optional TTS greeting.
async fn boot_ready_impl() -> Result<Runtime, BootError> {
    use bus::{Event, SystemEvent};

    let runtime = boot::boot().await?;

    // Wait up to 30 seconds for all spawned actors to emit ActorReady.
    supervisor::wait_for_readiness(
        &runtime.bus,
        &runtime.expected_actors,
        Duration::from_secs(30),
    )
    .await?;

    // All actors confirmed up — now safe to broadcast BootComplete.
    runtime
        .bus
        .broadcast(Event::System(SystemEvent::BootComplete))
        .await
        .map_err(|e| BootError::BroadcastFailed(e.to_string()))?;

    // Post-boot TTS greeting (non-fatal — speech may not be available).
    if runtime.config.speech_enabled {
        use bus::events::speech::SpeechEvent;
        let _ = runtime
            .bus
            .broadcast(Event::Speech(SpeechEvent::SpeakRequested {
                text: "Hi, I'm Sena".to_string(),
                request_id: 0,
            }))
            .await;
    }

    Ok(runtime)
}
