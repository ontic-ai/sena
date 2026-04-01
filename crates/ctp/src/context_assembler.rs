//! Context snapshot assembly from signal buffer.

use std::time::{Duration, Instant};

use bus::events::ctp::{ContextSnapshot, TaskHint};
use bus::events::platform::{KeystrokeCadence, WindowContext};

use crate::signal_buffer::SignalBuffer;

/// Assembles a ContextSnapshot from signal buffer state.
///
/// Transforms the raw signal buffer into the typed ContextSnapshot format
/// that the CTP layer emits on the bus.
pub struct ContextAssembler;

impl ContextAssembler {
    /// Create a new context assembler.
    pub fn new() -> Self {
        Self
    }

    /// Assemble a ContextSnapshot from the current signal buffer state.
    ///
    /// # Arguments
    /// * `buffer` - The signal buffer containing recent platform events
    /// * `session_start` - When the current session started (for duration calculation)
    ///
    /// # Returns
    /// A fully populated ContextSnapshot with defaults for missing signals.
    pub fn assemble(&self, buffer: &SignalBuffer, session_start: Instant) -> ContextSnapshot {
        self.assemble_with_previous(buffer, session_start, None)
    }

    /// Assemble a ContextSnapshot from current buffer state while preserving
    /// the last known observation when a signal is temporarily absent.
    pub fn assemble_with_previous(
        &self,
        buffer: &SignalBuffer,
        session_start: Instant,
        previous_snapshot: Option<&ContextSnapshot>,
    ) -> ContextSnapshot {
        let now = Instant::now();

        // Preserve the last known active app when the rolling buffer no longer
        // contains a recent window event.
        let active_app = buffer
            .latest_window()
            .cloned()
            .or_else(|| previous_snapshot.map(|snapshot| snapshot.active_app.clone()))
            .unwrap_or_else(|| WindowContext {
                app_name: "Unknown".to_string(),
                window_title: None,
                bundle_id: None,
                timestamp: now,
            });

        // Get recent file events
        let recent_files = buffer.recent_files();

        // Get clipboard digest string
        let clipboard_digest = buffer
            .latest_clipboard()
            .and_then(|digest| digest.digest.clone())
            .or_else(|| previous_snapshot.and_then(|snapshot| snapshot.clipboard_digest.clone()));

        // Preserve the last observed cadence when no new pattern event has
        // arrived since the previous snapshot.
        let keystroke_cadence = buffer
            .latest_keystroke()
            .cloned()
            .or_else(|| previous_snapshot.map(|snapshot| snapshot.keystroke_cadence.clone()))
            .unwrap_or_else(|| KeystrokeCadence {
                events_per_minute: 0.0,
                burst_detected: false,
                idle_duration: Duration::from_secs(0),
                timestamp: now,
            });

        // Calculate session duration
        let session_duration = now.duration_since(session_start);

        // Infer likely task category from active app and interaction cadence.
        let inferred_task = infer_task_hint(
            &active_app.app_name,
            active_app.window_title.as_deref(),
            &keystroke_cadence,
        );

        // Construct the snapshot
        ContextSnapshot {
            active_app,
            recent_files,
            clipboard_digest,
            keystroke_cadence,
            session_duration,
            inferred_task,
            timestamp: now,
        }
    }
}

fn infer_task_hint(
    app_name: &str,
    window_title: Option<&str>,
    cadence: &KeystrokeCadence,
) -> Option<TaskHint> {
    let app = app_name.to_lowercase();
    let title = window_title.unwrap_or_default().to_lowercase();

    let (category, mut confidence): (&str, f64) = if app.contains("code")
        || app.contains("rustrover")
        || app.contains("idea")
        || app.contains("cursor")
    {
        ("coding", 0.78)
    } else if app.contains("chrome")
        || app.contains("firefox")
        || app.contains("edge")
        || app.contains("browser")
    {
        ("research", 0.66)
    } else if app.contains("word")
        || app.contains("notion")
        || app.contains("obsidian")
        || title.contains("doc")
    {
        ("writing", 0.70)
    } else if app.contains("terminal") || app.contains("powershell") || app.contains("cmd") {
        ("operations", 0.72)
    } else {
        ("general", 0.45)
    };

    if cadence.burst_detected {
        confidence += 0.08;
    }
    if cadence.events_per_minute > 140.0 {
        confidence += 0.05;
    }

    Some(TaskHint {
        category: category.to_owned(),
        confidence: confidence.min(0.95) as f32,
    })
}

impl Default for ContextAssembler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bus::events::ctp::TaskHint;
    use bus::events::platform::{ClipboardDigest, FileEvent, FileEventKind, WindowContext};
    use std::path::PathBuf;
    use std::thread::sleep;

    #[test]
    fn test_assemble_with_empty_buffer() {
        let assembler = ContextAssembler::new();
        let buffer = SignalBuffer::new(Duration::from_secs(300));
        let session_start = Instant::now();

        let snapshot = assembler.assemble(&buffer, session_start);

        // Should have default values
        assert_eq!(snapshot.active_app.app_name, "Unknown");
        assert_eq!(snapshot.recent_files.len(), 0);
        assert!(snapshot.clipboard_digest.is_none());
        assert_eq!(snapshot.keystroke_cadence.events_per_minute, 0.0);
        assert!(snapshot.inferred_task.is_some());
    }

    #[test]
    fn test_assemble_with_window_event() {
        let assembler = ContextAssembler::new();
        let mut buffer = SignalBuffer::new(Duration::from_secs(300));
        let session_start = Instant::now();

        let window_ctx = WindowContext {
            app_name: "VSCode".to_string(),
            window_title: Some("main.rs".to_string()),
            bundle_id: Some("com.microsoft.VSCode".to_string()),
            timestamp: Instant::now(),
        };

        buffer.push_window(window_ctx);

        let snapshot = assembler.assemble(&buffer, session_start);

        assert_eq!(snapshot.active_app.app_name, "VSCode");
        assert_eq!(
            snapshot.active_app.window_title,
            Some("main.rs".to_string())
        );
    }

    #[test]
    fn test_session_duration_calculated() {
        let assembler = ContextAssembler::new();
        let buffer = SignalBuffer::new(Duration::from_secs(300));

        // Session started 2 seconds ago
        let session_start = Instant::now()
            .checked_sub(Duration::from_secs(2))
            .unwrap_or_else(Instant::now);

        // Small sleep to ensure measurable duration
        sleep(Duration::from_millis(10));

        let snapshot = assembler.assemble(&buffer, session_start);

        // Session duration should be at least 2 seconds
        assert!(snapshot.session_duration >= Duration::from_secs(2));
    }

    #[test]
    fn test_assemble_with_all_signals() {
        let assembler = ContextAssembler::new();
        let mut buffer = SignalBuffer::new(Duration::from_secs(300));
        let session_start = Instant::now();

        // Add various events
        let window_ctx = WindowContext {
            app_name: "Terminal".to_string(),
            window_title: Some("bash".to_string()),
            bundle_id: None,
            timestamp: Instant::now(),
        };
        buffer.push_window(window_ctx);

        let clipboard = ClipboardDigest {
            digest: Some("abc123".to_string()),
            char_count: 42,
            timestamp: Instant::now(),
        };
        buffer.push_clipboard(clipboard);

        let file_event = FileEvent {
            path: PathBuf::from("/test/file.rs"),
            event_kind: FileEventKind::Modified,
            timestamp: Instant::now(),
        };
        buffer.push_file_event(file_event);

        let keystroke = KeystrokeCadence {
            events_per_minute: 180.5,
            burst_detected: true,
            idle_duration: Duration::from_secs(1),
            timestamp: Instant::now(),
        };
        buffer.push_keystroke(keystroke);

        let snapshot = assembler.assemble(&buffer, session_start);

        // Verify all signals are present
        assert_eq!(snapshot.active_app.app_name, "Terminal");
        assert_eq!(snapshot.clipboard_digest, Some("abc123".to_string()));
        assert_eq!(snapshot.recent_files.len(), 1);
        assert_eq!(snapshot.keystroke_cadence.events_per_minute, 180.5);
    }

    #[test]
    fn assemble_with_previous_preserves_last_known_signals() {
        let assembler = ContextAssembler::new();
        let buffer = SignalBuffer::new(Duration::from_secs(1));
        let session_start = Instant::now();
        let previous_snapshot = ContextSnapshot {
            active_app: WindowContext {
                app_name: "Code".to_string(),
                window_title: Some("lib.rs".to_string()),
                bundle_id: Some("com.microsoft.VSCode".to_string()),
                timestamp: Instant::now(),
            },
            recent_files: Vec::new(),
            clipboard_digest: Some("digest-123".to_string()),
            keystroke_cadence: KeystrokeCadence {
                events_per_minute: 88.0,
                burst_detected: true,
                idle_duration: Duration::from_secs(2),
                timestamp: Instant::now(),
            },
            session_duration: Duration::from_secs(30),
            inferred_task: Some(TaskHint {
                category: "coding".to_string(),
                confidence: 0.8,
            }),
            timestamp: Instant::now(),
        };

        let snapshot =
            assembler.assemble_with_previous(&buffer, session_start, Some(&previous_snapshot));

        assert_eq!(snapshot.active_app.app_name, "Code");
        assert_eq!(snapshot.clipboard_digest.as_deref(), Some("digest-123"));
        assert_eq!(snapshot.keystroke_cadence.events_per_minute, 88.0);
    }
}
