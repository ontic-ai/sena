//! Context assembler — transforms SignalBuffer into a ContextSnapshot.
//!
//! The assembler reads the current state of the signal buffer and constructs
//! a typed `ContextSnapshot` suitable for CTP trigger evaluation and broadcast.

use crate::signal_buffer::SignalBuffer;
use bus::events::ctp::{ContextSnapshot, EnrichedInferredTask};
use platform::{KeystrokeCadence, WindowContext};
use std::time::{Duration, Instant};

/// Assembles `ContextSnapshot` instances from buffered platform signals.
pub struct ContextAssembler;

impl ContextAssembler {
    /// Create a new context assembler.
    pub fn new() -> Self {
        Self
    }

    /// Assemble a snapshot from the current signal buffer state.
    ///
    /// If `previous` is provided, fields not updated since the last snapshot are
    /// carried forward, preserving continuity across assemblies.
    pub fn assemble_with_previous(
        &self,
        buffer: &SignalBuffer,
        session_start: Instant,
        previous: Option<&ContextSnapshot>,
    ) -> ContextSnapshot {
        let session_duration = session_start.elapsed();

        // Active window: latest from buffer, else carry forward from previous, else default
        let active_app = buffer
            .latest_window()
            .cloned()
            .or_else(|| previous.map(|p| p.active_app.clone()))
            .unwrap_or_else(|| WindowContext {
                app_name: "Unknown".to_string(),
                window_title: None,
                bundle_id: None,
                timestamp: Instant::now(),
            });

        // Keystroke cadence: latest from buffer, else carry forward, else zero
        let keystroke_cadence = buffer
            .latest_keystroke()
            .cloned()
            .or_else(|| previous.map(|p| p.keystroke_cadence.clone()))
            .unwrap_or_else(|| KeystrokeCadence {
                events_per_minute: 0.0,
                burst_detected: false,
                idle_duration: Duration::from_secs(0),
                timestamp: Instant::now(),
            });

        // Clipboard digest summary
        let clipboard_digest = buffer
            .latest_clipboard()
            .and_then(|d| d.digest.clone());

        // Recent file events (up to 10)
        let recent_files: Vec<_> = buffer.file_events().take(10).cloned().collect();

        // Infer task from active window (simple heuristic — real inference is deferred)
        let inferred_task = Self::infer_task_from_window(&active_app);

        ContextSnapshot {
            active_app,
            recent_files,
            clipboard_digest,
            keystroke_cadence,
            session_duration,
            inferred_task,
            user_state: None,
            visual_context: None,
            timestamp: Instant::now(),
            soul_identity_signal: previous.and_then(|p| p.soul_identity_signal.clone()),
        }
    }

    /// Heuristic task inference from window name.
    ///
    /// Returns a low-confidence `EnrichedInferredTask` based on the app name.
    /// This is replaced by the TaskInferenceEngine in later phases.
    fn infer_task_from_window(window: &WindowContext) -> Option<EnrichedInferredTask> {
        let app = window.app_name.to_lowercase();
        let category = if app.contains("code") || app.contains("vim") || app.contains("nvim") {
            "coding"
        } else if app.contains("chrome") || app.contains("firefox") || app.contains("safari") {
            "browsing"
        } else if app.contains("terminal") || app.contains("powershell") || app.contains("cmd") {
            "terminal"
        } else if app.contains("word") || app.contains("docs") || app.contains("notion") {
            "writing"
        } else {
            return None;
        };

        Some(EnrichedInferredTask {
            category: category.to_string(),
            semantic_description: format!("User is using {}", window.app_name),
            confidence: 0.3,
        })
    }
}

impl Default for ContextAssembler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn assembler_produces_snapshot_from_empty_buffer() {
        let assembler = ContextAssembler::new();
        let buffer = SignalBuffer::new(Duration::from_secs(60));
        let snapshot =
            assembler.assemble_with_previous(&buffer, Instant::now(), None);
        assert_eq!(snapshot.active_app.app_name, "Unknown");
    }

    #[test]
    fn assembler_uses_buffer_window() {
        let assembler = ContextAssembler::new();
        let mut buffer = SignalBuffer::new(Duration::from_secs(60));
        buffer.push_window(WindowContext {
            app_name: "Code".to_string(),
            window_title: None,
            bundle_id: None,
            timestamp: Instant::now(),
        });
        let snapshot =
            assembler.assemble_with_previous(&buffer, Instant::now(), None);
        assert_eq!(snapshot.active_app.app_name, "Code");
    }
}
