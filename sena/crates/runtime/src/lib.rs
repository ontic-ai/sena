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
pub mod config;
pub mod download_manager;
pub mod error;
pub mod health;
pub mod supervisor;

pub use boot::{BootResult, boot};
pub use config::{SenaConfig, load_or_create_config, save_config};
pub use download_manager::{DownloadClient, DownloadError, ModelCache};
pub use error::RuntimeError;
pub use health::{ActorEntry, ActorRegistry};
pub use supervisor::supervision_loop;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn boot_completes_successfully() {
        // Boot sequence should complete successfully even without speech models.
        // Speech model verification is permissive: missing models trigger warnings
        // but do not block boot. Speech actors fall back to stub backends.
        let result = boot::boot().await;
        assert!(result.is_ok());

        let boot_result = result.unwrap();
        assert!(!boot_result.actor_handles.is_empty());
        assert!(!boot_result.expected_actors.is_empty());
    }
}
