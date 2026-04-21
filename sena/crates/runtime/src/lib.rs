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

mod analytics;

pub mod boot;
pub mod builder;
pub mod config;
pub mod download_manager;
pub mod error;
pub mod health;
pub mod llama_backend;
pub mod ipc_server;
pub mod single_instance;
pub mod supervisor;

pub use boot::{BootResult, boot};
pub use config::{SenaConfig, load_or_create_config, save_config};
pub use download_manager::{DownloadClient, DownloadError, ModelCache};
pub use error::RuntimeError;
pub use health::{ActorEntry, ActorRegistry};
pub use ipc_server::{IpcCommand, IpcServer, spawn_ipc_server};
pub use single_instance::InstanceGuard;
pub use supervisor::supervision_loop;

// Re-export llama.cpp log suppression for CLI use.
pub use inference::suppress_llama_logs;

#[cfg(test)]
mod tests {
    use super::*;
    use speech::{ModelCache, ModelManifest};
    use tempfile::tempdir;
    use tokio::fs;

    #[tokio::test]
    async fn boot_completes_successfully() {
        // Boot sequence should complete successfully with minimal setup.
        // Speech model verification is permissive: missing models trigger warnings
        // but do not block boot. Speech actors fall back to stub backends.
        //
        // Embed model verification is STRICT: boot fails if the required embed
        // model is missing. This test creates a stub embed model file to satisfy
        // the strict requirement.
        let temp_dir = tempdir().expect("create tempdir");

        // Override APPDATA/HOME for this test
        #[cfg(target_os = "windows")]
        unsafe {
            std::env::set_var("APPDATA", temp_dir.path());
        }

        #[cfg(any(target_os = "macos", target_os = "linux"))]
        unsafe {
            std::env::set_var("HOME", temp_dir.path());
        }

        // Create embed models directory and stub embed model file
        let embed_model = ModelManifest::required_embed_model();

        #[cfg(target_os = "windows")]
        let embed_models_dir = temp_dir.path().join("sena").join("models").join("embed");

        #[cfg(target_os = "macos")]
        let embed_models_dir = temp_dir
            .path()
            .join("Library")
            .join("Application Support")
            .join("sena")
            .join("models")
            .join("embed");

        #[cfg(target_os = "linux")]
        let embed_models_dir = temp_dir
            .path()
            .join(".config")
            .join("sena")
            .join("models")
            .join("embed");

        fs::create_dir_all(&embed_models_dir)
            .await
            .expect("create embed models dir");

        let model_path = ModelCache::cached_path(&embed_models_dir, &embed_model);
        fs::write(&model_path, b"stub embed model data")
            .await
            .expect("write stub embed model");

        let result = boot::boot().await;
        assert!(result.is_ok());

        let boot_result = result.unwrap();
        assert!(!boot_result.actor_handles.is_empty());
        assert!(!boot_result.expected_actors.is_empty());
    }
}
