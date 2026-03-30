//! Rolling time-window accumulator for platform events.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use bus::events::platform::{ClipboardDigest, FileEvent, KeystrokeCadence, WindowContext};

/// Rolling time-window buffer for platform events.
///
/// Maintains a sliding window of recent events. Events older than the
/// configured window_duration are pruned on demand.
pub struct SignalBuffer {
    window_events: VecDeque<WindowContext>,
    clipboard_events: VecDeque<ClipboardDigest>,
    file_events: VecDeque<FileEvent>,
    keystroke_events: VecDeque<KeystrokeCadence>,
    window_duration: Duration,
}

impl SignalBuffer {
    /// Create a new signal buffer with the specified time window.
    pub fn new(window_duration: Duration) -> Self {
        Self {
            window_events: VecDeque::new(),
            clipboard_events: VecDeque::new(),
            file_events: VecDeque::new(),
            keystroke_events: VecDeque::new(),
            window_duration,
        }
    }

    /// Push a window context event into the buffer.
    pub fn push_window(&mut self, ctx: WindowContext) {
        self.window_events.push_back(ctx);
    }

    /// Push a clipboard digest into the buffer.
    pub fn push_clipboard(&mut self, digest: ClipboardDigest) {
        self.clipboard_events.push_back(digest);
    }

    /// Push a file event into the buffer.
    pub fn push_file_event(&mut self, event: FileEvent) {
        self.file_events.push_back(event);
    }

    /// Push a keystroke cadence event into the buffer.
    pub fn push_keystroke(&mut self, cadence: KeystrokeCadence) {
        self.keystroke_events.push_back(cadence);
    }

    /// Remove events older than the window duration.
    ///
    /// Events are pruned based on their timestamp fields relative to now.
    pub fn prune(&mut self) {
        let now = Instant::now();
        let cutoff_time = now.checked_sub(self.window_duration).unwrap_or(now);

        // Prune window events
        while let Some(event) = self.window_events.front() {
            if event.timestamp <= cutoff_time {
                self.window_events.pop_front();
            } else {
                break;
            }
        }

        // Prune clipboard events
        while let Some(event) = self.clipboard_events.front() {
            if event.timestamp <= cutoff_time {
                self.clipboard_events.pop_front();
            } else {
                break;
            }
        }

        // Prune file events
        while let Some(event) = self.file_events.front() {
            if event.timestamp <= cutoff_time {
                self.file_events.pop_front();
            } else {
                break;
            }
        }

        // Prune keystroke events - these don't have a timestamp field,
        // so we keep all of them for now
        // TODO: reconsider if KeystrokeCadence needs timestamp
    }

    /// Get the most recent window context, if any.
    pub fn latest_window(&self) -> Option<&WindowContext> {
        self.window_events.back()
    }

    /// Get the most recent clipboard digest, if any.
    pub fn latest_clipboard(&self) -> Option<&ClipboardDigest> {
        self.clipboard_events.back()
    }

    /// Get all recent file events as a Vec.
    pub fn recent_files(&self) -> Vec<FileEvent> {
        self.file_events.iter().cloned().collect()
    }

    /// Get the most recent keystroke cadence, if any.
    pub fn latest_keystroke(&self) -> Option<&KeystrokeCadence> {
        self.keystroke_events.back()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bus::events::platform::FileEventKind;
    use std::path::PathBuf;

    #[test]
    fn test_push_and_retrieve_window_event() {
        let mut buffer = SignalBuffer::new(Duration::from_secs(300));
        let now = Instant::now();

        let ctx = WindowContext {
            app_name: "TestApp".to_string(),
            window_title: Some("Test Window".to_string()),
            bundle_id: Some("com.test.app".to_string()),
            timestamp: now,
        };

        buffer.push_window(ctx.clone());

        let latest = buffer.latest_window();
        assert!(latest.is_some());
        assert_eq!(latest.unwrap().app_name, "TestApp");
    }

    #[test]
    fn test_push_and_retrieve_keystroke() {
        let mut buffer = SignalBuffer::new(Duration::from_secs(300));

        let cadence = KeystrokeCadence {
            events_per_minute: 120.0,
            burst_detected: false,
            idle_duration: Duration::from_secs(5),
        };

        buffer.push_keystroke(cadence.clone());

        let latest = buffer.latest_keystroke();
        assert!(latest.is_some());
        assert_eq!(latest.unwrap().events_per_minute, 120.0);
    }

    #[test]
    fn test_prune_removes_old_events() {
        let mut buffer = SignalBuffer::new(Duration::from_secs(2));

        // Create an old event (3 seconds ago)
        let old_time = Instant::now()
            .checked_sub(Duration::from_secs(3))
            .unwrap_or_else(Instant::now);

        let old_ctx = WindowContext {
            app_name: "OldApp".to_string(),
            window_title: None,
            bundle_id: None,
            timestamp: old_time,
        };

        buffer.push_window(old_ctx);

        // Create a recent event
        let recent_ctx = WindowContext {
            app_name: "NewApp".to_string(),
            window_title: None,
            bundle_id: None,
            timestamp: Instant::now(),
        };

        buffer.push_window(recent_ctx);

        // Before pruning, we should have 2 events
        assert_eq!(buffer.window_events.len(), 2);

        // Prune old events
        buffer.prune();

        // After pruning, we should only have the recent event
        assert_eq!(buffer.window_events.len(), 1);
        assert_eq!(buffer.latest_window().unwrap().app_name, "NewApp");
    }

    #[test]
    fn test_empty_buffer_returns_none() {
        let buffer = SignalBuffer::new(Duration::from_secs(300));

        assert!(buffer.latest_window().is_none());
        assert!(buffer.latest_clipboard().is_none());
        assert!(buffer.latest_keystroke().is_none());
        assert_eq!(buffer.recent_files().len(), 0);
    }

    #[test]
    fn test_recent_files_returns_all_files() {
        let mut buffer = SignalBuffer::new(Duration::from_secs(300));
        let now = Instant::now();

        let file1 = FileEvent {
            path: PathBuf::from("/test/file1.txt"),
            event_kind: FileEventKind::Modified,
            timestamp: now,
        };

        let file2 = FileEvent {
            path: PathBuf::from("/test/file2.txt"),
            event_kind: FileEventKind::Created,
            timestamp: now,
        };

        buffer.push_file_event(file1);
        buffer.push_file_event(file2);

        let files = buffer.recent_files();
        assert_eq!(files.len(), 2);
    }
}
