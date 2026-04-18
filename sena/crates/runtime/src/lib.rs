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
//!    2.5. Model download (Phase 4)
//! 3. EventBus init
//! 4. Soul init
//! 5. Core actors spawn (Platform, Soul, Memory, Inference, CTP, Speech)
//! 6. IPC server spawn (Phase 5)
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
pub mod download_manager;
pub mod error;
pub mod health;
pub mod ipc_server;
pub mod supervisor;

pub use boot::{BootResult, boot};
pub use error::RuntimeError;
pub use health::{ActorEntry, ActorRegistry};
pub use ipc_server::{
    IpcClientHandle, IpcCommand, IpcEvent, IpcResponse, IpcServer, LoopInfo, spawn_ipc_server,
};
pub use supervisor::supervision_loop;

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
}
