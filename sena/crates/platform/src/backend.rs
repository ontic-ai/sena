//! Platform backend trait definition.
//!
//! Defines the contract that all platform-specific implementations must satisfy.

use crate::error::PlatformError;
use crate::types::PlatformSignal;

/// Platform backend trait — one method per signal type.
///
/// Each OS-specific backend (Windows, macOS, Linux) implements this trait.
/// Stub implementations return typed defaults with logging.
pub trait PlatformBackend: Send + Sync {
    /// Retrieve the currently active window context.
    ///
    /// Returns `PlatformSignal::Window` with application name, window title, and bundle ID
    /// if available. Stub implementations return a default context with a placeholder app name.
    fn active_window(&self) -> Result<PlatformSignal, PlatformError>;

    /// Retrieve clipboard content digest.
    ///
    /// Returns `PlatformSignal::Clipboard` with a SHA-256 digest and character count.
    /// Never returns raw clipboard text. Stub implementations return an empty digest.
    fn clipboard_content(&self) -> Result<PlatformSignal, PlatformError>;

    /// Retrieve keystroke cadence pattern.
    ///
    /// Returns `PlatformSignal::Keystroke` with timing patterns only — no character content.
    /// Stub implementations return a zero-activity cadence.
    fn keystroke_cadence(&self) -> Result<PlatformSignal, PlatformError>;

    /// Capture the current screen frame.
    ///
    /// Returns `PlatformSignal::ScreenFrame` with raw RGB pixel data.
    /// Stub implementations return a 1x1 black pixel frame.
    fn screen_frame(&self) -> Result<PlatformSignal, PlatformError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ClipboardDigest, KeystrokeCadence, ScreenFrame, WindowContext};
    use std::time::{Duration, Instant};

    struct TestBackend;

    impl PlatformBackend for TestBackend {
        fn active_window(&self) -> Result<PlatformSignal, PlatformError> {
            Ok(PlatformSignal::Window(WindowContext {
                app_name: "TestApp".to_string(),
                window_title: None,
                bundle_id: None,
                timestamp: Instant::now(),
            }))
        }

        fn clipboard_content(&self) -> Result<PlatformSignal, PlatformError> {
            Ok(PlatformSignal::Clipboard(ClipboardDigest {
                digest: None,
                char_count: 0,
                timestamp: Instant::now(),
            }))
        }

        fn keystroke_cadence(&self) -> Result<PlatformSignal, PlatformError> {
            Ok(PlatformSignal::Keystroke(KeystrokeCadence {
                events_per_minute: 0.0,
                burst_detected: false,
                idle_duration: Duration::from_secs(0),
                timestamp: Instant::now(),
            }))
        }

        fn screen_frame(&self) -> Result<PlatformSignal, PlatformError> {
            Ok(PlatformSignal::ScreenFrame(ScreenFrame {
                width: 1,
                height: 1,
                rgb_data: vec![0, 0, 0],
                timestamp: Instant::now(),
            }))
        }
    }

    #[test]
    fn test_backend_implements_trait() {
        let backend = TestBackend;
        assert!(backend.active_window().is_ok());
        assert!(backend.clipboard_content().is_ok());
        assert!(backend.keystroke_cadence().is_ok());
        assert!(backend.screen_frame().is_ok());
    }

    #[test]
    fn test_backend_returns_correct_variants() {
        let backend = TestBackend;

        match backend.active_window().ok() {
            Some(PlatformSignal::Window(_)) => {}
            _ => panic!("expected Window variant"),
        }

        match backend.clipboard_content().ok() {
            Some(PlatformSignal::Clipboard(_)) => {}
            _ => panic!("expected Clipboard variant"),
        }

        match backend.keystroke_cadence().ok() {
            Some(PlatformSignal::Keystroke(_)) => {}
            _ => panic!("expected Keystroke variant"),
        }

        match backend.screen_frame().ok() {
            Some(PlatformSignal::ScreenFrame(_)) => {}
            _ => panic!("expected ScreenFrame variant"),
        }
    }
}
