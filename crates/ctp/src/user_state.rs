//! User state classification from behavioral signals.

use bus::events::ctp::{ContextSnapshot, SignalPattern, SignalPatternType, UserState};

/// Classifier for user cognitive state.
///
/// Computes frustration level, flow detection, and context switch cost
/// based on the current context snapshot and detected signal patterns.
pub struct UserStateClassifier;

impl UserStateClassifier {
    /// Create a new user state classifier.
    pub fn new() -> Self {
        Self
    }

    /// Compute user state from latest snapshot and detected patterns.
    ///
    /// # Classification Rules
    /// - `frustration_level` (0-100): base 10, +25 if Frustration pattern,
    ///   +10 per app switch in buffer, +15 if clipboard changed >=5 times. Clamp 0-100.
    /// - `flow_detected`: true if FlowState pattern OR (cadence 80-140 EPM + no frustration/anomaly)
    /// - `context_switch_cost` (0-100): based on app switch count. 0 switches → 0, >=10 switches → 100.
    pub fn classify(
        &mut self,
        snapshot: &ContextSnapshot,
        patterns: &[SignalPattern],
    ) -> UserState {
        // Compute frustration level
        let mut frustration_level = 10;

        // Check for frustration pattern
        if patterns
            .iter()
            .any(|p| matches!(p.pattern_type, SignalPatternType::Frustration))
        {
            frustration_level += 25;
        }

        // Add frustration based on app switches (infer from recent_files as proxy for activity)
        // Since we don't have direct app switch count in snapshot, estimate from context
        let app_switch_penalty = self.estimate_app_switches(snapshot);
        frustration_level += app_switch_penalty * 10;

        // Add frustration if clipboard changed frequently
        // We don't have clipboard change count in snapshot, so use a heuristic
        if snapshot.clipboard_digest.is_some() && snapshot.keystroke_cadence.burst_detected {
            frustration_level += 15;
        }

        // Clamp to 0-100
        let frustration_level = frustration_level.min(100) as u8;

        // Detect flow state
        let has_flow_pattern = patterns
            .iter()
            .any(|p| matches!(p.pattern_type, SignalPatternType::FlowState));

        let has_frustration = patterns
            .iter()
            .any(|p| matches!(p.pattern_type, SignalPatternType::Frustration));

        let has_anomaly = patterns
            .iter()
            .any(|p| matches!(p.pattern_type, SignalPatternType::Anomaly));

        let cadence = snapshot.keystroke_cadence.events_per_minute;
        let in_flow_cadence = (80.0..=140.0).contains(&cadence);

        let flow_detected =
            has_flow_pattern || (in_flow_cadence && !has_frustration && !has_anomaly);

        // Compute context switch cost
        let switch_count = self.estimate_app_switches(snapshot);
        let context_switch_cost = if switch_count == 0 {
            0
        } else if switch_count >= 10 {
            100
        } else {
            // Linear interpolation: 0 switches → 0, 10 switches → 100
            ((switch_count as f64 / 10.0) * 100.0) as u8
        };

        UserState {
            frustration_level,
            flow_detected,
            context_switch_cost,
        }
    }

    /// Estimate app switches from snapshot context.
    ///
    /// This is a heuristic based on available information in the snapshot.
    /// Uses recent_files count as a proxy for activity level.
    fn estimate_app_switches(&self, snapshot: &ContextSnapshot) -> usize {
        // Use file event count as a rough proxy for context switching
        // More file events often correlate with switching between different tasks
        let file_event_count = snapshot.recent_files.len();

        // Heuristic: every 2 file events suggests a potential app switch
        (file_event_count / 2).min(15)
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
    use bus::events::platform::{FileEvent, FileEventKind, KeystrokeCadence, WindowContext};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    fn mock_snapshot(cadence: f64, burst: bool, file_count: usize) -> ContextSnapshot {
        let now = Instant::now();
        ContextSnapshot {
            active_app: WindowContext {
                app_name: "Code".to_string(),
                window_title: None,
                bundle_id: None,
                timestamp: now,
            },
            recent_files: (0..file_count)
                .map(|i| FileEvent {
                    path: PathBuf::from(format!("/path/to/file{}.rs", i)),
                    event_kind: FileEventKind::Modified,
                    timestamp: now,
                })
                .collect(),
            clipboard_digest: None,
            keystroke_cadence: KeystrokeCadence {
                events_per_minute: cadence,
                burst_detected: burst,
                idle_duration: Duration::from_secs(0),
                timestamp: now,
            },
            session_duration: Duration::from_secs(600),
            inferred_task: None,
            user_state: None,
            visual_context: None,
            timestamp: now,
        }
    }

    #[test]
    fn calm_state_low_frustration() {
        let mut classifier = UserStateClassifier::new();
        let snapshot = mock_snapshot(90.0, false, 0);
        let patterns = vec![];

        let state = classifier.classify(&snapshot, &patterns);

        assert_eq!(state.frustration_level, 10); // base level
        assert!(state.flow_detected); // cadence in flow range
        assert_eq!(state.context_switch_cost, 0); // no switches
    }

    #[test]
    fn frustrated_state_with_pattern() {
        let mut classifier = UserStateClassifier::new();
        let snapshot = mock_snapshot(180.0, true, 8);

        let patterns = vec![SignalPattern {
            pattern_type: SignalPatternType::Frustration,
            confidence: 0.70,
            description: "Frustration detected".to_string(),
        }];

        let state = classifier.classify(&snapshot, &patterns);

        // Base 10 + 25 (frustration pattern) + 15 (clipboard burst) + 4*10 (file switches)
        assert!(state.frustration_level >= 50);
        assert!(!state.flow_detected); // high cadence, out of flow range
        assert!(state.context_switch_cost > 0);
    }

    #[test]
    fn flow_state_detected_from_pattern() {
        let mut classifier = UserStateClassifier::new();
        let snapshot = mock_snapshot(120.0, false, 2);

        let patterns = vec![SignalPattern {
            pattern_type: SignalPatternType::FlowState,
            confidence: 0.75,
            description: "Flow state".to_string(),
        }];

        let state = classifier.classify(&snapshot, &patterns);

        assert!(state.flow_detected); // explicit flow pattern
        assert!(state.frustration_level < 50);
    }

    #[test]
    fn flow_state_from_cadence_without_pattern() {
        let mut classifier = UserStateClassifier::new();
        let snapshot = mock_snapshot(100.0, false, 0);
        let patterns = vec![]; // No patterns, but cadence in flow range

        let state = classifier.classify(&snapshot, &patterns);

        assert!(state.flow_detected); // inferred from cadence
        assert_eq!(state.frustration_level, 10);
        assert_eq!(state.context_switch_cost, 0);
    }

    #[test]
    fn high_context_switch_cost() {
        let mut classifier = UserStateClassifier::new();
        let snapshot = mock_snapshot(80.0, false, 20); // Many file events

        let patterns = vec![];

        let state = classifier.classify(&snapshot, &patterns);

        // 20 file events → ~10 estimated switches → 100 cost
        assert_eq!(state.context_switch_cost, 100);
        assert!(state.frustration_level >= 10);
    }

    #[test]
    fn anomaly_blocks_flow_detection() {
        let mut classifier = UserStateClassifier::new();
        let snapshot = mock_snapshot(110.0, false, 0); // cadence in flow range

        let patterns = vec![SignalPattern {
            pattern_type: SignalPatternType::Anomaly,
            confidence: 0.60,
            description: "Anomaly".to_string(),
        }];

        let state = classifier.classify(&snapshot, &patterns);

        assert!(!state.flow_detected); // anomaly blocks flow detection
    }
}
