//! Soul subsystem events: event log writes, summaries, identity signals.

use std::time::SystemTime;

/// Emitted by onboarding wizard: Soul actor will persist user's name.
#[derive(Debug, Clone)]
pub struct SoulNameInitialized {
    /// The name stored successfully.
    pub name: String,
}

/// A structured event to be written to the Soul event log.
#[derive(Debug, Clone)]
pub struct SoulWriteRequest {
    pub description: String,
    pub app_context: Option<String>,
    pub timestamp: SystemTime,
    pub request_id: u64,
}

/// Emitted after a Soul event log write completes successfully.
#[derive(Debug, Clone)]
pub struct SoulEventLogged {
    pub row_id: u64,
    pub request_id: u64,
}

/// Request for a structured summary of recent Soul events.
#[derive(Debug, Clone)]
pub struct SoulSummaryRequested {
    pub max_events: usize,
    pub request_id: u64,
    /// Optional maximum character count for the summary content.
    /// When set, content is truncated with a `...[truncated]` suffix.
    pub max_chars: Option<usize>,
}

/// Summary of recent Soul events, passed to PromptComposer.
#[derive(Clone)]
pub struct SoulSummary {
    pub content: String,
    pub event_count: usize,
    pub request_id: u64,
}

impl std::fmt::Debug for SoulSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SoulSummary")
            .field("content", &"[REDACTED]")
            .field("event_count", &self.event_count)
            .field("request_id", &self.request_id)
            .finish()
    }
}

/// An identity signal derived from behavioral patterns.
#[derive(Debug, Clone)]
pub struct IdentitySignalEmitted {
    pub key: String,
    pub value: String,
    pub timestamp: SystemTime,
}

/// TTS personality parameters derived from Soul identity state.
///
/// Emitted after boot completes and after every identity signal update that
/// affects voice personality. The speech TTS actor subscribes to this event
/// and adjusts synthesis parameters accordingly.
///
/// - `rate`: speaking rate multiplier (0.5 = slow, 1.0 = normal, 2.0 = fast).
///   Driven by the user's observed work cadence preference.
/// - `warmth`: [0, 100] scale. Higher values produce softer tonal inflection.
///   Driven by `response_warmth` soul preference.
/// - `verbosity`: [0, 100] scale. Higher values allow longer spoken responses.
///   Driven by `verbosity_preference` soul preference.
#[derive(Debug, Clone, PartialEq)]
pub struct PersonalityUpdated {
    pub rate: f32,
    pub warmth: u8,
    pub verbosity: u8,
}

/// Distilled identity signal extracted from behavioral patterns.
#[derive(Debug, Clone)]
pub struct DistilledIdentitySignal {
    /// The signal's semantic key (e.g., "preferred_editor", "work_start_time").
    pub signal_key: String,
    /// The signal's value (e.g., "vscode", "09:00").
    pub signal_value: String,
    /// Confidence in this signal's accuracy (0.0 to 1.0).
    pub confidence: f32,
    /// Number of source events that contributed to this signal.
    pub source_event_count: u32,
}

/// Temporal behavior pattern derived from event log analysis.
#[derive(Debug, Clone)]
pub struct TemporalBehaviorPattern {
    /// Hour of day (0-23), if time-of-day is relevant.
    pub hour_of_day: Option<u8>,
    /// Day of week (0=Monday, 6=Sunday), if day-of-week is relevant.
    pub day_of_week: Option<u8>,
    /// Semantic category of the behavior (e.g., "deep_work", "meetings").
    pub behavior_category: String,
    /// How often this pattern occurs (event count).
    pub frequency: u32,
}

/// User engagement signal in response to Sena's proactive inference.
#[derive(Debug, Clone)]
pub enum EngagementSignal {
    /// User explicitly accepted or acted on the response.
    Accepted,
    /// User ignored the response (no interaction within timeout).
    Ignored,
    /// User interrupted or dismissed the response.
    Interrupted,
    /// User asked a follow-up question.
    FollowUpQuery,
}

/// Type of Soul content section in a rich summary.
#[derive(Debug, Clone)]
pub enum SoulSectionType {
    /// Recent event log entries.
    RecentEvents,
    /// Distilled identity signals.
    IdentitySignals,
    /// Temporal habit patterns.
    TemporalHabits,
    /// User preferences.
    Preferences,
}

/// A single section of a rich Soul summary.
#[derive(Debug, Clone)]
pub struct SoulSection {
    /// Type of content in this section.
    pub section_type: SoulSectionType,
    /// The actual content text.
    pub content: String,
    /// Relevance score for prompt prioritization (0.0 to 1.0).
    pub relevance_score: f32,
}

/// Rich structured summary of Soul state for prompt composition.
#[derive(Clone)]
pub struct RichSoulSummary {
    /// Ordered sections (highest relevance first).
    pub sections: Vec<SoulSection>,
    /// Total token count (approximate).
    pub token_count: usize,
    /// Request ID for correlation.
    pub request_id: u64,
}

impl std::fmt::Debug for RichSoulSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RichSoulSummary")
            .field("sections", &format!("{} sections", self.sections.len()))
            .field("token_count", &self.token_count)
            .field("request_id", &self.request_id)
            .finish()
    }
}

/// Request for a transparency view of the Soul state for transparency queries.
#[derive(Debug, Clone)]
pub struct SoulReadRequest {
    pub request_id: u64,
}

/// Response to SoulReadRequest: the transparency view of Soul state.
/// Includes only high-level aggregates, never raw identity data.
#[derive(Debug, Clone)]
pub struct SoulReadCompleted {
    /// High-level summary for transparency purposes.
    pub summary: crate::events::transparency::SoulSummaryForTransparency,
    pub request_id: u64,
}

/// Top-level soul event enum wrapping all Soul subsystem events.
#[derive(Debug, Clone)]
pub enum SoulEvent {
    WriteRequested(SoulWriteRequest),
    EventLogged(SoulEventLogged),
    SummaryRequested(SoulSummaryRequested),
    SummaryReady(SoulSummary),
    IdentitySignalEmitted(IdentitySignalEmitted),
    ReadRequested(SoulReadRequest),
    ReadCompleted(SoulReadCompleted),
    /// Request Soul to store the user's chosen name. Emitted during first-boot onboarding only.
    InitializeWithName {
        name: String,
    },
    /// Soul actor confirms name was persisted.
    NameInitialized(SoulNameInitialized),
    /// TTS personality parameters derived from Soul state.
    /// Emitted at boot completion and after identity signal updates.
    PersonalityUpdated(PersonalityUpdated),
    /// Request Soul to export its event log and identity signals to a file.
    /// Path is the target export file (JSON).
    ExportRequested {
        /// Target path for the exported JSON file.
        path: std::path::PathBuf,
    },
    /// Soul export completed. Contains path to the exported file.
    ExportCompleted {
        /// Path to the exported file.
        path: std::path::PathBuf,
    },
    /// Soul export failed.
    ExportFailed {
        /// Failure reason.
        reason: String,
    },
    /// A distilled identity signal has been extracted.
    IdentitySignalDistilled(DistilledIdentitySignal),
    /// A temporal behavior pattern has been detected.
    TemporalPatternDetected(TemporalBehaviorPattern),
    /// User engagement signal in response to a proactive inference.
    PreferenceLearningUpdate {
        /// ID of the response that triggered this signal.
        response_id: u64,
        /// The engagement signal type.
        signal: EngagementSignal,
    },
    /// Request for a rich, structured Soul summary.
    RichSummaryRequested {
        /// Token budget for the summary.
        token_budget: usize,
        /// Request ID for correlation.
        request_id: u64,
    },
    /// Rich summary is ready.
    RichSummaryReady(RichSoulSummary),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soul_write_request_constructs_and_clones() {
        let r = SoulWriteRequest {
            description: "d".into(),
            app_context: None,
            timestamp: SystemTime::now(),
            request_id: 1,
        };
        assert_eq!(r.clone().request_id, 1);
    }

    #[test]
    fn soul_summary_debug_redacts_content() {
        let summary = SoulSummary {
            content: "user identity data and private events".into(),
            event_count: 10,
            request_id: 99,
        };
        let debug_output = format!("{:?}", summary);
        assert!(debug_output.contains("[REDACTED]"));
        assert!(!debug_output.contains("user identity data"));
        assert!(!debug_output.contains("private events"));
        assert!(debug_output.contains("10"));
        assert!(debug_output.contains("99"));
    }

    #[test]
    fn soul_event_all_variants_clone() {
        let now = SystemTime::now();
        let events = [
            SoulEvent::WriteRequested(SoulWriteRequest {
                description: "d".into(),
                app_context: None,
                timestamp: now,
                request_id: 1,
            }),
            SoulEvent::EventLogged(SoulEventLogged {
                row_id: 1,
                request_id: 1,
            }),
            SoulEvent::SummaryRequested(SoulSummaryRequested {
                max_events: 5,
                request_id: 2,
                max_chars: None,
            }),
            SoulEvent::SummaryReady(SoulSummary {
                content: "c".into(),
                event_count: 1,
                request_id: 2,
            }),
            SoulEvent::IdentitySignalEmitted(IdentitySignalEmitted {
                key: "k".into(),
                value: "v".into(),
                timestamp: now,
            }),
            SoulEvent::ReadRequested(SoulReadRequest { request_id: 3 }),
            SoulEvent::ReadCompleted(SoulReadCompleted {
                summary: crate::events::transparency::SoulSummaryForTransparency {
                    user_name: None,
                    inference_cycle_count: 42,
                    work_patterns: vec![],
                    tool_preferences: vec![],
                    interest_clusters: vec![],
                },
                request_id: 3,
            }),
        ];
        assert_eq!(events.iter().count(), 7);
    }

    fn assert_send_static<T: Send + 'static>() {}

    #[test]
    fn all_soul_types_are_send_and_static() {
        assert_send_static::<SoulWriteRequest>();
        assert_send_static::<SoulEventLogged>();
        assert_send_static::<SoulSummaryRequested>();
        assert_send_static::<SoulSummary>();
        assert_send_static::<IdentitySignalEmitted>();
        assert_send_static::<SoulReadRequest>();
        assert_send_static::<SoulReadCompleted>();
        assert_send_static::<SoulEvent>();
    }
}
