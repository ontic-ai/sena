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
pub mod ipc_server;
pub mod supervisor;

pub use boot::{boot, BootResult};
pub use error::RuntimeError;
pub use ipc_server::{spawn_ipc_server, IpcCommand, IpcServer};
pub use supervisor::supervision_loop;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn boot_and_supervision_integration() {
        let boot_result = boot::boot().await.expect("boot failed");
        let result = supervisor::supervision_loop(boot_result).await;
        assert!(result.is_ok());
    }
}
