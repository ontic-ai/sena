//! CTP (Continuous Thought Processing) events and context types.
//!
//! ContextSnapshot is the structured, typed capture of the user's computing
//! context at a moment in time. It references types from the platform module.

use std::time::{Duration, Instant};

use super::platform::{FileEvent, KeystrokeCadence, WindowContext};

/// Type alias for active application context.
/// Re-uses WindowContext from platform module.
pub type AppContext = WindowContext;

/// Inferred task category and confidence.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskHint {
    /// Task category (e.g., "coding", "writing", "research").
    pub category: String,
    /// Confidence level for the inferred task (0.0 to 1.0).
    pub confidence: f32,
}

/// Structured capture of user's computing context at a moment in time.
///
/// This is the typed output of the CTP Context Assembler.
/// Exact spec from architecture.md §6.2.
#[derive(Debug, Clone)]
pub struct ContextSnapshot {
    /// Currently active application context.
    pub active_app: AppContext,
    /// Recent file system events.
    pub recent_files: Vec<FileEvent>,
    /// Digest/summary of clipboard content — NOT raw text.
    pub clipboard_digest: Option<String>,
    /// Keystroke timing patterns — privacy-safe, no character content.
    pub keystroke_cadence: KeystrokeCadence,
    /// Duration of the current session.
    pub session_duration: Duration,
    /// Inferred task, if any.
    pub inferred_task: Option<TaskHint>,
    /// When this snapshot was captured.
    pub timestamp: Instant,
}

/// CTP-layer events.
#[derive(Debug, Clone)]
pub enum CTPEvent {
    /// A new context snapshot has been assembled and is ready.
    ContextSnapshotReady(ContextSnapshot),
    /// A thought event has been triggered based on a context snapshot.
    ThoughtEventTriggered(ContextSnapshot),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::events::platform::FileEventKind;

    #[test]
    fn task_hint_constructs_and_clones() {
        let hint = TaskHint {
            category: "coding".to_string(),
            confidence: 0.85,
        };
        let cloned = hint.clone();
        assert_eq!(cloned.category, "coding");
        assert_eq!(cloned.confidence, 0.85);
    }

    #[test]
    fn context_snapshot_constructs_with_realistic_fields() {
        let now = Instant::now();

        let active_app = WindowContext {
            app_name: "Code".to_string(),
            window_title: Some("main.rs - sena".to_string()),
            bundle_id: Some("com.microsoft.VSCode".to_string()),
            timestamp: now,
        };

        let file_event = FileEvent {
            path: PathBuf::from("/home/user/project/src/main.rs"),
            event_kind: FileEventKind::Modified,
            timestamp: now,
        };

        let keystroke_cadence = KeystrokeCadence {
            events_per_minute: 180.5,
            burst_detected: true,
            idle_duration: Duration::from_secs(2),
            timestamp: now,
        };

        let task_hint = TaskHint {
            category: "coding".to_string(),
            confidence: 0.92,
        };

        let snapshot = ContextSnapshot {
            active_app,
            recent_files: vec![file_event],
            clipboard_digest: Some("sha256:abcdef1234567890".to_string()),
            keystroke_cadence,
            session_duration: Duration::from_secs(3600),
            inferred_task: Some(task_hint),
            timestamp: now,
        };

        // Verify construction
        assert_eq!(snapshot.active_app.app_name, "Code");
        assert_eq!(snapshot.recent_files.len(), 1);
        assert_eq!(
            snapshot.clipboard_digest,
            Some("sha256:abcdef1234567890".to_string())
        );
        assert_eq!(snapshot.keystroke_cadence.events_per_minute, 180.5);
        assert_eq!(snapshot.session_duration, Duration::from_secs(3600));
        assert!(snapshot.inferred_task.is_some());
        if let Some(hint) = &snapshot.inferred_task {
            assert_eq!(hint.category, "coding");
            assert_eq!(hint.confidence, 0.92);
        }
    }

    #[test]
    fn context_snapshot_clones_correctly() {
        let now = Instant::now();

        let snapshot = ContextSnapshot {
            active_app: WindowContext {
                app_name: "Firefox".to_string(),
                window_title: Some("GitHub".to_string()),
                bundle_id: Some("org.mozilla.firefox".to_string()),
                timestamp: now,
            },
            recent_files: vec![],
            clipboard_digest: None,
            keystroke_cadence: KeystrokeCadence {
                events_per_minute: 50.0,
                burst_detected: false,
                idle_duration: Duration::from_secs(10),
                timestamp: now,
            },
            session_duration: Duration::from_secs(1800),
            inferred_task: None,
            timestamp: now,
        };

        let cloned = snapshot.clone();
        assert_eq!(cloned.active_app.app_name, "Firefox");
        assert_eq!(cloned.recent_files.len(), 0);
        assert_eq!(cloned.clipboard_digest, None);
        assert_eq!(cloned.inferred_task, None);
        assert_eq!(cloned.session_duration, Duration::from_secs(1800));
    }

    #[test]
    fn ctp_event_context_snapshot_ready_constructs_and_clones() {
        let snapshot = create_minimal_snapshot();
        let event = CTPEvent::ContextSnapshotReady(snapshot.clone());
        let cloned = event.clone();

        if let CTPEvent::ContextSnapshotReady(s) = cloned {
            assert_eq!(s.active_app.app_name, "TestApp");
        } else {
            panic!("Expected ContextSnapshotReady variant");
        }
    }

    #[test]
    fn ctp_event_thought_event_triggered_constructs_and_clones() {
        let snapshot = create_minimal_snapshot();
        let event = CTPEvent::ThoughtEventTriggered(snapshot.clone());
        let cloned = event.clone();

        if let CTPEvent::ThoughtEventTriggered(s) = cloned {
            assert_eq!(s.active_app.app_name, "TestApp");
        } else {
            panic!("Expected ThoughtEventTriggered variant");
        }
    }

    #[test]
    fn ctp_event_variants_are_distinct() {
        let snapshot = create_minimal_snapshot();
        let ready = CTPEvent::ContextSnapshotReady(snapshot.clone());
        let triggered = CTPEvent::ThoughtEventTriggered(snapshot.clone());

        // Verify they are different variants
        matches!(ready, CTPEvent::ContextSnapshotReady(_));
        matches!(triggered, CTPEvent::ThoughtEventTriggered(_));
    }

    // Compile-time verification: all types are Send
    #[allow(dead_code)]
    fn assert_send<T: Send>() {}

    #[test]
    fn types_are_send() {
        assert_send::<TaskHint>();
        assert_send::<ContextSnapshot>();
        assert_send::<CTPEvent>();
    }

    // Helper to create a minimal valid ContextSnapshot for testing
    fn create_minimal_snapshot() -> ContextSnapshot {
        let now = Instant::now();
        ContextSnapshot {
            active_app: WindowContext {
                app_name: "TestApp".to_string(),
                window_title: None,
                bundle_id: None,
                timestamp: now,
            },
            recent_files: vec![],
            clipboard_digest: None,
            keystroke_cadence: KeystrokeCadence {
                events_per_minute: 0.0,
                burst_detected: false,
                idle_duration: Duration::from_secs(0),
                timestamp: now,
            },
            session_duration: Duration::from_secs(0),
            inferred_task: None,
            timestamp: now,
        }
    }
}
