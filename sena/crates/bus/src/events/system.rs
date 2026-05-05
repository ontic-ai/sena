//! System-level events for the Sena event bus.

use serde::{Deserialize, Serialize};
use std::time::Instant;

/// Kinds of models Sena manages.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ModelKind {
    /// Large Language Model (GGUF).
    Llm,
    /// Embedding model (Bert).
    Embedding,
    /// Speech-to-text model (Whisper).
    Stt,
}

/// Status of an individual actor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ActorStatus {
    /// Actor is currently starting up.
    Starting,
    /// Actor is fully operational.
    Ready,
    /// Actor is idle or paused.
    Idle,
    /// Actor has encountered a non-fatal error but is still running.
    Degraded {
        /// Warning message or error description.
        reason: String,
    },
    /// Actor has failed and is no longer processing events.
    Failed {
        /// Error message.
        reason: String,
    },
}

/// Health information for a single actor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActorHealth {
    /// Name of the actor (e.g., "ctp", "soul").
    pub name: String,
    /// Current operational status.
    pub status: ActorStatus,
    /// Last heartbeat or activity timestamp.
    #[serde(with = "instant_serde")]
    pub last_seen: Instant,
}

/// System lifecycle and control events.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    ActorReady { actor_name: String },
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
    /// Configuration was updated.
    ConfigUpdated {
        /// Path to the key that was updated.
        path: String,
    },
}

/// Serde serialization modules for standard types not implementing Serialize.
pub mod instant_serde {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::{Duration, Instant, SystemTime};

    /// Serialize an Instant by converting it to SystemTime.
    /// Note: This is an approximation since Instant is monotonic and SystemTime is wall-clock.
    pub fn serialize<S>(instant: &Instant, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let now = Instant::now();
        let sys_now = SystemTime::now();
        let diff = if *instant > now {
            sys_now + instant.duration_since(now)
        } else {
            sys_now - now.duration_since(*instant)
        };
        serde::Serialize::serialize(&diff, serializer)
    }

    /// Deserialize an Instant by reconstructing it relative to current time.
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Instant, D::Error>
    where
        D: Deserializer<'de>,
    {
        let sys_time: SystemTime = Deserialize::deserialize(deserializer)?;
        let sys_now = SystemTime::now();
        let now = Instant::now();

        Ok(if sys_time > sys_now {
            now + sys_time
                .duration_since(sys_now)
                .unwrap_or(Duration::from_secs(0))
        } else {
            now - sys_now
                .duration_since(sys_time)
                .unwrap_or(Duration::from_secs(0))
        })
    }
}
