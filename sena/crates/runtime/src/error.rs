//! Runtime errors.

use bus::BusError;
use crypto::CryptoError;
use soul::SoulError;

/// Runtime subsystem errors.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    /// Configuration loading failed.
    #[error("config load failed: {0}")]
    ConfigLoadFailed(String),

    /// Encryption initialization failed.
    #[error("encryption init failed: {0}")]
    EncryptionFailed(#[from] CryptoError),

    /// Soul initialization failed.
    #[error("soul init failed: {0}")]
    SoulInitFailed(#[from] SoulError),

    /// Soul store creation or open failed.
    #[error("soul store failed: {0}")]
    SoulStore(String),

    /// Memory store creation or open failed.
    #[error("memory store failed: {0}")]
    MemoryStore(String),

    /// Bus initialization failed.
    #[error("bus init failed: {0}")]
    BusInitFailed(#[from] BusError),

    /// Actor spawn failed.
    #[error("actor spawn failed: {actor_name}: {reason}")]
    ActorSpawnFailed {
        actor_name: &'static str,
        reason: String,
    },

    /// Readiness gate timeout.
    #[error("readiness gate timeout: {0} actors did not emit ActorReady within 30s")]
    ReadinessTimeout(usize),

    /// IPC server failed to start.
    #[error("IPC server failed: {0}")]
    IpcServerFailed(String),

    /// Supervision loop failed.
    #[error("supervision failed: {0}")]
    SupervisionFailed(String),

    /// Model verification or download failed.
    #[error("model verification failed: {0}")]
    ModelVerificationFailed(String),

    /// Model loading failed.
    #[error("model load failed: {0}")]
    ModelLoadFailed(String),

    /// Directory resolution failed.
    #[error("directory resolution failed: {0}")]
    DirectoryResolutionFailed(String),

    /// Another Sena daemon instance is already running.
    #[error("another Sena instance is already running (lock file: {lock_path})")]
    InstanceAlreadyRunning { lock_path: String },

    /// Required model missing and download failed.
    #[error("required model missing: {model_name}. Boot cannot continue. Reason: {reason}")]
    RequiredModelMissing { model_name: String, reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_error_displays_correctly() {
        let err = RuntimeError::ConfigLoadFailed("file not found".to_string());
        assert_eq!(err.to_string(), "config load failed: file not found");
    }

    #[test]
    fn actor_spawn_failure_contains_actor_name() {
        let err = RuntimeError::ActorSpawnFailed {
            actor_name: "test_actor",
            reason: "channel closed".to_string(),
        };
        assert!(err.to_string().contains("test_actor"));
        assert!(err.to_string().contains("channel closed"));
    }

    #[test]
    fn required_model_missing_displays_model_name_and_reason() {
        let err = RuntimeError::RequiredModelMissing {
            model_name: "test-model".to_string(),
            reason: "download failed".to_string(),
        };
        assert!(err.to_string().contains("test-model"));
        assert!(err.to_string().contains("download failed"));
        assert!(err.to_string().contains("Boot cannot continue"));
    }
}
