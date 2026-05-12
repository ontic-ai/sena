//! Trigger gate — decides when CTP should emit a ThoughtEvent from raw context.

use bus::events::ctp::ContextSnapshot;
use std::time::{Duration, Instant};

/// Default minimum interval between consecutive thought events.
const DEFAULT_MIN_INTERVAL: Duration = Duration::from_secs(600); // 10 minutes

/// Evaluates whether CTP should emit a ThoughtEvent for a given snapshot.
pub struct TriggerGate {
    min_interval: Duration,
    last_trigger: Option<Instant>,
}

impl TriggerGate {
    /// Create a new trigger gate with the given minimum interval.
    pub fn new(min_interval: Duration) -> Self {
        Self {
            min_interval,
            last_trigger: None,
        }
    }

    /// Reset the trigger timer so the next interval check can fire immediately.
    pub fn reset(&mut self) {
        self.last_trigger = Some(Instant::now() - self.min_interval - Duration::from_secs(1));
    }

    /// Evaluate whether a ThoughtEvent should be emitted.
    ///
    /// The first call never fires. It establishes a baseline snapshot so later
    /// evaluations can establish the initial cooldown state instead of immediately
    /// triggering during startup.
    pub fn should_trigger(&mut self, snapshot: &ContextSnapshot) -> bool {
        let now = Instant::now();
        let _ = snapshot;

        match self.last_trigger {
            None => {
                self.last_trigger = Some(now);
                false
            }
            Some(last) if now.duration_since(last) >= self.min_interval => {
                self.last_trigger = Some(now);
                true
            }
            Some(_) => false,
        }
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
            visual_context: None,
            timestamp: now,
            soul_identity_signal: None,
        }
    }

    #[test]
    fn first_check_does_not_trigger_warm_up_guard() {
        let mut gate = TriggerGate::new(Duration::from_secs(5));
        assert!(!gate.should_trigger(&snapshot("Code")));
    }

    #[test]
    fn check_after_interval_triggers() {
        let mut gate = TriggerGate::new(Duration::from_millis(100));

        assert!(!gate.should_trigger(&snapshot("Code")));

        sleep(Duration::from_millis(150));

        assert!(gate.should_trigger(&snapshot("Code")));
    }

    #[test]
    fn reset_allows_immediate_trigger() {
        let mut gate = TriggerGate::new(Duration::from_secs(10));

        assert!(!gate.should_trigger(&snapshot("Code")));
        assert!(!gate.should_trigger(&snapshot("Code")));

        gate.reset();

        assert!(gate.should_trigger(&snapshot("Code")));
    }

    #[test]
    fn context_switch_does_not_bypass_cooldown() {
        let mut gate = TriggerGate::new(Duration::from_secs(9999));

        assert!(!gate.should_trigger(&snapshot("Code")));
        assert!(!gate.should_trigger(&snapshot("Browser")));
    }

    #[test]
    fn window_title_change_does_not_bypass_cooldown() {
        let mut gate = TriggerGate::new(Duration::from_secs(9999));
        let first = snapshot("Code");

        let mut second = snapshot("Code");
        second.active_app.window_title = Some("README.md".to_string());

        assert!(!gate.should_trigger(&first));
        assert!(!gate.should_trigger(&second));
    }

    #[test]
    fn keystroke_shift_does_not_bypass_cooldown() {
        let mut gate = TriggerGate::new(Duration::from_secs(9999));
        let first = snapshot("Code");
        let mut second = snapshot("Code");
        second.keystroke_cadence.burst_detected = true;
        second.keystroke_cadence.idle_duration = Duration::from_secs(45);
        second.keystroke_cadence.events_per_minute = 180.0;

        assert!(!gate.should_trigger(&first));
        assert!(!gate.should_trigger(&second));
    }

    #[test]
    fn clipboard_change_does_not_bypass_cooldown() {
        let mut gate = TriggerGate::new(Duration::from_secs(9999));
        let first = snapshot("Code");
        let mut second = snapshot("Code");
        second.clipboard_digest = Some("digest-2".to_string());

        assert!(!gate.should_trigger(&first));
        assert!(!gate.should_trigger(&second));
    }
}
