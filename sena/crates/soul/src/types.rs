//! Soul subsystem type definitions.

use serde::{Deserialize, Serialize};
use std::time::SystemTime;

/// Summary of recent Soul events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoulSummary {
    /// Assembled summary content.
    pub content: String,
    /// Number of events included in summary.
    pub event_count: usize,
}

/// Single event in the Soul event log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoulEventRecord {
    /// Unique row ID.
    pub row_id: u64,
    /// Event description.
    pub description: String,
    /// Optional application context.
    pub app_context: Option<String>,
    /// Event timestamp.
    pub timestamp: SystemTime,
}

/// Identity signal key-value pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentitySignal {
    /// Signal key (e.g., "voice::rate").
    pub key: String,
    /// Signal value.
    pub value: String,
}

/// Temporal pattern derived from behavioral observations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemporalPattern {
    /// Pattern type (e.g., "high_cadence_work").
    pub pattern_type: String,
    /// Pattern strength [0.0, 1.0].
    pub strength: f64,
    /// First observed.
    pub first_seen: SystemTime,
    /// Last observed.
    pub last_seen: SystemTime,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soul_summary_is_serializable() {
        let summary = SoulSummary {
            content: "test summary".to_string(),
            event_count: 5,
        };
        let json = serde_json::to_string(&summary).expect("serialization failed");
        assert!(json.contains("test summary"));
    }

    #[test]
    fn soul_event_record_is_cloneable() {
        let record = SoulEventRecord {
            row_id: 42,
            description: "test event".to_string(),
            app_context: Some("test_app".to_string()),
            timestamp: SystemTime::now(),
        };
        let cloned = record.clone();
        assert_eq!(cloned.row_id, 42);
    }

    #[test]
    fn identity_signal_is_serializable() {
        let signal = IdentitySignal {
            key: "voice::rate".to_string(),
            value: "1.2".to_string(),
        };
        let json = serde_json::to_string(&signal).expect("serialization failed");
        assert!(json.contains("voice::rate"));
    }
}
