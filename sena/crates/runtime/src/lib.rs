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
pub mod single_instance;
pub mod supervisor;

pub use boot::{BootResult, boot};
pub use config::{SenaConfig, load_or_create_config, save_config};
pub use download_manager::{DownloadClient, DownloadError, ModelCache};
pub use error::RuntimeError;
pub use health::{ActorEntry, ActorRegistry};
pub use single_instance::InstanceGuard;
pub use supervisor::supervision_loop;

// Re-export llama.cpp log suppression for CLI use.
pub use inference::suppress_llama_logs;

/// Resolve the default Ollama models directory for this host.
pub fn ollama_models_dir() -> Result<std::path::PathBuf, inference::InferenceError> {
    infer::ollama_models_dir().map_err(|e| inference::InferenceError::BackendFailed(e.to_string()))
}

/// Discover models from an Ollama models directory.
pub fn discover_models(
    models_dir: &std::path::Path,
) -> Result<inference::ModelRegistry, inference::InferenceError> {
    inference::discover_models(models_dir)
}

/// Return the currently selected backend type label.
pub fn auto_detect_backend_name() -> String {
    inference::preferred_llama_backend().to_string()
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SpeechStatusSnapshot {
    pub speech_enabled: bool,
    pub stt_enabled: bool,
    pub tts_enabled: bool,
    pub wakeword_enabled: bool,
    pub wakeword_ready: bool,
    pub stt_backend: String,
    pub speech_models_dir: std::path::PathBuf,
}

pub async fn speech_status_snapshot() -> Result<SpeechStatusSnapshot, String> {
    let config = load_or_create_config()
        .await
        .map_err(|e| format!("failed to load config: {}", e))?;

    let speech_models_dir = resolve_speech_models_dir()?;
    let encoder = speech::ModelCache::cached_path(
        &speech_models_dir,
        &speech::ModelManifest::parakeet_encoder(),
    );
    let decoder = speech::ModelCache::cached_path(
        &speech_models_dir,
        &speech::ModelManifest::parakeet_decoder(),
    );
    let tokenizer = speech::ModelCache::cached_path(
        &speech_models_dir,
        &speech::ModelManifest::parakeet_tokenizer(),
    );
    let parakeet_ready = encoder.exists() && decoder.exists() && tokenizer.exists();
    let wakeword = speech::ModelCache::cached_path(
        &speech_models_dir,
        &speech::ModelManifest::open_wakeword(),
    );
    let wakeword_ready = wakeword.exists();

    Ok(SpeechStatusSnapshot {
        speech_enabled: config.speech_enabled,
        stt_enabled: config.speech_enabled && parakeet_ready,
        tts_enabled: config.speech_enabled,
        wakeword_enabled: config.speech_enabled && config.wakeword_enabled && wakeword_ready,
        wakeword_ready,
        stt_backend: if parakeet_ready {
            "parakeet".to_string()
        } else {
            "stub".to_string()
        },
        speech_models_dir,
    })
}

fn resolve_speech_models_dir() -> Result<std::path::PathBuf, String> {
    #[cfg(target_os = "windows")]
    {
        let appdata = std::env::var("APPDATA").map_err(|e| format!("APPDATA not set: {}", e))?;
        Ok(std::path::PathBuf::from(appdata)
            .join("sena")
            .join("models")
            .join("speech"))
    }

    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").map_err(|e| format!("HOME not set: {}", e))?;
        Ok(std::path::PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("sena")
            .join("models")
            .join("speech"))
    }

    #[cfg(target_os = "linux")]
    {
        let home = std::env::var("HOME").map_err(|e| format!("HOME not set: {}", e))?;
        Ok(std::path::PathBuf::from(home)
            .join(".config")
            .join("sena")
            .join("models")
            .join("speech"))
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::ffi::OsString;
    use std::path::Path;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    #[cfg(target_os = "windows")]
    const TEST_ENV_KEY: &str = "APPDATA";

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    const TEST_ENV_KEY: &str = "HOME";

    pub(crate) fn env_test_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .expect("env test lock should not be poisoned")
    }

    pub(crate) struct TestEnvGuard {
        previous: Option<OsString>,
    }

    impl TestEnvGuard {
        pub(crate) fn set(path: &Path) -> Self {
            let previous = std::env::var_os(TEST_ENV_KEY);
            unsafe {
                std::env::set_var(TEST_ENV_KEY, path);
            }
            Self { previous }
        }
    }

    impl Drop for TestEnvGuard {
        fn drop(&mut self) {
            unsafe {
                if let Some(previous) = self.previous.as_ref() {
                    std::env::set_var(TEST_ENV_KEY, previous);
                } else {
                    std::env::remove_var(TEST_ENV_KEY);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{TestEnvGuard, env_test_lock};
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
        let _env_lock = env_test_lock();
        let temp_dir = tempdir().expect("create tempdir");
        let _env = TestEnvGuard::set(temp_dir.path());

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
