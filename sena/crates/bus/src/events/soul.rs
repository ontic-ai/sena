//! Soul subsystem events: event log writes, summaries, identity signals.

use crate::causal::CausalId;
use std::time::SystemTime;

/// Soul subsystem events.
#[derive(Debug, Clone)]
pub enum SoulEvent {
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
}

impl SoulEvent {
    pub fn causal_id(&self) -> Option<CausalId> {
        match self {
            Self::WriteRequested { causal_id, .. }
            | Self::EventLogged { causal_id, .. }
            | Self::SummaryRequested { causal_id, .. }
            | Self::SummaryCompleted { causal_id, .. }
            | Self::OperationFailed { causal_id, .. } => Some(*causal_id),
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
}
