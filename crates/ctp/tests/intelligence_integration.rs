use std::path::PathBuf;
use std::time::{Duration, Instant};

use bus::events::ctp::{ContextSnapshot, SignalPattern, SignalPatternType};
use bus::events::platform::{FileEvent, FileEventKind, KeystrokeCadence, WindowContext};
use ctp::pattern_engine::PatternEngine;
use ctp::signal_buffer::SignalBuffer;
use ctp::task_inference::TaskInferenceEngine;
use ctp::trigger_gate::TriggerGate;
use ctp::user_state::UserStateClassifier;

fn make_window(app_name: &str, window_title: &str, timestamp: Instant) -> WindowContext {
    WindowContext {
        app_name: app_name.to_string(),
        window_title: Some(window_title.to_string()),
        bundle_id: Some(format!("com.test.{}", app_name.to_lowercase())),
        timestamp,
    }
}

fn make_file_event(index: usize, timestamp: Instant) -> FileEvent {
    FileEvent {
        path: PathBuf::from(format!("workspace/src/file_{index}.rs")),
        event_kind: FileEventKind::Modified,
        timestamp,
    }
}

fn make_snapshot(
    app_name: &str,
    window_title: &str,
    events_per_minute: f64,
    burst_detected: bool,
    idle_duration: Duration,
    file_count: usize,
    clipboard_digest: Option<&str>,
) -> ContextSnapshot {
    let now = Instant::now();
    ContextSnapshot {
        active_app: make_window(app_name, window_title, now),
        recent_files: (0..file_count)
            .map(|index| make_file_event(index, now))
            .collect(),
        clipboard_digest: clipboard_digest.map(str::to_string),
        keystroke_cadence: KeystrokeCadence {
            events_per_minute,
            burst_detected,
            idle_duration,
            timestamp: now,
        },
        session_duration: Duration::from_secs(60 * 45),
        inferred_task: None,
        user_state: None,
        visual_context: None,
        timestamp: now,
        soul_identity_signal: None,
    }
}

#[test]
fn test_pattern_engine_detects_frustration_from_signals() {
    let mut engine = PatternEngine::new();
    let mut buffer = SignalBuffer::new(Duration::from_secs(60 * 5));
    let start = Instant::now()
        .checked_sub(Duration::from_secs(120))
        .expect("test clock should allow subtracting two minutes");

    buffer.push_keystroke(KeystrokeCadence {
        events_per_minute: 0.0,
        burst_detected: false,
        idle_duration: Duration::from_secs(35),
        timestamp: start,
    });

    for offset_seconds in [0_u64, 30, 60, 90] {
        buffer.push_window(make_window(
            "Code",
            "main.rs - sena",
            start
                .checked_add(Duration::from_secs(offset_seconds))
                .expect("test clock should allow adding window offsets"),
        ));
    }

    buffer.push_keystroke(KeystrokeCadence {
        events_per_minute: 185.0,
        burst_detected: true,
        idle_duration: Duration::from_secs(35),
        timestamp: Instant::now(),
    });

    let snapshot = make_snapshot(
        "Code",
        "main.rs - sena",
        185.0,
        true,
        Duration::from_secs(35),
        2,
        None,
    );

    let patterns = engine.detect(&buffer, &snapshot);
    let frustration = patterns
        .iter()
        .find(|pattern| pattern.pattern_type == SignalPatternType::Frustration)
        .expect("frustration pattern should be detected from idle burst typing in the same app");

    assert!(frustration.confidence >= 0.70);
    assert!(frustration.description.contains("Frustration detected"));
    assert!(frustration.description.contains("Code"));
}

#[test]
fn test_user_state_classification_from_snapshot() {
    let mut classifier = UserStateClassifier::new();
    let snapshot = make_snapshot(
        "Code",
        "main.rs - sena",
        110.0,
        false,
        Duration::from_secs(4),
        8,
        None,
    );

    let state = classifier.classify(&snapshot, &[]);

    assert!((45..=55).contains(&state.frustration_level));
    assert!(state.flow_detected);
    assert!((35..=45).contains(&state.context_switch_cost));
}

#[test]
fn test_task_inference_generates_semantic_description() {
    let engine = TaskInferenceEngine;
    let snapshot = make_snapshot(
        "Code",
        "main.rs - sena",
        165.0,
        true,
        Duration::from_secs(2),
        3,
        None,
    );

    let task = engine
        .infer(&snapshot)
        .expect("task inference should produce a semantic description for coding activity");
    let lowered = task.semantic_description.to_lowercase();

    assert!(lowered.contains("rust") || lowered.contains("code"));
    assert!(task.confidence > 0.5);
}

#[test]
fn test_trigger_gate_significance_scoring_with_patterns() {
    let baseline = make_snapshot(
        "Code",
        "main.rs - sena",
        90.0,
        false,
        Duration::from_secs(5),
        1,
        None,
    );
    let candidate = make_snapshot(
        "Code",
        "main.rs - sena",
        190.0,
        true,
        Duration::from_secs(50),
        1,
        None,
    );

    let patterns = vec![
        SignalPattern {
            pattern_type: SignalPatternType::Frustration,
            confidence: 0.85,
            description: "Frustration signal".to_string(),
        },
        SignalPattern {
            pattern_type: SignalPatternType::Anomaly,
            confidence: 0.65,
            description: "Cadence anomaly".to_string(),
        },
    ];

    let mut gate_without_patterns = TriggerGate::new(Duration::from_secs(60 * 60));
    assert!(!gate_without_patterns.should_trigger(&baseline, &[], 0.0));
    let without_patterns = gate_without_patterns.should_trigger(&candidate, &[], 0.0);

    let mut gate_with_patterns = TriggerGate::new(Duration::from_secs(60 * 60));
    assert!(!gate_with_patterns.should_trigger(&baseline, &[], 0.0));
    let with_patterns = gate_with_patterns.should_trigger(&candidate, &patterns, 0.0);

    assert!(!without_patterns);
    assert!(with_patterns);
}
