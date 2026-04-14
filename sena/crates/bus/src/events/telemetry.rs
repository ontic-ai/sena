//! Telemetry and metrics events.

use std::time::Duration;

/// Telemetry and metrics events.
#[derive(Debug, Clone)]
pub enum TelemetryEvent {
    /// Memory usage snapshot.
    MemoryUsage {
        /// Memory usage in megabytes.
        usage_mb: usize,
        /// Memory limit in megabytes.
        limit_mb: usize,
    },

    /// Inference performance metric.
    InferenceMetric {
        /// Tokens generated.
        token_count: usize,
        /// Time taken for inference.
        duration: Duration,
        /// Tokens per second.
        tokens_per_second: f64,
    },

    /// Actor lifecycle event.
    ActorLifecycle {
        actor_name: &'static str,
        event_type: ActorLifecycleEventType,
    },
}

/// Type of actor lifecycle event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActorLifecycleEventType {
    Started,
    Stopped,
    Failed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_events_are_cloneable() {
        let event = TelemetryEvent::MemoryUsage {
            usage_mb: 512,
            limit_mb: 1024,
        };
        let cloned = event.clone();
        assert!(matches!(cloned, TelemetryEvent::MemoryUsage { .. }));
    }

    #[test]
    fn actor_lifecycle_event_types() {
        assert_eq!(
            ActorLifecycleEventType::Started,
            ActorLifecycleEventType::Started
        );
        assert_ne!(
            ActorLifecycleEventType::Started,
            ActorLifecycleEventType::Stopped
        );
    }
}
