//! Pattern engine — detects behavioral signal patterns from the signal buffer.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use bus::events::ctp::{ContextSnapshot, SignalPattern, SignalPatternType};

use crate::signal_buffer::SignalBuffer;

/// Detects behavioral patterns from accumulated CTP signals.
pub struct PatternEngine {
    last_app_name: Option<String>,
    sustained_cadence_start: Option<Instant>,
    clipboard_history: HashMap<String, usize>,
    last_pattern_trigger: HashMap<SignalPatternType, Instant>,
}

impl PatternEngine {
    /// Create a new pattern engine.
    pub fn new() -> Self {
        Self {
            last_app_name: None,
            sustained_cadence_start: None,
            clipboard_history: HashMap::new(),
            last_pattern_trigger: HashMap::new(),
        }
    }

    /// Detect behavioral patterns from the signal buffer and current snapshot.
    ///
    /// Returns a (possibly empty) list of detected patterns, ordered by confidence.
    pub fn detect(
        &mut self,
        buffer: &SignalBuffer,
        snapshot: &ContextSnapshot,
    ) -> Vec<SignalPattern> {
        let mut patterns = Vec::new();

        if let Some(frustration) = self.detect_frustration(buffer, snapshot) {
            patterns.push(frustration);
        }

        if let Some(repetition) = self.detect_repetition(buffer) {
            patterns.push(repetition);
        }

        if let Some(flow) = self.detect_flow_state(buffer, snapshot) {
            patterns.push(flow);
        }

        if let Some(anomaly) = self.detect_anomaly(buffer, snapshot) {
            patterns.push(anomaly);
        }

        self.last_app_name = Some(snapshot.active_app.app_name.clone());

        patterns
    }

    fn detect_frustration(
        &mut self,
        buffer: &SignalBuffer,
        snapshot: &ContextSnapshot,
    ) -> Option<SignalPattern> {
        let now = Instant::now();

        if let Some(&last) = self
            .last_pattern_trigger
            .get(&SignalPatternType::Frustration)
            && now.duration_since(last) < Duration::from_secs(60)
        {
            return None;
        }

        let burst_after_idle = snapshot.keystroke_cadence.burst_detected
            && snapshot.keystroke_cadence.idle_duration >= Duration::from_secs(30);

        if !burst_after_idle {
            return None;
        }

        let same_app_sustained = buffer.window_event_count() >= 4
            && buffer
                .windows()
                .rev()
                .take(4)
                .all(|window| window.app_name == snapshot.active_app.app_name);

        let high_cadence = snapshot.keystroke_cadence.events_per_minute >= 160.0;

        if burst_after_idle && (same_app_sustained || high_cadence) {
            self.last_pattern_trigger
                .insert(SignalPatternType::Frustration, now);
            return Some(SignalPattern {
                pattern_type: SignalPatternType::Frustration,
                confidence: 0.70,
                description: format!(
                    "Frustration detected: idle followed by burst ({:.0} EPM) in {}",
                    snapshot.keystroke_cadence.events_per_minute, snapshot.active_app.app_name
                ),
            });
        }

        None
    }

    fn detect_repetition(&mut self, buffer: &SignalBuffer) -> Option<SignalPattern> {
        let now = Instant::now();

        if let Some(&last) = self
            .last_pattern_trigger
            .get(&SignalPatternType::Repetition)
            && now.duration_since(last) < Duration::from_secs(90)
        {
            return None;
        }

        self.clipboard_history.clear();
        for digest in buffer.clipboard_events() {
            if let Some(ref digest_str) = digest.digest {
                *self
                    .clipboard_history
                    .entry(digest_str.clone())
                    .or_insert(0) += 1;
            }
        }

        for &count in self.clipboard_history.values() {
            if count >= 3 {
                self.last_pattern_trigger
                    .insert(SignalPatternType::Repetition, now);
                return Some(SignalPattern {
                    pattern_type: SignalPatternType::Repetition,
                    confidence: 0.65,
                    description: format!(
                        "Repetition detected: clipboard content repeated {} times",
                        count
                    ),
                });
            }
        }

        None
    }

    fn detect_flow_state(
        &mut self,
        buffer: &SignalBuffer,
        snapshot: &ContextSnapshot,
    ) -> Option<SignalPattern> {
        let now = Instant::now();
        let cadence = snapshot.keystroke_cadence.events_per_minute;

        if (80.0..=150.0).contains(&cadence) {
            if self.sustained_cadence_start.is_none() {
                self.sustained_cadence_start = Some(now);
            }
        } else {
            self.sustained_cadence_start = None;
            return None;
        }

        if let Some(start) = self.sustained_cadence_start {
            let duration = now.duration_since(start);
            if duration >= Duration::from_secs(600) {
                let app_switches = self.count_app_switches(buffer);

                if app_switches <= 5 {
                    if let Some(&last) =
                        self.last_pattern_trigger.get(&SignalPatternType::FlowState)
                        && now.duration_since(last) < Duration::from_secs(300)
                    {
                        return None;
                    }

                    self.last_pattern_trigger
                        .insert(SignalPatternType::FlowState, now);
                    return Some(SignalPattern {
                        pattern_type: SignalPatternType::FlowState,
                        confidence: 0.75,
                        description: format!(
                            "Flow state detected: sustained cadence {:.0} EPM for {:.0} minutes",
                            cadence,
                            duration.as_secs() as f64 / 60.0
                        ),
                    });
                }
            }
        }

        None
    }

    fn detect_anomaly(
        &mut self,
        buffer: &SignalBuffer,
        snapshot: &ContextSnapshot,
    ) -> Option<SignalPattern> {
        let now = Instant::now();

        if let Some(&last) = self.last_pattern_trigger.get(&SignalPatternType::Anomaly)
            && now.duration_since(last) < Duration::from_secs(60)
        {
            return None;
        }

        let cadence = snapshot.keystroke_cadence.events_per_minute;
        let app_switches = self.count_app_switches(buffer);

        if cadence > 200.0 {
            self.last_pattern_trigger
                .insert(SignalPatternType::Anomaly, now);
            return Some(SignalPattern {
                pattern_type: SignalPatternType::Anomaly,
                confidence: 0.60,
                description: format!(
                    "Anomaly: extremely high keystroke cadence ({:.0} EPM)",
                    cadence
                ),
            });
        }

        if app_switches > 8 {
            self.last_pattern_trigger
                .insert(SignalPatternType::Anomaly, now);
            return Some(SignalPattern {
                pattern_type: SignalPatternType::Anomaly,
                confidence: 0.60,
                description: format!("Anomaly: rapid app switching ({} switches)", app_switches),
            });
        }

        None
    }

    fn count_app_switches(&self, buffer: &SignalBuffer) -> usize {
        let mut switches = 0;
        let mut previous_app: Option<&str> = None;

        for window in buffer.windows() {
            if let Some(previous) = previous_app
                && previous != window.app_name.as_str()
            {
                switches += 1;
            }
            previous_app = Some(&window.app_name);
        }

        switches
    }
}

impl Default for PatternEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform::{ClipboardDigest, KeystrokeCadence, WindowContext};

    fn mock_snapshot(app_name: &str, cadence: f64, idle: u64, burst: bool) -> ContextSnapshot {
        ContextSnapshot {
            active_app: WindowContext {
                app_name: app_name.to_string(),
                window_title: None,
                bundle_id: None,
                timestamp: Instant::now(),
            },
            recent_files: vec![],
            clipboard_digest: None,
            keystroke_cadence: KeystrokeCadence {
                events_per_minute: cadence,
                burst_detected: burst,
                idle_duration: Duration::from_secs(idle),
                timestamp: Instant::now(),
            },
            session_duration: Duration::from_secs(600),
            inferred_task: None,
            user_state: None,
            visual_context: None,
            timestamp: Instant::now(),
            soul_identity_signal: None,
        }
    }

    #[test]
    fn frustration_pattern_detected() {
        let mut engine = PatternEngine::new();
        let mut buffer = SignalBuffer::new(Duration::from_secs(600));

        for _ in 0..5 {
            buffer.push_window(WindowContext {
                app_name: "Code".to_string(),
                window_title: Some("main.rs".to_string()),
                bundle_id: None,
                timestamp: Instant::now(),
            });
        }

        let patterns = engine.detect(&buffer, &mock_snapshot("Code", 180.0, 35, true));

        assert_eq!(patterns.len(), 1);
        assert!(matches!(
            patterns[0].pattern_type,
            SignalPatternType::Frustration
        ));
    }

    #[test]
    fn repetition_pattern_detected() {
        let mut engine = PatternEngine::new();
        let mut buffer = SignalBuffer::new(Duration::from_secs(600));

        for _ in 0..3 {
            buffer.push_clipboard(ClipboardDigest {
                digest: Some("sha256:abc123".to_string()),
                char_count: 20,
                timestamp: Instant::now(),
            });
        }

        let patterns = engine.detect(&buffer, &mock_snapshot("Chrome", 60.0, 0, false));

        assert_eq!(patterns.len(), 1);
        assert!(matches!(
            patterns[0].pattern_type,
            SignalPatternType::Repetition
        ));
    }

    #[test]
    fn anomaly_pattern_detected_for_rapid_switching() {
        let mut engine = PatternEngine::new();
        let mut buffer = SignalBuffer::new(Duration::from_secs(600));

        for app in [
            "Code", "Chrome", "Terminal", "Slack", "Code", "Chrome", "Firefox", "Code", "Terminal",
            "VSCode",
        ] {
            buffer.push_window(WindowContext {
                app_name: app.to_string(),
                window_title: None,
                bundle_id: None,
                timestamp: Instant::now(),
            });
        }

        let patterns = engine.detect(&buffer, &mock_snapshot("VSCode", 100.0, 0, false));

        assert_eq!(patterns.len(), 1);
        assert!(matches!(
            patterns[0].pattern_type,
            SignalPatternType::Anomaly
        ));
    }

    #[test]
    fn normal_activity_does_not_emit_patterns() {
        let mut engine = PatternEngine::new();
        let buffer = SignalBuffer::new(Duration::from_secs(600));

        let patterns = engine.detect(&buffer, &mock_snapshot("Code", 90.0, 0, false));

        assert!(patterns.is_empty());
    }
}
