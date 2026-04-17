//! User state classifier — derives cognitive state from context and patterns.

use bus::events::ctp::{ContextSnapshot, SignalPattern, SignalPatternType, UserState};

/// Classifies the user's current cognitive state from signals and patterns.
///
/// BONES stub: applies simple heuristics on keystroke cadence and detected patterns.
pub struct UserStateClassifier;

impl UserStateClassifier {
    /// Create a new user state classifier.
    pub fn new() -> Self {
        Self
    }

    /// Classify the user's current cognitive state.
    ///
    /// Returns a `UserState` derived from the snapshot signals and detected patterns.
    pub fn classify(&self, snapshot: &ContextSnapshot, patterns: &[SignalPattern]) -> UserState {
        let mut frustration_level: u8 = 0;
        let mut flow_detected = false;
        let mut context_switch_cost: u8 = 0;

        // Apply pattern-based adjustments
        for pattern in patterns {
            match pattern.pattern_type {
                SignalPatternType::Frustration => {
                    frustration_level =
                        frustration_level.saturating_add((pattern.confidence * 60.0) as u8);
                }
                SignalPatternType::FlowState => {
                    flow_detected = true;
                }
                SignalPatternType::Anomaly | SignalPatternType::Repetition => {
                    context_switch_cost =
                        context_switch_cost.saturating_add((pattern.confidence * 30.0) as u8);
                }
            }
        }

        // Keystroke idle → lower flow confidence
        let idle_secs = snapshot.keystroke_cadence.idle_duration.as_secs();
        if idle_secs > 120 {
            flow_detected = false;
        }

        UserState {
            frustration_level,
            flow_detected,
            context_switch_cost,
        }
    }
}

impl Default for UserStateClassifier {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bus::events::ctp::{ContextSnapshot, SignalPattern, SignalPatternType};
    use platform::{KeystrokeCadence, WindowContext};
    use std::time::{Duration, Instant};

    fn stub_snapshot() -> ContextSnapshot {
        ContextSnapshot {
            active_app: WindowContext {
                app_name: "TestApp".to_string(),
                window_title: None,
                bundle_id: None,
                timestamp: Instant::now(),
            },
            recent_files: Vec::new(),
            clipboard_digest: None,
            keystroke_cadence: KeystrokeCadence {
                events_per_minute: 60.0,
                burst_detected: false,
                idle_duration: Duration::from_secs(0),
                timestamp: Instant::now(),
            },
            session_duration: Duration::from_secs(100),
            inferred_task: None,
            user_state: None,
            visual_context: None,
            timestamp: Instant::now(),
            soul_identity_signal: None,
        }
    }

    #[test]
    fn classify_detects_flow_from_pattern() {
        let classifier = UserStateClassifier::new();
        let snapshot = stub_snapshot();
        let patterns = vec![SignalPattern {
            pattern_type: SignalPatternType::FlowState,
            confidence: 0.9,
            description: "sustained focused activity".to_string(),
        }];

        let state = classifier.classify(&snapshot, &patterns);
        assert!(state.flow_detected);
        assert_eq!(state.frustration_level, 0);
    }

    #[test]
    fn classify_no_patterns_produces_neutral_state() {
        let classifier = UserStateClassifier::new();
        let snapshot = stub_snapshot();
        let state = classifier.classify(&snapshot, &[]);
        assert!(!state.flow_detected);
        assert_eq!(state.frustration_level, 0);
    }
}
