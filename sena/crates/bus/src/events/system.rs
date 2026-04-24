//! System-level events: boot, shutdown, failure.

/// Information about a failed actor.
#[derive(Debug, Clone)]
pub struct ActorFailureInfo {
    /// Static name of the actor that failed.
    pub actor_name: &'static str,
    /// Error message describing the failure.
    pub error_msg: String,
}

/// Kind of model being downloaded or managed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelKind {
    /// LLM inference model (GGUF).
    Llm,
    /// Speech-to-text model (Whisper GGUF).
    Stt,
    /// Text-to-speech voice model (Piper).
    Tts,
    /// Wakeword detection model (OpenWakeWord).
    Wakeword,
}

/// Health status of an actor.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum ActorStatus {
    /// Actor is starting — registered but has not yet emitted ActorReady.
    Starting,
    /// Actor is running normally — has emitted ActorReady.
    Running,
    /// Actor has stopped.
    Stopped,
    /// Actor has failed.
    Failed {
        /// Failure reason.
        reason: String,
    },
}

/// Actor health information.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ActorHealth {
    /// Actor name.
    pub name: String,
    /// Current status.
    pub status: ActorStatus,
    /// Uptime in seconds.
    pub uptime_seconds: u64,
}

/// System lifecycle and control events.
#[derive(Debug, Clone)]
pub enum SystemEvent {
    /// Signal to initiate graceful shutdown.
    ShutdownSignal,
    /// Alias for ShutdownSignal for backward compatibility.
    ShutdownRequested,
    /// Emitted by runtime when shutdown sequence has begun.
    ShutdownInitiated,
    /// Boot sequence completed successfully.
    BootComplete,
    /// Emitted when this is the first time Sena has been run.
    FirstBoot,
    /// An actor has failed.
    ActorFailed {
        /// Name of the failed actor.
        actor: String,
        /// Failure reason.
        reason: String,
    },
    /// Emitted by each actor when it has successfully started and is ready.
    ActorReady { actor_name: &'static str },
    /// Encryption subsystem initialized successfully.
    EncryptionInitialized,
    /// Request for health check from an actor or subsystem.
    HealthCheckRequest { target: Option<String> },
    /// Response to health check request.
    HealthCheckResponse {
        /// List of actor health information.
        actors: Vec<ActorHealth>,
        /// Runtime uptime in seconds.
        uptime_seconds: u64,
    },
    /// Progress update for model download.
    DownloadProgress {
        /// Type of model being downloaded.
        model: ModelKind,
        /// Download progress percentage (0-100).
        percent: u8,
    },
    /// Download started for a model.
    DownloadStarted {
        /// Type of model being downloaded.
        model: ModelKind,
    },
    /// Download completed successfully for a model.
    DownloadCompleted {
        /// Type of model that was downloaded.
        model: ModelKind,
    },
    /// Download failed for a model.
    DownloadFailed {
        /// Type of model that failed to download.
        model: ModelKind,
        /// Failure reason.
        reason: String,
    },
    /// First-time onboarding is required (detected at boot).
    OnboardingRequired,
    /// First-time onboarding completed successfully.
    OnboardingCompleted,
    /// Boot sequence failed — daemon cannot start.
    BootFailed {
        /// Failure reason.
        reason: String,
    },
    /// inference_max_tokens was automatically adjusted based on observed usage.
    TokenBudgetAutoTuned {
        /// Previous token limit.
        old_max_tokens: usize,
        /// New token limit after tuning.
        new_max_tokens: usize,
        /// P95 token count from the observation window that drove this decision.
        p95_tokens: usize,
    },
    /// System wake event — emitted when OS wakes from sleep.
    SystemWake,
    /// System sleep event — emitted when OS is about to sleep.
    SystemSleep,
    /// VRAM usage monitoring update.
    VramUsageUpdated {
        /// Used VRAM in megabytes.
        used_mb: u32,
        /// Total VRAM in megabytes.
        total_mb: u32,
        /// Usage percentage (0-100).
        percent: u8,
    },
    /// Request to enable or disable a background loop.
    LoopControlRequested {
        /// Name of the loop to control.
        loop_name: String,
        /// Target state (true = enabled, false = disabled).
        enabled: bool,
    },
    /// Background loop status changed.
    LoopStatusChanged {
        /// Name of the loop.
        loop_name: String,
        /// New state (true = enabled, false = disabled).
        enabled: bool,
    },

    /// Runtime configuration was updated.
    ConfigUpdated {
        /// Dotted config path that changed.
        path: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actor_failure_info_constructs() {
        let info = ActorFailureInfo {
            actor_name: "test_actor",
            error_msg: "test error".to_string(),
        };
        assert_eq!(info.actor_name, "test_actor");
        assert_eq!(info.error_msg, "test error");
    }

    #[test]
    fn system_events_are_cloneable() {
        let event = SystemEvent::BootComplete;
        let cloned = event.clone();
        assert!(matches!(cloned, SystemEvent::BootComplete));
    }

    #[test]
    fn actor_failed_event_constructs() {
        let event = SystemEvent::ActorFailed {
            actor: "test_actor".to_string(),
            reason: "test error".to_string(),
        };
        assert!(matches!(event, SystemEvent::ActorFailed { .. }));
    }

    #[test]
    fn model_kind_variants_exist() {
        let kinds = [
            ModelKind::Llm,
            ModelKind::Stt,
            ModelKind::Tts,
            ModelKind::Wakeword,
        ];
        assert_eq!(kinds.len(), 4);
    }

    #[test]
    fn health_check_events_construct() {
        let req = SystemEvent::HealthCheckRequest {
            target: Some("soul".to_string()),
        };
        let resp = SystemEvent::HealthCheckResponse {
            actors: vec![ActorHealth {
                name: "soul".to_string(),
                status: ActorStatus::Running,
                uptime_seconds: 42,
            }],
            uptime_seconds: 100,
        };
        assert!(matches!(req, SystemEvent::HealthCheckRequest { .. }));
        assert!(matches!(resp, SystemEvent::HealthCheckResponse { .. }));
    }

    #[test]
    fn download_progress_event_constructs() {
        let event = SystemEvent::DownloadProgress {
            model: ModelKind::Stt,
            percent: 75,
        };
        assert!(matches!(event, SystemEvent::DownloadProgress { .. }));
    }

    #[test]
    fn download_lifecycle_events_construct() {
        let started = SystemEvent::DownloadStarted {
            model: ModelKind::Llm,
        };
        let completed = SystemEvent::DownloadCompleted {
            model: ModelKind::Tts,
        };
        let failed = SystemEvent::DownloadFailed {
            model: ModelKind::Wakeword,
            reason: "network error".to_string(),
        };
        assert!(matches!(started, SystemEvent::DownloadStarted { .. }));
        assert!(matches!(completed, SystemEvent::DownloadCompleted { .. }));
        assert!(matches!(failed, SystemEvent::DownloadFailed { .. }));
    }

    #[test]
    fn onboarding_events_construct() {
        let required = SystemEvent::OnboardingRequired;
        let completed = SystemEvent::OnboardingCompleted;
        assert!(matches!(required, SystemEvent::OnboardingRequired));
        assert!(matches!(completed, SystemEvent::OnboardingCompleted));
    }

    #[test]
    fn boot_failed_event_constructs() {
        let event = SystemEvent::BootFailed {
            reason: "speech models unavailable".to_string(),
        };
        assert!(matches!(event, SystemEvent::BootFailed { .. }));
    }

    #[test]
    fn token_budget_auto_tuned_event_constructs() {
        let event = SystemEvent::TokenBudgetAutoTuned {
            old_max_tokens: 512,
            new_max_tokens: 768,
            p95_tokens: 640,
        };
        assert!(matches!(event, SystemEvent::TokenBudgetAutoTuned { .. }));
    }

    #[test]
    fn system_wake_event_constructs() {
        let event = SystemEvent::SystemWake;
        assert!(matches!(event, SystemEvent::SystemWake));
    }

    #[test]
    fn system_sleep_event_constructs() {
        let event = SystemEvent::SystemSleep;
        assert!(matches!(event, SystemEvent::SystemSleep));
    }
}
