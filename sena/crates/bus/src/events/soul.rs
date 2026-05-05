//! Soul subsystem events: event log writes, summaries, identity signals.

use crate::causal::CausalId;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

/// Compact summary of Soul state for bus communication and prompt composition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoulSummary {
    /// Assembled summary content from the Soul event log.
    pub content: String,
    /// Number of events included in the summary.
    pub event_count: usize,
    /// Correlation ID linking this summary to its originating request.
    pub request_id: u64,
}

/// Response verbosity preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Verbosity {
    Terse,
    #[default]
    Balanced,
    Verbose,
}

impl Verbosity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Terse => "terse",
            Self::Balanced => "balanced",
            Self::Verbose => "verbose",
        }
    }
}

/// Response warmth preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Warmth {
    Professional,
    #[default]
    Friendly,
    Casual,
}

impl Warmth {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Professional => "professional",
            Self::Friendly => "friendly",
            Self::Casual => "casual",
        }
    }
}

/// Type of a section within a rich soul summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SoulSectionType {
    /// Recently observed events.
    RecentEvents,
    /// Distilled identity signals (preferences, style, habits).
    IdentitySignals,
    /// Temporal behaviour patterns (time-of-day, cadence).
    TemporalHabits,
    /// Explicit user preferences.
    Preferences,
}

/// A single section within a [`RichSoulSummary`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoulSection {
    /// Section type.
    pub section_type: SoulSectionType,
    /// Text content of the section.
    pub content: String,
    /// Relevance score for the current context (0.0–1.0).
    pub relevance_score: f64,
}

/// Rich, structured soul summary with multiple sections and token budget tracking.
///
/// Used by the prompt subsystem when it wants finer-grained control over which
/// parts of the Soul context to include within a token budget.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RichSoulSummary {
    /// Ordered list of content sections.
    pub sections: Vec<SoulSection>,
    /// Estimated token count across all sections.
    pub token_count: usize,
    /// Correlation ID linking this summary to its originating request.
    pub request_id: u64,
}

/// Personality metadata for prompt composition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonalityMetadata {
    /// Verbosity preference.
    pub verbosity: Verbosity,
    /// Response warmth.
    pub warmth: Warmth,
    /// Work cadence preference.
    pub work_cadence: WorkCadence,
}

/// Work cadence preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum WorkCadence {
    Burst,
    #[default]
    Steady,
    LongFocus,
}

impl WorkCadence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Burst => "burst",
            Self::Steady => "steady",
            Self::LongFocus => "long_focus",
        }
    }
}

/// Distilled identity signal from Soul for CTP/prompt composition.
/// Privacy-safe typed representation of learned identity attributes.
#[derive(Clone, Serialize, Deserialize)]
pub struct DistilledIdentitySignal {
    /// The signal's semantic key (e.g., "voice::rate", "work_style::preferred_cadence").
    pub signal_key: String,
    /// The signal's value.
    pub signal_value: String,
    /// Confidence in this signal's accuracy (0.0 to 1.0).
    pub confidence: f32,
}

impl std::fmt::Debug for DistilledIdentitySignal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DistilledIdentitySignal")
            .field("signal_key", &self.signal_key)
            .field("signal_value", &"[REDACTED]")
            .field("confidence", &self.confidence)
            .finish()
    }
}

/// Temporal behavior pattern detected by Soul.
/// Represents stable/recurring patterns in user's temporal behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
            content: "Recent activity summary".to_string(),
            event_count: 100,
            request_id: 42,
        };
        assert_eq!(summary.event_count, 100);
    }

    #[test]
    fn rich_soul_summary_constructs() {
        let section = SoulSection {
            section_type: SoulSectionType::RecentEvents,
            content: "user was coding".to_string(),
            relevance_score: 0.9,
        };
        let rich = RichSoulSummary {
            sections: vec![section],
            token_count: 10,
            request_id: 1,
        };
        assert_eq!(rich.sections.len(), 1);
        assert_eq!(rich.token_count, 10);
    }
}
