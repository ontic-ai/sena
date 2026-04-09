//! Time-based trigger gate for thought events.

use std::time::{Duration, Instant};

use bus::events::ctp::ContextSnapshot;

/// Context-aware trigger gate for CTP thought events.
///
/// Phase 3 implementation combines:
/// - periodic reflection interval fallback
/// - context-diff score between snapshots
///
/// Lower sensitivity means a higher diff score is required.
pub struct TriggerGate {
    interval: Duration,
    last_trigger: Option<Instant>,
    sensitivity: f64,
    last_snapshot: Option<ContextSnapshot>,
}

impl TriggerGate {
    /// Create a new trigger gate with the specified interval.
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last_trigger: None,
            sensitivity: 0.5,
            last_snapshot: None,
        }
    }

    /// Set trigger sensitivity in [0.0, 1.0].
    ///
    /// Lower values require stronger context changes.
    pub fn with_sensitivity(mut self, sensitivity: f64) -> Self {
        self.sensitivity = sensitivity.clamp(0.0, 1.0);
        self
    }

    /// Mutate sensitivity in place.
    pub fn set_sensitivity(&mut self, sensitivity: f64) {
        self.sensitivity = sensitivity.clamp(0.0, 1.0);
    }

    /// Check if enough time has passed since last trigger.
    ///
    /// Returns true if a ThoughtEvent should be emitted.
    /// Updates internal state to mark the trigger time.
    ///
    /// # Warm-up behaviour
    /// The first call always returns `false` and records the current time as the
    /// baseline. A thought event is only emitted after one full `interval` has
    /// elapsed from startup. This prevents the CTP from firing proactive
    /// inference within milliseconds of boot before the inference backend or
    /// any models are loaded — an early call into llama.cpp C FFI can crash
    /// the process with STATUS_ACCESS_VIOLATION (exit 0xC0000005).
    pub fn should_trigger(&mut self, snapshot: &ContextSnapshot) -> bool {
        let now = Instant::now();
        let periodic_trigger = match self.last_trigger {
            None => {
                // First call: record baseline, do NOT fire.
                self.last_trigger = Some(now);
                false
            }
            Some(last) if now.duration_since(last) >= self.interval => {
                self.last_trigger = Some(now);
                true
            }
            _ => false,
        };

        let diff_trigger = self
            .last_snapshot
            .as_ref()
            .map(|prev| {
                let score = context_diff_score(prev, snapshot);
                score >= diff_threshold(self.sensitivity)
            })
            .unwrap_or(false);

        self.last_snapshot = Some(snapshot.clone());

        if diff_trigger {
            self.last_trigger = Some(now);
        }

        periodic_trigger || diff_trigger
    }

    /// Reset the trigger timer so the next call to `should_trigger()` fires immediately.
    ///
    /// Sets `last_trigger` to a time already past the interval so the interval check
    /// fires on the very next call, bypassing the startup warm-up guard.
    pub fn reset(&mut self) {
        self.last_trigger = Some(Instant::now() - self.interval - Duration::from_secs(1));
    }
}

fn diff_threshold(sensitivity: f64) -> f64 {
    // sensitivity=0.0 -> 0.75 (harder), sensitivity=1.0 -> 0.25 (easier)
    0.75 - (0.50 * sensitivity)
}

fn context_diff_score(prev: &ContextSnapshot, current: &ContextSnapshot) -> f64 {
    let mut score: f64 = 0.0;

    if prev.active_app.app_name != current.active_app.app_name {
        score += 0.55;
    }

    if prev.active_app.window_title != current.active_app.window_title {
        score += 0.15;
    }

    if prev.clipboard_digest != current.clipboard_digest {
        score += 0.10;
    }

    let file_delta = prev.recent_files.len().abs_diff(current.recent_files.len());
    if file_delta >= 2 {
        score += 0.10;
    }

    // Frustration/repetition proxy: burst after noticeable idle.
    if current.keystroke_cadence.burst_detected
        && current.keystroke_cadence.idle_duration >= Duration::from_secs(45)
    {
        score += 0.20;
    }

    // Anomaly proxy: very high cadence spike.
    if current.keystroke_cadence.events_per_minute >= 180.0 {
        score += 0.10;
    }

    if prev.inferred_task != current.inferred_task {
        score += 0.30;
    }

    score.min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bus::events::ctp::EnrichedInferredTask;
    use bus::events::platform::{KeystrokeCadence, WindowContext};
    use std::thread::sleep;

    fn snapshot(app: &str) -> ContextSnapshot {
        let now = Instant::now();
        ContextSnapshot {
            active_app: WindowContext {
                app_name: app.to_owned(),
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
        }
    }

    #[test]
    fn first_check_does_not_trigger_warm_up_guard() {
        // The first call must NOT fire — it only sets the baseline timestamp.
        // Firing immediately causes proactive inference to run before models are
        // loaded, which crashes the process via STATUS_ACCESS_VIOLATION in
        // llama.cpp C FFI (exit 0xC0000005).
        let mut gate = TriggerGate::new(Duration::from_secs(5));
        assert!(!gate.should_trigger(&snapshot("Code")));
    }

    #[test]
    fn second_check_within_interval_does_not_trigger() {
        let mut gate = TriggerGate::new(Duration::from_secs(1));

        // First call: warm-up, no trigger.
        assert!(!gate.should_trigger(&snapshot("Code")));

        // Immediate second check should not trigger either.
        assert!(!gate.should_trigger(&snapshot("Code")));
    }

    #[test]
    fn check_after_interval_triggers() {
        let mut gate = TriggerGate::new(Duration::from_millis(100));

        // Warm-up call: does not trigger, records baseline.
        assert!(!gate.should_trigger(&snapshot("Code")));

        // Wait for interval to pass.
        sleep(Duration::from_millis(150));

        // Should trigger now that the interval has elapsed.
        assert!(gate.should_trigger(&snapshot("Code")));
    }

    #[test]
    fn reset_allows_immediate_trigger() {
        let mut gate = TriggerGate::new(Duration::from_secs(10));

        // Warm-up: no trigger.
        assert!(!gate.should_trigger(&snapshot("Code")));

        // Within interval: no trigger.
        assert!(!gate.should_trigger(&snapshot("Code")));

        // Reset sets the baseline to a past time so the interval is already elapsed.
        gate.reset();

        // Should trigger immediately after reset.
        assert!(gate.should_trigger(&snapshot("Code")));
    }

    #[test]
    fn context_switch_triggers_without_waiting_interval() {
        // With a huge interval, a context switch (score 0.55 >= threshold 0.50) fires.
        let mut gate = TriggerGate::new(Duration::from_secs(9999));
        // Warm-up call — no trigger, but records "Code" as previous snapshot.
        assert!(!gate.should_trigger(&snapshot("Code")));
        // Context diff: app changed → score 0.55 → diff_trigger fires.
        assert!(gate.should_trigger(&snapshot("Browser")));
    }

    #[test]
    fn task_change_contributes_to_diff_score() {
        let mut gate = TriggerGate::new(Duration::from_secs(9999)).with_sensitivity(1.0);
        let mut first = snapshot("Code");
        first.inferred_task = Some(EnrichedInferredTask {
            category: "coding".to_owned(),
            semantic_description: "Writing code".to_string(),
            confidence: 0.8,
        });
        let mut second = snapshot("Code");
        second.inferred_task = Some(EnrichedInferredTask {
            category: "research".to_owned(),
            semantic_description: "Reading documentation".to_string(),
            confidence: 0.7,
        });

        // Warm-up with first snapshot — no trigger.
        assert!(!gate.should_trigger(&first));
        // Task changed: score += 0.30, threshold at sensitivity=1.0 is 0.25 → fires.
        assert!(gate.should_trigger(&second));
    }
}
