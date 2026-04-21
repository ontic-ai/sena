//! Trigger gate — decides when CTP should emit a ThoughtEvent.

use bus::events::ctp::{ContextSnapshot, SignalPattern, SignalPatternType};
use std::time::{Duration, Instant};

/// Default minimum interval between consecutive thought events.
const DEFAULT_MIN_INTERVAL: Duration = Duration::from_secs(600); // 10 minutes

/// Evaluates whether CTP should emit a ThoughtEvent for a given snapshot.
pub struct TriggerGate {
    min_interval: Duration,
    sensitivity: f32,
    last_trigger: Option<Instant>,
    last_snapshot: Option<ContextSnapshot>,
}

impl TriggerGate {
    /// Create a new trigger gate with the given minimum interval.
    pub fn new(min_interval: Duration) -> Self {
        Self {
            min_interval,
            sensitivity: 0.5,
            last_trigger: None,
            last_snapshot: None,
        }
    }

    /// Set the sensitivity (0.0 = hardest to trigger; 1.0 = easiest to trigger).
    pub fn with_sensitivity(mut self, sensitivity: f32) -> Self {
        self.sensitivity = sensitivity.clamp(0.0, 1.0);
        self
    }

    /// Reset the trigger timer so the next interval check can fire immediately.
    pub fn reset(&mut self) {
        self.last_trigger = Some(Instant::now() - self.min_interval - Duration::from_secs(1));
    }

    /// Evaluate whether a ThoughtEvent should be emitted.
    ///
    /// The first call never fires. It establishes a baseline snapshot so later
    /// evaluations can reason about context changes instead of immediately
    /// triggering during startup.
    pub fn should_trigger(
        &mut self,
        snapshot: &ContextSnapshot,
        patterns: &[SignalPattern],
    ) -> bool {
        let now = Instant::now();

        let time_since_last = match self.last_trigger {
            None => {
                self.last_trigger = Some(now);
                self.last_snapshot = Some(snapshot.clone());
                return false;
            }
            Some(last) => now.duration_since(last),
        };

        let periodic_trigger = time_since_last >= self.min_interval;
        let periodic_bonus = ((time_since_last.as_secs_f32() / (30.0 * 60.0)) * 0.05).min(0.15);

        let base_score = self
            .last_snapshot
            .as_ref()
            .map(|previous| context_diff_score(previous, snapshot))
            .unwrap_or(0.0);

        let mut significance_bonus = 0.0;

        for pattern in patterns {
            match pattern.pattern_type {
                SignalPatternType::Frustration => significance_bonus += 0.20,
                SignalPatternType::Anomaly => significance_bonus += 0.15,
                SignalPatternType::Repetition => significance_bonus += 0.10,
                SignalPatternType::FlowState => significance_bonus += 0.05,
            }
        }

        if let Some(user_state) = &snapshot.user_state
            && user_state.context_switch_cost >= 60
        {
            significance_bonus += 0.10;
        }

        let total_score = (base_score + significance_bonus + periodic_bonus).min(1.0);
        let significance_trigger = total_score >= diff_threshold(self.sensitivity);

        self.last_snapshot = Some(snapshot.clone());

        if periodic_trigger || significance_trigger {
            self.last_trigger = Some(now);
            true
        } else {
            false
        }
    }
}

fn diff_threshold(sensitivity: f32) -> f32 {
    0.75 - (0.50 * sensitivity)
}

fn context_diff_score(previous: &ContextSnapshot, current: &ContextSnapshot) -> f32 {
    let mut score: f32 = 0.0;

    if previous.active_app.app_name != current.active_app.app_name {
        score += 0.55;
    }

    if previous.active_app.window_title != current.active_app.window_title {
        score += 0.15;
    }

    if previous.clipboard_digest != current.clipboard_digest {
        score += 0.10;
    }

    let file_delta = previous
        .recent_files
        .len()
        .abs_diff(current.recent_files.len());
    if file_delta >= 2 {
        score += 0.10;
    }

    if current.keystroke_cadence.burst_detected
        && current.keystroke_cadence.idle_duration >= Duration::from_secs(45)
    {
        score += 0.20;
    }

    if current.keystroke_cadence.events_per_minute >= 180.0 {
        score += 0.10;
    }

    if inferred_task_changed(previous, current) {
        score += 0.30;
    }

    score.min(1.0)
}

fn inferred_task_changed(previous: &ContextSnapshot, current: &ContextSnapshot) -> bool {
    match (&previous.inferred_task, &current.inferred_task) {
        (Some(previous_task), Some(current_task)) => previous_task != current_task,
        (None, None) => false,
        _ => true,
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
    use bus::events::ctp::{EnrichedInferredTask, UserState};
    use platform::{KeystrokeCadence, WindowContext};
    use std::thread::sleep;

    fn snapshot(app_name: &str) -> ContextSnapshot {
        let now = Instant::now();
        ContextSnapshot {
            active_app: WindowContext {
                app_name: app_name.to_string(),
                window_title: None,
                bundle_id: None,
                timestamp: now,
            },
            recent_files: Vec::new(),
            clipboard_digest: None,
            keystroke_cadence: KeystrokeCadence {
                events_per_minute: 0.0,
                burst_detected: false,
                idle_duration: Duration::from_secs(0),
                timestamp: now,
            },
            session_duration: Duration::from_secs(10),
            inferred_task: None,
            user_state: None,
            visual_context: None,
            timestamp: now,
            soul_identity_signal: None,
        }
    }

    #[test]
    fn first_check_does_not_trigger_warm_up_guard() {
        let mut gate = TriggerGate::new(Duration::from_secs(5));
        assert!(!gate.should_trigger(&snapshot("Code"), &[]));
    }

    #[test]
    fn check_after_interval_triggers() {
        let mut gate = TriggerGate::new(Duration::from_millis(100));

        assert!(!gate.should_trigger(&snapshot("Code"), &[]));

        sleep(Duration::from_millis(150));

        assert!(gate.should_trigger(&snapshot("Code"), &[]));
    }

    #[test]
    fn reset_allows_immediate_trigger() {
        let mut gate = TriggerGate::new(Duration::from_secs(10));

        assert!(!gate.should_trigger(&snapshot("Code"), &[]));
        assert!(!gate.should_trigger(&snapshot("Code"), &[]));

        gate.reset();

        assert!(gate.should_trigger(&snapshot("Code"), &[]));
    }

    #[test]
    fn context_switch_triggers_without_waiting_interval() {
        let mut gate = TriggerGate::new(Duration::from_secs(9999));

        assert!(!gate.should_trigger(&snapshot("Code"), &[]));
        assert!(gate.should_trigger(&snapshot("Browser"), &[]));
    }

    #[test]
    fn task_change_contributes_to_diff_score() {
        let mut gate = TriggerGate::new(Duration::from_secs(9999)).with_sensitivity(1.0);
        let mut first = snapshot("Code");
        first.inferred_task = Some(EnrichedInferredTask {
            category: "coding".to_string(),
            semantic_description: "Writing code".to_string(),
            confidence: 0.8,
        });

        let mut second = snapshot("Code");
        second.inferred_task = Some(EnrichedInferredTask {
            category: "research".to_string(),
            semantic_description: "Reading documentation".to_string(),
            confidence: 0.7,
        });

        assert!(!gate.should_trigger(&first, &[]));
        assert!(gate.should_trigger(&second, &[]));
    }

    #[test]
    fn context_switch_cost_bonus_contributes_to_trigger() {
        let mut gate = TriggerGate::new(Duration::from_secs(9999)).with_sensitivity(1.0);
        let first = snapshot("Code");
        let mut second = snapshot("Code");
        second.user_state = Some(UserState {
            frustration_level: 0,
            flow_detected: false,
            context_switch_cost: 80,
        });
        let anomaly_pattern = SignalPattern {
            pattern_type: SignalPatternType::Anomaly,
            confidence: 0.60,
            description: "Cadence spike".to_string(),
        };

        assert!(!gate.should_trigger(&first, &[]));
        assert!(gate.should_trigger(&second, &[anomaly_pattern]));
    }
}
