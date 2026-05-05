//! Task inference placeholder for model-driven interpretation.
//!
//! CTP must not hardcode task meaning as a product behavior. Context is
//! forwarded through the bus to the inference actor, which asks the model to
//! interpret the user's activity.

use bus::events::ctp::{ContextSnapshot, EnrichedInferredTask};

/// Placeholder type retained so the CTP crate keeps an explicit task-inference boundary.
pub struct TaskInferenceEngine;

impl TaskInferenceEngine {
    pub fn new() -> Self {
        Self
    }

    /// Hardcoded task inference is intentionally disabled.
    pub fn infer(&self, _snapshot: &ContextSnapshot) -> Option<EnrichedInferredTask> {
        None
    }
}

impl Default for TaskInferenceEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bus::events::platform::{KeystrokeCadence, WindowContext};
    use std::time::{Duration, Instant};

    fn mock_snapshot() -> ContextSnapshot {
        let now = Instant::now();
        ContextSnapshot {
            active_app: WindowContext {
                app_name: "Code".to_string(),
                window_title: Some("main.rs".to_string()),
                bundle_id: None,
                timestamp: now,
            },
            recent_files: vec![],
            clipboard_digest: None,
            keystroke_cadence: KeystrokeCadence {
                events_per_minute: 120.0,
                burst_detected: false,
                idle_duration: Duration::from_secs(0),
                timestamp: now,
            },
            session_duration: Duration::from_secs(600),
            inferred_task: None,
            user_state: None,
            visual_context: None,
            timestamp: now,
            soul_identity_signal: None,
        }
    }

    #[test]
    fn infer_returns_none_until_model_path_is_wired() {
        let engine = TaskInferenceEngine::new();
        assert!(engine.infer(&mock_snapshot()).is_none());
    }
}
