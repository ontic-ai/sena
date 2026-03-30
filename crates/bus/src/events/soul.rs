//! Soul subsystem events: event log writes, summaries, identity signals.

use std::time::SystemTime;

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
}

/// Summary of recent Soul events, passed to PromptComposer.
#[derive(Debug, Clone)]
pub struct SoulSummary {
    pub content: String,
    pub event_count: usize,
    pub request_id: u64,
}

/// An identity signal derived from behavioral patterns.
#[derive(Debug, Clone)]
pub struct IdentitySignalEmitted {
    pub key: String,
    pub value: String,
    pub timestamp: SystemTime,
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
    fn soul_event_all_variants_clone() {
        let now = SystemTime::now();
        let events = vec![
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
                    inference_cycle_count: 42,
                    work_patterns: vec![],
                    tool_preferences: vec![],
                    interest_clusters: vec![],
                },
                request_id: 3,
            }),
        ];
        assert_eq!(events.iter().cloned().count(), 7);
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
