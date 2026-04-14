//! Platform signal types.
//!
//! Re-exports privacy-safe types from bus and defines the unified PlatformSignal enum.

pub use bus::events::{ClipboardDigest, FileEvent, FileEventKind, KeystrokeCadence, WindowContext};

/// Unified platform signal type returned by PlatformBackend methods.
#[derive(Debug, Clone)]
pub enum PlatformSignal {
    /// Active window context.
    Window(WindowContext),

    /// Clipboard content digest (never raw text).
    Clipboard(ClipboardDigest),

    /// Keystroke cadence pattern (privacy-safe, no character content).
    Keystroke(KeystrokeCadence),

    /// Screen frame capture (raw RGB bytes).
    ScreenFrame(ScreenFrame),

    /// File system event.
    FileSystem(FileEvent),
}

/// Screen frame capture — raw RGB pixel data.
#[derive(Debug, Clone)]
pub struct ScreenFrame {
    /// Width of the captured frame in pixels.
    pub width: u32,

    /// Height of the captured frame in pixels.
    pub height: u32,

    /// Raw RGB pixel data (length = width * height * 3).
    pub rgb_data: Vec<u8>,

    /// Timestamp when this frame was captured.
    pub timestamp: std::time::Instant,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn platform_signal_constructs_window() {
        let ctx = WindowContext {
            app_name: "TestApp".to_string(),
            window_title: Some("Test".to_string()),
            bundle_id: None,
            timestamp: Instant::now(),
        };
        let signal = PlatformSignal::Window(ctx);
        match signal {
            PlatformSignal::Window(w) => assert_eq!(w.app_name, "TestApp"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn platform_signal_constructs_keystroke() {
        let cadence = KeystrokeCadence {
            events_per_minute: 90.0,
            burst_detected: false,
            idle_duration: Duration::from_secs(2),
            timestamp: Instant::now(),
        };
        let signal = PlatformSignal::Keystroke(cadence);
        match signal {
            PlatformSignal::Keystroke(k) => assert_eq!(k.events_per_minute, 90.0),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn screen_frame_constructs() {
        let frame = ScreenFrame {
            width: 1920,
            height: 1080,
            rgb_data: vec![0u8; 1920 * 1080 * 3],
            timestamp: Instant::now(),
        };
        assert_eq!(frame.width, 1920);
        assert_eq!(frame.rgb_data.len(), 1920 * 1080 * 3);
    }
}
