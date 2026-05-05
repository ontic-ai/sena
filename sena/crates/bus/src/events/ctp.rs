//! CTP (Continuous Thought Processing) events and context types.

use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

use super::platform::{FileEvent, KeystrokeCadence, WindowContext};
use super::soul::DistilledIdentitySignal;

/// Privacy-safe visual context from screen capture.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisualContext {
    /// Resolution of the captured image (width, height).
    pub resolution: (u32, u32),
    /// Age of the capture relative to now.
    pub age: Duration,
}

/// Enriched inferred task with semantic description.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnrichedInferredTask {
    /// Task category (e.g., "coding", "writing", "research").
    pub category: String,
    /// Semantic description of what the user appears to be doing.
    pub semantic_description: String,
    /// Confidence level for the inference (0.0 to 1.0).
    pub confidence: f32,
}

/// User cognitive state derived from behavioral signals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserState {
    /// Frustration level (0-100).
    pub frustration_level: u8,
    /// Whether the user is in a flow state.
    pub flow_detected: bool,
    /// Cost of context switching (0-100).
    pub context_switch_cost: u8,
}

/// Type of signal pattern detected.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SignalPatternType {
    Frustration,
    Repetition,
    FlowState,
    Anomaly,
}

/// Detected signal pattern from multi-modal signal analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalPattern {
    /// Type of pattern detected.
    pub pattern_type: SignalPatternType,
    /// Confidence in the pattern detection (0.0 to 1.0).
    pub confidence: f32,
    /// Human-readable description of the pattern.
    pub description: String,
}

/// Structured capture of user's computing context at a moment in time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSnapshot {
    /// Currently active application context.
    pub active_app: WindowContext,
    /// Recent file system events.
    pub recent_files: Vec<FileEvent>,
    /// Digest/summary of clipboard content — NOT raw text.
    pub clipboard_digest: Option<String>,
    /// Keystroke cadence pattern.
    pub keystroke_cadence: KeystrokeCadence,
    /// Session duration since boot.
    pub session_duration: Duration,
    /// Inferred task with semantic description.
    pub inferred_task: Option<EnrichedInferredTask>,
    /// User cognitive state.
    pub user_state: Option<UserState>,
    /// Visual context from recent screen capture.
    pub visual_context: Option<VisualContext>,
    /// When this snapshot was captured.
    #[serde(with = "crate::events::system::instant_serde")]
    pub timestamp: Instant,
    /// Cached identity signal from Soul (preserved across snapshots).
    pub soul_identity_signal: Option<DistilledIdentitySignal>,
}

/// CTP (Continuous Thought Processing) events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CTPEvent {
    /// The CTP processing loop has started.
    LoopStarted,
    /// The CTP processing loop has stopped.
    LoopStopped,
    /// A thought event was triggered by CTP.
    ThoughtEventTriggered(ContextSnapshot),

    /// Context snapshot was assembled (may not trigger thought).
    ContextSnapshotReady(ContextSnapshot),

    /// User state was computed.
    UserStateComputed(UserState),

    /// Signal pattern was detected.
    SignalPatternDetected(SignalPattern),

    /// Signal received and buffered by CTP.
    SignalReceived {
        signal_type: String,
        #[serde(with = "crate::events::system::instant_serde")]
        timestamp: Instant,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_snapshot_constructs() {
        let snapshot = ContextSnapshot {
            active_app: WindowContext {
                app_name: "TestApp".to_string(),
                window_title: None,
                bundle_id: None,
                timestamp: Instant::now(),
            },
            recent_files: vec![],
            clipboard_digest: None,
            keystroke_cadence: KeystrokeCadence {
                events_per_minute: 0.0,
                burst_detected: false,
                idle_duration: Duration::from_secs(0),
                timestamp: Instant::now(),
            },
            session_duration: Duration::from_secs(10),
            inferred_task: None,
            user_state: None,
            visual_context: None,
            timestamp: Instant::now(),
            soul_identity_signal: None,
        };
        assert_eq!(snapshot.active_app.app_name, "TestApp");
    }

    #[test]
    fn enriched_task_constructs() {
        let task = EnrichedInferredTask {
            category: "coding".to_string(),
            semantic_description: "Editing Rust code".to_string(),
            confidence: 0.85,
        };
        assert_eq!(task.category, "coding");
        assert!(task.confidence > 0.8);
    }

    #[test]
    fn user_state_constructs() {
        let state = UserState {
            frustration_level: 25,
            flow_detected: true,
            context_switch_cost: 10,
        };
        assert_eq!(state.frustration_level, 25);
        assert!(state.flow_detected);
    }

    #[test]
    fn signal_pattern_constructs() {
        let pattern = SignalPattern {
            pattern_type: SignalPatternType::FlowState,
            confidence: 0.90,
            description: "Sustained coding flow detected".to_string(),
        };
        assert_eq!(pattern.pattern_type, SignalPatternType::FlowState);
    }

    #[test]
    fn distilled_identity_signal_from_soul_imports_correctly() {
        // Verify DistilledIdentitySignal is imported from soul, not duplicated locally
        let signal = DistilledIdentitySignal {
            signal_key: "test::key".to_string(),
            signal_value: "test_value".to_string(),
            confidence: 0.95,
        };
        assert_eq!(signal.signal_key, "test::key");
        assert_eq!(signal.confidence, 0.95);
    }
}
