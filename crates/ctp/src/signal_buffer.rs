//! Rolling time-window accumulator for platform events.

use std::collections::VecDeque;
use std::time::{Duration, Instant, SystemTime};

use bus::events::ctp::VisualContext;
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
    /// Latest visual context from screen capture. Stored separately as
    /// captures may be infrequent and not part of the rolling window.
    latest_visual_context: Option<(VisualContext, SystemTime)>,
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
            latest_visual_context: None,
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

    /// Store a visual context from screen capture.
    /// Replaces any previously stored visual context.
    pub fn push_visual_context(&mut self, visual_context: VisualContext, timestamp: SystemTime) {
        self.latest_visual_context = Some((visual_context, timestamp));
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

        // Prune keystroke events
        while let Some(event) = self.keystroke_events.front() {
            if event.timestamp <= cutoff_time {
                self.keystroke_events.pop_front();
            } else {
                break;
            }
        }
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

    /// Get the most recent visual context if age is less than 30 seconds.
    /// Returns None if no visual context stored or if it's too old.
    pub fn latest_visual_context(&self) -> Option<VisualContext> {
        let now = SystemTime::now();
        self.latest_visual_context
            .as_ref()
            .and_then(|(ctx, timestamp)| {
                now.duration_since(*timestamp).ok().and_then(|age| {
                    if age < Duration::from_secs(30) {
                        let mut refreshed = ctx.clone();
                        refreshed.age = age;
                        Some(refreshed)
                    } else {
                        None
                    }
                })
            })
    }

    /// Get all window events in the buffer.
    pub fn all_windows(&self) -> &VecDeque<WindowContext> {
        &self.window_events
    }

    /// Get all clipboard events in the buffer.
    pub fn all_clipboard(&self) -> &VecDeque<ClipboardDigest> {
        &self.clipboard_events
    }

    /// Get the count of window events in the buffer.
    pub fn window_events_count(&self) -> usize {
        self.window_events.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bus::events::ctp::VisualContext;
    use bus::events::platform::FileEventKind;
    use bus::events::platform_vision::ImageDigest;
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
            timestamp: Instant::now(),
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

    #[test]
    fn latest_visual_context_returns_refreshed_age_when_recent() {
        let mut buffer = SignalBuffer::new(Duration::from_secs(300));
        let capture_time = SystemTime::now()
            .checked_sub(Duration::from_secs(5))
            .unwrap_or(SystemTime::now());

        buffer.push_visual_context(
            VisualContext {
                digest: ImageDigest::new([7u8; 32]),
                resolution: (1920, 1080),
                age: Duration::from_secs(0),
            },
            capture_time,
        );

        let visual = buffer
            .latest_visual_context()
            .expect("visual context should be present when capture is recent");
        assert_eq!(visual.resolution, (1920, 1080));
        assert!(visual.age >= Duration::from_secs(5));
        assert!(visual.age < Duration::from_secs(30));
    }

    #[test]
    fn latest_visual_context_returns_none_when_capture_is_stale() {
        let mut buffer = SignalBuffer::new(Duration::from_secs(300));
        let capture_time = SystemTime::now()
            .checked_sub(Duration::from_secs(31))
            .unwrap_or(SystemTime::now());

        buffer.push_visual_context(
            VisualContext {
                digest: ImageDigest::new([9u8; 32]),
                resolution: (1280, 720),
                age: Duration::from_secs(0),
            },
            capture_time,
        );

        assert!(buffer.latest_visual_context().is_none());
    }
}
