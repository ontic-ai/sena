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
    /// Platform-specific bundle identifier (e.g., "com.mozilla.firefox").
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
///
/// Any such field is a critical privacy violation per copilot-instructions.md §4.7.
#[derive(Debug, Clone)]
pub struct KeystrokeCadence {
    /// Average keystrokes per minute over the observation window.
    pub events_per_minute: f64,
    /// Whether a burst of rapid typing was detected.
    pub burst_detected: bool,
    /// Duration of idle time since last keystroke.
    pub idle_duration: Duration,
}

/// Platform-layer events.
#[derive(Debug, Clone)]
pub enum PlatformEvent {
    /// Active window changed.
    WindowChanged(WindowContext),
    /// Clipboard content changed.
    ClipboardChanged(ClipboardDigest),
    /// File system event detected.
    FileEvent(FileEvent),
    /// Keystroke cadence pattern detected.
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
        assert_eq!(cloned.window_title, Some("Test Window".to_string()));
        assert_eq!(cloned.bundle_id, Some("com.test.app".to_string()));
    }

    #[test]
    fn clipboard_digest_constructs_and_clones() {
        let digest = ClipboardDigest {
            digest: Some("abc123".to_string()),
            char_count: 42,
            timestamp: Instant::now(),
        };
        let cloned = digest.clone();
        assert_eq!(cloned.digest, Some("abc123".to_string()));
        assert_eq!(cloned.char_count, 42);
    }

    #[test]
    fn file_event_kind_clones() {
        let kind = FileEventKind::Created;
        let cloned = kind.clone();
        matches!(cloned, FileEventKind::Created);

        let kind = FileEventKind::Modified;
        matches!(kind.clone(), FileEventKind::Modified);

        let kind = FileEventKind::Deleted;
        matches!(kind.clone(), FileEventKind::Deleted);

        let kind = FileEventKind::Renamed;
        matches!(kind.clone(), FileEventKind::Renamed);
    }

    #[test]
    fn file_event_constructs_and_clones() {
        let event = FileEvent {
            path: PathBuf::from("/tmp/test.txt"),
            event_kind: FileEventKind::Modified,
            timestamp: Instant::now(),
        };
        let cloned = event.clone();
        assert_eq!(cloned.path, PathBuf::from("/tmp/test.txt"));
        matches!(cloned.event_kind, FileEventKind::Modified);
    }

    #[test]
    fn keystroke_cadence_constructs_and_clones() {
        let cadence = KeystrokeCadence {
            events_per_minute: 120.5,
            burst_detected: true,
            idle_duration: Duration::from_secs(5),
        };
        let cloned = cadence.clone();
        assert_eq!(cloned.events_per_minute, 120.5);
        assert!(cloned.burst_detected);
        assert_eq!(cloned.idle_duration, Duration::from_secs(5));
    }

    #[test]
    fn platform_event_window_changed_constructs_and_clones() {
        let ctx = WindowContext {
            app_name: "Browser".to_string(),
            window_title: None,
            bundle_id: None,
            timestamp: Instant::now(),
        };
        let event = PlatformEvent::WindowChanged(ctx);
        let cloned = event.clone();

        if let PlatformEvent::WindowChanged(window_ctx) = cloned {
            assert_eq!(window_ctx.app_name, "Browser");
        } else {
            panic!("Expected WindowChanged variant");
        }
    }

    #[test]
    fn platform_event_clipboard_changed_constructs_and_clones() {
        let digest = ClipboardDigest {
            digest: None,
            char_count: 0,
            timestamp: Instant::now(),
        };
        let event = PlatformEvent::ClipboardChanged(digest);
        let cloned = event.clone();

        if let PlatformEvent::ClipboardChanged(clip_digest) = cloned {
            assert_eq!(clip_digest.char_count, 0);
        } else {
            panic!("Expected ClipboardChanged variant");
        }
    }

    #[test]
    fn platform_event_file_event_constructs_and_clones() {
        let file_event = FileEvent {
            path: PathBuf::from("/test/file.rs"),
            event_kind: FileEventKind::Created,
            timestamp: Instant::now(),
        };
        let event = PlatformEvent::FileEvent(file_event);
        let cloned = event.clone();

        if let PlatformEvent::FileEvent(f_event) = cloned {
            assert_eq!(f_event.path, PathBuf::from("/test/file.rs"));
            matches!(f_event.event_kind, FileEventKind::Created);
        } else {
            panic!("Expected FileEvent variant");
        }
    }

    #[test]
    fn platform_event_keystroke_pattern_constructs_and_clones() {
        let cadence = KeystrokeCadence {
            events_per_minute: 80.0,
            burst_detected: false,
            idle_duration: Duration::from_secs(10),
        };
        let event = PlatformEvent::KeystrokePattern(cadence);
        let cloned = event.clone();

        if let PlatformEvent::KeystrokePattern(kc) = cloned {
            assert_eq!(kc.events_per_minute, 80.0);
            assert!(!kc.burst_detected);
            assert_eq!(kc.idle_duration, Duration::from_secs(10));
        } else {
            panic!("Expected KeystrokePattern variant");
        }
    }

    // Compile-time verification: all types are Send
    #[allow(dead_code)]
    fn assert_send<T: Send>() {}

    #[test]
    fn types_are_send() {
        assert_send::<WindowContext>();
        assert_send::<ClipboardDigest>();
        assert_send::<FileEventKind>();
        assert_send::<FileEvent>();
        assert_send::<KeystrokeCadence>();
        assert_send::<PlatformEvent>();
    }

    /// Privacy verification: KeystrokeCadence contains no character-capable types.
    ///
    /// This test ensures the type signature remains privacy-safe.
    /// If KeystrokeCadence ever gains a char/String/Vec<char>/Vec<u8> field,
    /// this will fail to compile or the assertion will fail.
    #[test]
    fn keystroke_cadence_is_privacy_safe() {
        // Construct a KeystrokeCadence with only allowed types
        let cadence = KeystrokeCadence {
            events_per_minute: 100.0,
            burst_detected: true,
            idle_duration: Duration::from_secs(2),
        };

        // Verify fields are the expected primitive types
        assert!(cadence.events_per_minute > 0.0);
        assert!(cadence.burst_detected);
        assert!(cadence.idle_duration.as_secs() > 0);

        // This test is intentionally minimal. The real protection is:
        // 1. Type signature in the struct definition above
        // 2. Code review enforcement per copilot-instructions.md §4.7
        // 3. Grep audit: no char/String fields in KeystrokeCadence
    }
}
