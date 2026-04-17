//! Platform-layer events: window focus, clipboard, file system, input patterns.
//!
//! PRIVACY-CRITICAL: KeystrokeCadence is a compile-time privacy boundary.
//! It MUST NOT contain any char, String, Vec<char>, or Vec<u8> fields.

use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Information about the currently active window.
#[derive(Debug, Clone)]
pub struct WindowContext {
    /// Application name (e.g., "Firefox", "Code").
    pub app_name: String,
    /// Window title, if accessible.
    pub window_title: Option<String>,
    /// Platform-specific bundle identifier.
    pub bundle_id: Option<String>,
    /// When this context was captured.
    pub timestamp: Instant,
}

/// Digest of clipboard content — never raw text.
#[derive(Debug, Clone)]
pub struct ClipboardDigest {
    /// SHA-256 digest of clipboard content, if available.
    pub digest: Option<String>,
    /// Character count of clipboard content.
    pub char_count: usize,
    /// When this digest was captured.
    pub timestamp: Instant,
}

/// File system event kind.
#[derive(Debug, Clone)]
pub enum FileEventKind {
    /// File was created.
    Created,
    /// File was modified.
    Modified,
    /// File was deleted.
    Deleted,
    /// File was renamed.
    Renamed,
}

/// File system event detected by platform watcher.
#[derive(Debug, Clone)]
pub struct FileEvent {
    /// Path to the file.
    pub path: PathBuf,
    /// Type of file system event.
    pub event_kind: FileEventKind,
    /// When this event was detected.
    pub timestamp: Instant,
}

/// Keystroke timing cadence — PRIVACY BOUNDARY.
///
/// This type captures input *patterns* only. It MUST NOT contain:
/// - char
/// - String
/// - Vec<char>
/// - Vec<u8> representing character content
#[derive(Debug, Clone)]
pub struct KeystrokeCadence {
    /// Average keystrokes per minute over the observation window.
    pub events_per_minute: f64,
    /// Whether a burst of rapid typing was detected.
    pub burst_detected: bool,
    /// Duration of idle time since last keystroke.
    pub idle_duration: Duration,
    /// When this cadence was captured.
    pub timestamp: Instant,
}

/// Platform-layer events.
#[derive(Debug, Clone)]
pub enum PlatformEvent {
    /// Active window changed.
    ActiveWindowChanged(WindowContext),
    /// Clipboard content changed.
    ClipboardChanged(ClipboardDigest),
    /// File system event detected.
    FileEvent(FileEvent),
    /// Keystroke cadence pattern detected.
    KeystrokeCadenceUpdated(KeystrokeCadence),
    /// Vision frame (screenshot) captured and ready for processing.
    VisionFrameAvailable {
        /// Frame data (encoded as PNG bytes).
        frame_data: Vec<u8>,
        /// Screen ID or display number.
        screen_id: u8,
        /// When this frame was captured.
        timestamp: Instant,
    },

    /// Deprecated: use ActiveWindowChanged.
    #[deprecated(note = "use ActiveWindowChanged")]
    WindowChanged(WindowContext),
    /// Deprecated: use KeystrokeCadenceUpdated.
    #[deprecated(note = "use KeystrokeCadenceUpdated")]
    KeystrokePattern(KeystrokeCadence),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_context_constructs_and_clones() {
        let ctx = WindowContext {
            app_name: "TestApp".to_string(),
            window_title: Some("Test Window".to_string()),
            bundle_id: Some("com.test.app".to_string()),
            timestamp: Instant::now(),
        };
        let cloned = ctx.clone();
        assert_eq!(cloned.app_name, "TestApp");
    }

    #[test]
    fn keystroke_cadence_is_privacy_safe() {
        let cadence = KeystrokeCadence {
            events_per_minute: 120.0,
            burst_detected: true,
            idle_duration: Duration::from_secs(5),
            timestamp: Instant::now(),
        };
        // Verify no character content fields exist
        assert_eq!(cadence.events_per_minute, 120.0);
    }

    #[test]
    fn active_window_changed_event_constructs() {
        let ctx = WindowContext {
            app_name: "TestApp".to_string(),
            window_title: None,
            bundle_id: None,
            timestamp: Instant::now(),
        };
        let event = PlatformEvent::ActiveWindowChanged(ctx);
        assert!(matches!(event, PlatformEvent::ActiveWindowChanged(_)));
    }

    #[test]
    fn vision_frame_available_constructs() {
        let event = PlatformEvent::VisionFrameAvailable {
            frame_data: vec![0x89, 0x50, 0x4E, 0x47], // PNG magic bytes
            screen_id: 0,
            timestamp: Instant::now(),
        };
        assert!(matches!(event, PlatformEvent::VisionFrameAvailable { .. }));
    }
}
