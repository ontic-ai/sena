//! Trigger gate — decides when CTP should emit a ThoughtEvent.
//!
//! The trigger gate enforces a minimum interval between thought events and
//! applies a sensitivity threshold to filter low-significance snapshots.
//! Higher sensitivity → more frequent triggers.

use bus::events::ctp::{ContextSnapshot, SignalPattern};
use std::time::{Duration, Instant};
use tracing::debug;

/// Default minimum interval between consecutive thought events.
const DEFAULT_MIN_INTERVAL: Duration = Duration::from_secs(600); // 10 minutes

/// Evaluates whether CTP should emit a ThoughtEvent for a given snapshot.
pub struct TriggerGate {
    /// Minimum time that must elapse between consecutive thought events.
    min_interval: Duration,
    /// Sensitivity threshold [0.0, 1.0]. Higher = triggers more easily.
    sensitivity: f32,
    /// Timestamp of the last emitted thought event.
    last_trigger: Option<Instant>,
}

impl TriggerGate {
    /// Create a new trigger gate with the given minimum interval.
    pub fn new(min_interval: Duration) -> Self {
        Self {
            min_interval,
            sensitivity: 0.5,
            last_trigger: None,
        }
    }

    /// Set the sensitivity (0.0 = never trigger; 1.0 = always trigger).
    pub fn with_sensitivity(mut self, sensitivity: f32) -> Self {
        self.sensitivity = sensitivity.clamp(0.0, 1.0);
        self
    }

    /// Evaluate whether a ThoughtEvent should be emitted.
    ///
    /// Returns `true` if:
    /// 1. The minimum interval since the last trigger has elapsed, AND
    /// 2. The snapshot's significance score exceeds the sensitivity threshold.
    pub fn should_trigger(
        &mut self,
        snapshot: &ContextSnapshot,
        patterns: &[SignalPattern],
    ) -> bool {
        // Enforce minimum interval
        if let Some(last) = self.last_trigger
            && last.elapsed() < self.min_interval
        {
            debug!(
                elapsed_secs = last.elapsed().as_secs(),
                min_secs = self.min_interval.as_secs(),
                "trigger gate: cooldown not elapsed"
            );
            return false;
        }

        // Compute significance score from snapshot signals
        let significance = self.compute_significance(snapshot, patterns);
        let threshold = 1.0 - self.sensitivity; // inverted: higher sensitivity → lower threshold

        if significance >= threshold {
            debug!(
                significance = significance,
                threshold = threshold,
                "trigger gate: TRIGGERED"
            );
            self.last_trigger = Some(Instant::now());
            true
        } else {
            debug!(
                significance = significance,
                threshold = threshold,
                "trigger gate: below threshold"
            );
            false
        }
    }

    /// Compute a significance score [0.0, 1.0] from the snapshot and patterns.
    ///
    /// BONES stub: uses simple heuristics based on task confidence and pattern count.
    fn compute_significance(&self, snapshot: &ContextSnapshot, patterns: &[SignalPattern]) -> f32 {
        let mut score: f32 = 0.0;

        // Any non-empty active app context indicates meaningful activity.
        if !snapshot.active_app.app_name.trim().is_empty() {
            score += 0.6;
        }

        // Boost score if an inferred task is present
        if let Some(task) = &snapshot.inferred_task {
            score += task.confidence * 0.4;
        }

        // Boost score based on keystroke activity
        let epm = snapshot.keystroke_cadence.events_per_minute as f32;
        let activity_score = (epm / 120.0).min(1.0) * 0.3;
        score += activity_score;

        // Boost score for each detected pattern
        for pattern in patterns {
            score += pattern.confidence * 0.1;
        }

        score.min(1.0)
    }
}

impl Default for TriggerGate {
    fn default() -> Self {
        Self::new(DEFAULT_MIN_INTERVAL)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bus::events::ctp::ContextSnapshot;
    use platform::{KeystrokeCadence, WindowContext};
    use std::time::Duration;

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
                events_per_minute: 0.0,
                burst_detected: false,
                idle_duration: Duration::from_secs(0),
                timestamp: Instant::now(),
            },
            session_duration: Duration::from_secs(0),
            inferred_task: None,
            user_state: None,
            visual_context: None,
            timestamp: Instant::now(),
            soul_identity_signal: None,
        }
    }

    #[test]
    fn trigger_gate_does_not_trigger_with_zero_sensitivity() {
        let mut gate = TriggerGate::new(Duration::from_secs(0)).with_sensitivity(0.0);
        let snapshot = stub_snapshot();
        assert!(!gate.should_trigger(&snapshot, &[]));
    }

    #[test]
    fn trigger_gate_triggers_with_full_sensitivity() {
        let mut gate = TriggerGate::new(Duration::from_secs(0)).with_sensitivity(1.0);
        let snapshot = stub_snapshot();
        assert!(gate.should_trigger(&snapshot, &[]));
    }

    #[test]
    fn trigger_gate_respects_min_interval() {
        let mut gate = TriggerGate::new(Duration::from_secs(9999)).with_sensitivity(1.0);
        let snapshot = stub_snapshot();
        assert!(gate.should_trigger(&snapshot, &[]));
        // Second call should be blocked by cooldown
        assert!(!gate.should_trigger(&snapshot, &[]));
    }
}
