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
}
