//! Soul subsystem events: event log writes, summaries, identity signals.

use crate::causal::CausalId;
use std::time::SystemTime;

/// Compact summary of Soul state for bus communication.
/// This is a simplified representation — the full SoulSummary type lives in the soul crate.
#[derive(Debug, Clone)]
pub struct SoulSummary {
    /// Total number of events logged.
    pub total_events: u64,
    /// Most recent event timestamp.
    pub last_event_time: Option<SystemTime>,
    /// Number of identity signals recorded.
    pub identity_signal_count: usize,
}

/// Personality metadata for prompt composition.
#[derive(Debug, Clone)]
pub struct PersonalityMetadata {
    /// Verbosity preference: 0.0 (minimal) to 1.0 (comprehensive).
    pub verbosity: f64,
    /// Response warmth: 0.0 (formal) to 1.0 (warm/friendly).
    pub warmth: f64,
    /// Work cadence preference.
    pub work_cadence: WorkCadence,
}

/// Work cadence preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkCadence {
    Burst,
    Steady,
    LongFocus,
}

/// Distilled identity signal from Soul for CTP/prompt composition.
/// Privacy-safe typed representation of learned identity attributes.
#[derive(Debug, Clone)]
pub struct DistilledIdentitySignal {
    /// The signal's semantic key (e.g., "voice::rate", "work_style::preferred_cadence").
    pub signal_key: String,
    /// The signal's value.
    pub signal_value: String,
    /// Confidence in this signal's accuracy (0.0 to 1.0).
    pub confidence: f32,
}

/// Temporal behavior pattern detected by Soul.
/// Represents stable/recurring patterns in user's temporal behavior.
#[derive(Debug, Clone)]
pub struct TemporalBehaviorPattern {
    /// Pattern type identifier (e.g., "high_cadence_work", "late_night_focus").
    pub pattern_type: String,
    /// Pattern strength [0.0, 1.0].
    pub strength: f64,
    /// First time this pattern was observed.
    pub first_seen: std::time::SystemTime,
    /// Most recent observation of this pattern.
    pub last_seen: std::time::SystemTime,
}

/// Soul subsystem events.
#[derive(Debug, Clone)]
pub enum SoulEvent {
    /// Initialize Soul with user's chosen name (first-boot onboarding).
    InitializeWithName {
        /// User's chosen name.
        name: String,
    },

    /// Request to write an event to the Soul event log.
    WriteRequested {
        description: String,
        app_context: Option<String>,
        timestamp: SystemTime,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Event log write completed successfully.
    EventLogged {
        row_id: u64,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Request for a summary of recent Soul events.
    SummaryRequested {
        max_events: usize,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Summary of recent Soul events.
    SummaryCompleted {
        content: String,
        event_count: usize,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Soul operation failed.
    OperationFailed {
        reason: String,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Personality metadata updated (emitted after schema updates and at boot completion).
    PersonalityUpdated {
        metadata: PersonalityMetadata,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Request to export Soul data to JSON file.
    ExportRequested {
        path: std::path::PathBuf,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Soul export completed successfully.
    ExportCompleted {
        path: std::path::PathBuf,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Soul export failed.
    ExportFailed {
        reason: String,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Request to delete all Soul data (requires explicit confirmation).
    DeleteRequested {
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Explicit confirmation to proceed with deletion (must follow DeleteRequested).
    DeleteConfirmed {
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Soul data deletion completed.
    Deleted {
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Identity signal distilled and ready for CTP/prompt use.
    IdentitySignalDistilled {
        signal: DistilledIdentitySignal,
        /// Causal chain ID.
        causal_id: CausalId,
    },

    /// Temporal behavior pattern detected.
    TemporalPatternDetected {
        pattern: TemporalBehaviorPattern,
        /// Causal chain ID.
        causal_id: CausalId,
    },
}

impl SoulEvent {
    pub fn causal_id(&self) -> Option<CausalId> {
        match self {
            Self::InitializeWithName { .. } => None,
            Self::WriteRequested { causal_id, .. }
            | Self::EventLogged { causal_id, .. }
            | Self::SummaryRequested { causal_id, .. }
            | Self::SummaryCompleted { causal_id, .. }
            | Self::OperationFailed { causal_id, .. }
            | Self::PersonalityUpdated { causal_id, .. }
            | Self::ExportRequested { causal_id, .. }
            | Self::ExportCompleted { causal_id, .. }
            | Self::ExportFailed { causal_id, .. }
            | Self::DeleteRequested { causal_id, .. }
            | Self::DeleteConfirmed { causal_id, .. }
            | Self::Deleted { causal_id, .. }
            | Self::IdentitySignalDistilled { causal_id, .. }
            | Self::TemporalPatternDetected { causal_id, .. } => Some(*causal_id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soul_event_causal_id_extraction() {
        let cid = CausalId::new();
        let event = SoulEvent::WriteRequested {
            description: "test event".to_string(),
            app_context: None,
            timestamp: SystemTime::now(),
            causal_id: cid,
        };
        assert_eq!(event.causal_id(), Some(cid));
    }

    #[test]
    fn soul_events_are_cloneable() {
        let event = SoulEvent::EventLogged {
            row_id: 42,
            causal_id: CausalId::new(),
        };
        let cloned = event.clone();
        assert!(matches!(cloned, SoulEvent::EventLogged { .. }));
    }

    #[test]
    fn soul_summary_constructs() {
        let summary = SoulSummary {
            total_events: 100,
            last_event_time: Some(SystemTime::now()),
            identity_signal_count: 5,
        };
        assert_eq!(summary.total_events, 100);
    }
}
