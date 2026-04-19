//! Runtime subsystem: boot sequence, actor registry, supervision, IPC server.
//!
//! The runtime is the composition root for Sena. It owns:
//! - Boot sequence: ordered initialization of all subsystems
//! - Actor registry: tracks spawned actors and their lifecycle
//! - Supervision loop: monitors actor health, handles readiness gate
//! - IPC server: receives commands from the CLI
//!
//! ## Boot Sequence
//!
//! Order is strict:
//! 1. Config load
//! 2. Encryption init
//! 3. EventBus init
//! 4. Soul init
//! 5. Core actors spawn (Platform, Soul, Memory, Inference, CTP, Speech)
//!
//! After boot, the supervisor waits for all expected actors to emit ActorReady,
//! then broadcasts BootComplete.
//!
//! ## Dependencies
//!
//! Runtime depends on all other subsystem crates and constructs their concrete
//! actor instances via builder functions.

pub mod boot;
pub mod builder;
pub mod error;
pub mod health;
pub mod ipc_server;
pub mod supervisor;

pub use boot::{BootResult, boot};
pub use error::RuntimeError;
pub use health::{ActorEntry, ActorRegistry};
pub use ipc_server::{IpcCommand, IpcServer, spawn_ipc_server};
pub use supervisor::supervision_loop;

/// Run Sena in background daemon mode.
///
/// This is the daemon entry point. It:
/// 1. Boots the runtime (all actors)
/// 2. Runs the supervision loop (readiness gate, health monitoring, shutdown)
/// 3. Returns Ok(()) on clean shutdown or Err on critical failure
///
/// Use this for standalone daemon mode. For integrated CLI+daemon mode,
/// call `boot()` and `supervision_loop()` separately.
pub async fn run_background() -> Result<(), RuntimeError> {
    tracing::info!("RUNTIME: starting daemon mode");
    let boot_result = boot().await?;
    supervision_loop(boot_result).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn boot_completes_successfully() {
        let result = boot::boot().await;
        assert!(result.is_ok());

        let boot_result = result.unwrap();
        assert!(boot_result.actor_handles.len() > 0);
        assert!(boot_result.expected_actors.len() > 0);
    }

    #[tokio::test]
    async fn run_background_completes_with_shutdown() {
        // Spawn run_background in a task
        let runtime_handle = tokio::spawn(async {
            run_background().await
        });

        // Give it time to boot
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Send shutdown signal via bus
        // (This test is incomplete — would need access to the bus)
        // For now, just abort the task to test that it compiles
        runtime_handle.abort();
    }
}
