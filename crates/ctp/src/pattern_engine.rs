//! Signal pattern recognition engine for behavioral analysis.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use bus::events::ctp::{ContextSnapshot, SignalPattern, SignalPatternType};

use crate::signal_buffer::SignalBuffer;

/// Pattern recognition engine for detecting behavioral signals.
///
/// Analyzes the signal buffer and context snapshot to detect recognizable
/// patterns like frustration, repetition, flow states, and anomalies.
pub struct PatternEngine {
    /// Track last app to detect sustained use.
    last_app_name: Option<String>,
    /// Track sustained cadence minutes for flow detection.
    sustained_cadence_start: Option<Instant>,
    /// Track clipboard history for repetition detection.
    clipboard_history: HashMap<String, usize>,
    /// Last trigger time per pattern type for debouncing.
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

    /// Analyze the signal buffer for recognizable behavioral patterns.
    ///
    /// Returns detected patterns (may return multiple simultaneously).
    pub fn detect(
        &mut self,
        buffer: &SignalBuffer,
        snapshot: &ContextSnapshot,
    ) -> Vec<SignalPattern> {
        let mut patterns = Vec::new();

        // Rule 1: Frustration pattern
        if let Some(frustration) = self.detect_frustration(buffer, snapshot) {
            patterns.push(frustration);
        }

        // Rule 2: Repetition pattern
        if let Some(repetition) = self.detect_repetition(buffer) {
            patterns.push(repetition);
        }

        // Rule 3: Flow state pattern
        if let Some(flow) = self.detect_flow_state(buffer, snapshot) {
            patterns.push(flow);
        }

        // Rule 4: Anomaly pattern
        if let Some(anomaly) = self.detect_anomaly(buffer, snapshot) {
            patterns.push(anomaly);
        }

        self.last_app_name = Some(snapshot.active_app.app_name.clone());

        patterns
    }

    /// Detect frustration: idle >=30s followed by burst + same app for >=2 min.
    fn detect_frustration(
        &mut self,
        buffer: &SignalBuffer,
        snapshot: &ContextSnapshot,
    ) -> Option<SignalPattern> {
        let now = Instant::now();

        // Debounce: don't re-fire frustration within 60 seconds
        if let Some(&last) = self
            .last_pattern_trigger
            .get(&SignalPatternType::Frustration)
        {
            if now.duration_since(last) < Duration::from_secs(60) {
                return None;
            }
        }

        // Check for burst after idle
        let burst_after_idle = snapshot.keystroke_cadence.burst_detected
            && snapshot.keystroke_cadence.idle_duration >= Duration::from_secs(30);

        if !burst_after_idle {
            return None;
        }

        // Check for sustained app usage (>=2 minutes in same app)
        let same_app_sustained = buffer.window_events_count() >= 4
            && buffer
                .all_windows()
                .iter()
                .rev()
                .take(4)
                .all(|window| window.app_name == snapshot.active_app.app_name);

        // High cadence burst (>=160 EPM)
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

    /// Detect repetition: same clipboard digest >=3 times in buffer.
    fn detect_repetition(&mut self, buffer: &SignalBuffer) -> Option<SignalPattern> {
        let now = Instant::now();

        // Debounce: don't re-fire repetition within 90 seconds
        if let Some(&last) = self
            .last_pattern_trigger
            .get(&SignalPatternType::Repetition)
        {
            if now.duration_since(last) < Duration::from_secs(90) {
                return None;
            }
        }

        // Count clipboard digest occurrences
        self.clipboard_history.clear();
        for digest in buffer.all_clipboard() {
            if let Some(ref digest_str) = digest.digest {
                *self
                    .clipboard_history
                    .entry(digest_str.clone())
                    .or_insert(0) += 1;
            }
        }

        // Check for any digest repeated >=3 times
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

    /// Detect flow state: sustained cadence 80-150 EPM for >=10 min with <=5 app switches.
    fn detect_flow_state(
        &mut self,
        buffer: &SignalBuffer,
        snapshot: &ContextSnapshot,
    ) -> Option<SignalPattern> {
        let now = Instant::now();
        let cadence = snapshot.keystroke_cadence.events_per_minute;

        // Check if cadence is in flow range
        let in_flow_range = (80.0..=150.0).contains(&cadence);

        if in_flow_range {
            // Start tracking if not already
            if self.sustained_cadence_start.is_none() {
                self.sustained_cadence_start = Some(now);
            }
        } else {
            // Reset if out of range
            self.sustained_cadence_start = None;
            return None;
        }

        // Check duration
        if let Some(start) = self.sustained_cadence_start {
            let duration = now.duration_since(start);
            if duration >= Duration::from_secs(600) {
                // 10 minutes
                // Count app switches in buffer
                let app_switches = self.count_app_switches(buffer);

                if app_switches <= 5 {
                    // Debounce: don't re-fire flow within 5 minutes
                    if let Some(&last) =
                        self.last_pattern_trigger.get(&SignalPatternType::FlowState)
                    {
                        if now.duration_since(last) < Duration::from_secs(300) {
                            return None;
                        }
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

    /// Detect anomaly: cadence >200 EPM OR >8 app switches in 2 min.
    fn detect_anomaly(
        &mut self,
        buffer: &SignalBuffer,
        snapshot: &ContextSnapshot,
    ) -> Option<SignalPattern> {
        let now = Instant::now();

        // Debounce: don't re-fire anomaly within 60 seconds
        if let Some(&last) = self.last_pattern_trigger.get(&SignalPatternType::Anomaly) {
            if now.duration_since(last) < Duration::from_secs(60) {
                return None;
            }
        }

        let cadence = snapshot.keystroke_cadence.events_per_minute;
        let app_switches = self.count_app_switches(buffer);

        // High cadence anomaly
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

        // Rapid app switching anomaly
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

    /// Count app switches in the buffer.
    fn count_app_switches(&self, buffer: &SignalBuffer) -> usize {
        let window_events = buffer.all_windows();
        if window_events.is_empty() {
            return 0;
        }

        let mut switches = 0;
        let mut prev_app: Option<&str> = None;

        for window in window_events {
            if let Some(prev) = prev_app {
                if prev != window.app_name.as_str() {
                    switches += 1;
                }
            }
            prev_app = Some(&window.app_name);
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
    use bus::events::platform::{ClipboardDigest, KeystrokeCadence, WindowContext};

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

        // Simulate sustained app use
        let now = Instant::now();
        for i in 0..5 {
            buffer.push_window(WindowContext {
                app_name: "Code".to_string(),
                window_title: Some("main.rs".to_string()),
                bundle_id: None,
                timestamp: now + Duration::from_secs(i * 30),
            });
        }

        engine.last_app_name = Some("Code".to_string());

        // High cadence burst after idle
        let snapshot = mock_snapshot("Code", 180.0, 35, true);

        let patterns = engine.detect(&buffer, &snapshot);
        assert_eq!(patterns.len(), 1);
        assert!(matches!(
            patterns[0].pattern_type,
            SignalPatternType::Frustration
        ));
        assert_eq!(patterns[0].confidence, 0.70);
    }

    #[test]
    fn repetition_pattern_detected() {
        let mut engine = PatternEngine::new();
        let mut buffer = SignalBuffer::new(Duration::from_secs(600));

        let now = Instant::now();
        // Same clipboard content 3 times
        for i in 0..3 {
            buffer.push_clipboard(ClipboardDigest {
                digest: Some("sha256:abc123".to_string()),
                char_count: 20,
                timestamp: now + Duration::from_secs(i * 60),
            });
        }

        let snapshot = mock_snapshot("Chrome", 60.0, 0, false);

        let patterns = engine.detect(&buffer, &snapshot);
        assert_eq!(patterns.len(), 1);
        assert!(matches!(
            patterns[0].pattern_type,
            SignalPatternType::Repetition
        ));
        assert_eq!(patterns[0].confidence, 0.65);
    }

    #[test]
    fn flow_state_pattern_detected() {
        let mut engine = PatternEngine::new();
        let mut buffer = SignalBuffer::new(Duration::from_secs(900));

        let now = Instant::now();
        // Minimal app switching (<=5)
        for i in 0..3 {
            buffer.push_window(WindowContext {
                app_name: "Code".to_string(),
                window_title: Some("lib.rs".to_string()),
                bundle_id: None,
                timestamp: now + Duration::from_secs(i * 120),
            });
        }

        // Simulate sustained cadence by backdating start time
        engine.sustained_cadence_start = Some(now - Duration::from_secs(620));

        let snapshot = mock_snapshot("Code", 120.0, 0, false);

        let patterns = engine.detect(&buffer, &snapshot);
        assert_eq!(patterns.len(), 1);
        assert!(matches!(
            patterns[0].pattern_type,
            SignalPatternType::FlowState
        ));
        assert_eq!(patterns[0].confidence, 0.75);
    }

    #[test]
    fn anomaly_pattern_high_cadence() {
        let mut engine = PatternEngine::new();
        let buffer = SignalBuffer::new(Duration::from_secs(600));

        // Very high cadence
        let snapshot = mock_snapshot("Terminal", 250.0, 0, true);

        let patterns = engine.detect(&buffer, &snapshot);
        assert_eq!(patterns.len(), 1);
        assert!(matches!(
            patterns[0].pattern_type,
            SignalPatternType::Anomaly
        ));
        assert_eq!(patterns[0].confidence, 0.60);
    }

    #[test]
    fn anomaly_pattern_rapid_switching() {
        let mut engine = PatternEngine::new();
        let mut buffer = SignalBuffer::new(Duration::from_secs(600));

        let now = Instant::now();
        // Rapid app switching (>8 switches)
        let apps = vec![
            "Code", "Chrome", "Terminal", "Slack", "Code", "Chrome", "Firefox", "Code", "Terminal",
            "VSCode",
        ];
        for (i, app) in apps.iter().enumerate() {
            buffer.push_window(WindowContext {
                app_name: app.to_string(),
                window_title: None,
                bundle_id: None,
                timestamp: now + Duration::from_secs(i as u64 * 10),
            });
        }

        let snapshot = mock_snapshot("VSCode", 100.0, 0, false);

        let patterns = engine.detect(&buffer, &snapshot);
        assert_eq!(patterns.len(), 1);
        assert!(matches!(
            patterns[0].pattern_type,
            SignalPatternType::Anomaly
        ));
        assert_eq!(patterns[0].confidence, 0.60);
    }

    #[test]
    fn no_patterns_detected_when_normal() {
        let mut engine = PatternEngine::new();
        let buffer = SignalBuffer::new(Duration::from_secs(600));

        // Normal cadence, no burst, no idle
        let snapshot = mock_snapshot("Code", 90.0, 0, false);

        let patterns = engine.detect(&buffer, &snapshot);
        // No frustration, no repetition, no sustained flow yet, no anomaly
        assert!(
            patterns.is_empty()
                || patterns
                    .iter()
                    .all(|p| matches!(p.pattern_type, SignalPatternType::FlowState))
        );
    }
}
