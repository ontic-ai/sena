//! Identity signal distillation: extract persistent patterns from counters.

use bus::events::soul::DistilledIdentitySignal;
use std::collections::HashMap;

/// Distillation engine that watches identity signal counters and extracts
/// persistent patterns when significance thresholds are crossed.
pub(crate) struct DistillationEngine {
    /// Accumulated observations: signal_key -> count.
    observations: HashMap<String, u64>,
    /// Total event count for computing proportions.
    total_events: u64,
    /// Minimum event count before a signal can be considered for distillation.
    min_threshold: u64,
}

impl DistillationEngine {
    pub(crate) fn new() -> Self {
        Self {
            observations: HashMap::new(),
            total_events: 0,
            min_threshold: 20,
        }
    }

    /// Feed an identity signal observation to the engine.
    pub(crate) fn observe_identity_signal(&mut self, key: &str, value: &str) {
        // Only accumulate numeric counter values (these are the counters we distill)
        if let Ok(count) = value.parse::<u64>() {
            self.observations.insert(key.to_string(), count);
            self.total_events = self.total_events.saturating_add(count);
        }
    }

    /// Harvest any identity signals that have crossed significance thresholds.
    ///
    /// Returns signals ready to persist and broadcast.
    /// Clears internal state after harvest (signals are now owned by caller).
    pub(crate) fn harvest(&mut self) -> Vec<DistilledIdentitySignal> {
        if self.total_events == 0 {
            return Vec::new();
        }

        let mut signals = Vec::new();

        // Distill tool preferences
        let tool_prefs: Vec<_> = self
            .observations
            .iter()
            .filter(|(k, _)| k.starts_with("tool_pref::"))
            .map(|(k, v)| (k.clone(), *v))
            .collect();

        if let Some((top_tool_key, top_tool_count)) =
            tool_prefs.iter().max_by_key(|(_, count)| *count)
        {
            if *top_tool_count >= self.min_threshold {
                let tool_name = top_tool_key.trim_start_matches("tool_pref::");
                let confidence = (*top_tool_count as f32) / (self.total_events as f32);
                let confidence = confidence.min(1.0);
                signals.push(DistilledIdentitySignal {
                    signal_key: "frequent_app".to_string(),
                    signal_value: tool_name.to_string(),
                    confidence,
                    source_event_count: *top_tool_count as u32,
                });
            }
        }

        // Distill interest clusters
        for (key, count) in &self.observations {
            if key.starts_with("interest::") && *count >= self.min_threshold {
                let interest = key.trim_start_matches("interest::");
                let confidence = (*count as f32) / (self.total_events as f32);
                let confidence = confidence.clamp(0.5, 1.0); // Interests get min 0.5 confidence
                signals.push(DistilledIdentitySignal {
                    signal_key: format!("interest::{}", interest),
                    signal_value: interest.to_string(),
                    confidence,
                    source_event_count: *count as u32,
                });
            }
        }

        // Distill work patterns
        for (key, count) in &self.observations {
            if key.starts_with("work_pattern::") && *count >= self.min_threshold {
                let pattern = key.trim_start_matches("work_pattern::");
                let confidence = (*count as f32) / (self.total_events as f32);
                let confidence = confidence.clamp(0.6, 1.0);
                signals.push(DistilledIdentitySignal {
                    signal_key: format!("work_pattern::{}", pattern),
                    signal_value: pattern.to_string(),
                    confidence,
                    source_event_count: *count as u32,
                });
            }
        }

        // Clear state after harvest
        self.observations.clear();
        self.total_events = 0;

        signals
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distillation_below_threshold_produces_nothing() {
        let mut engine = DistillationEngine::new();
        engine.observe_identity_signal("tool_pref::code", "10");
        engine.observe_identity_signal("tool_pref::browser", "5");

        let signals = engine.harvest();
        assert!(signals.is_empty());
    }

    #[test]
    fn distillation_above_threshold_produces_signal() {
        let mut engine = DistillationEngine::new();
        engine.observe_identity_signal("tool_pref::code", "60");
        engine.observe_identity_signal("tool_pref::browser", "30");
        engine.observe_identity_signal("tool_pref::terminal", "10");

        let signals = engine.harvest();
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].signal_key, "frequent_app");
        assert_eq!(signals[0].signal_value, "code");
        assert!(signals[0].confidence > 0.5);
        assert_eq!(signals[0].source_event_count, 60);
    }

    #[test]
    fn distillation_extracts_multiple_signals() {
        let mut engine = DistillationEngine::new();
        engine.observe_identity_signal("tool_pref::code", "100");
        engine.observe_identity_signal("interest::rust", "50");
        engine.observe_identity_signal("interest::ai", "30");
        engine.observe_identity_signal("work_pattern::high_cadence_count", "40");

        let signals = engine.harvest();
        // Should get: frequent_app, interest::rust, interest::ai, work_pattern::high_cadence_count
        assert!(signals.len() >= 4);

        let has_app = signals.iter().any(|s| s.signal_key == "frequent_app");
        let has_rust = signals.iter().any(|s| s.signal_key == "interest::rust");
        let has_ai = signals.iter().any(|s| s.signal_key == "interest::ai");
        let has_cadence = signals
            .iter()
            .any(|s| s.signal_key == "work_pattern::high_cadence_count");

        assert!(has_app);
        assert!(has_rust);
        assert!(has_ai);
        assert!(has_cadence);
    }

    #[test]
    fn harvest_clears_state() {
        let mut engine = DistillationEngine::new();
        engine.observe_identity_signal("tool_pref::code", "100");

        let signals1 = engine.harvest();
        assert_eq!(signals1.len(), 1);

        // Second harvest should return empty (state was cleared)
        let signals2 = engine.harvest();
        assert!(signals2.is_empty());
    }

    #[test]
    fn non_numeric_values_ignored() {
        let mut engine = DistillationEngine::new();
        engine.observe_identity_signal("tool_pref::code", "not_a_number");
        engine.observe_identity_signal("tool_pref::browser", "also_not_number");

        let signals = engine.harvest();
        assert!(signals.is_empty());
    }
}
