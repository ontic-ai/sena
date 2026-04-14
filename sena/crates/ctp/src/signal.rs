//! CTP signal types and buffering.

use platform::{ClipboardDigest, FileEvent, KeystrokeCadence, WindowContext};
use std::time::Instant;

/// Observation signal types that CTP can ingest.
#[derive(Debug, Clone)]
pub enum CtpSignal {
    /// Active window changed.
    WindowChanged(WindowContext),
    /// Clipboard content changed.
    ClipboardChanged(ClipboardDigest),
    /// File system event detected.
    FileEvent(FileEvent),
    /// Keystroke cadence pattern observed.
    KeystrokePattern(KeystrokeCadence),
    /// Manual tick requested (for testing or manual triggers).
    ManualTick,
}

impl CtpSignal {
    /// Returns the timestamp of when this signal was observed.
    pub fn timestamp(&self) -> Option<Instant> {
        match self {
            CtpSignal::WindowChanged(ctx) => Some(ctx.timestamp),
            CtpSignal::ClipboardChanged(digest) => Some(digest.timestamp),
            CtpSignal::FileEvent(event) => Some(event.timestamp),
            CtpSignal::KeystrokePattern(cadence) => Some(cadence.timestamp),
            CtpSignal::ManualTick => None,
        }
    }

    /// Returns a short descriptive name for logging.
    pub fn signal_type(&self) -> &'static str {
        match self {
            CtpSignal::WindowChanged(_) => "window_changed",
            CtpSignal::ClipboardChanged(_) => "clipboard_changed",
            CtpSignal::FileEvent(_) => "file_event",
            CtpSignal::KeystrokePattern(_) => "keystroke_pattern",
            CtpSignal::ManualTick => "manual_tick",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn signal_type_returns_correct_name() {
        let signal = CtpSignal::ManualTick;
        assert_eq!(signal.signal_type(), "manual_tick");
    }

    #[test]
    fn window_changed_signal_has_timestamp() {
        let ctx = WindowContext {
            app_name: "TestApp".to_string(),
            window_title: None,
            bundle_id: None,
            timestamp: Instant::now(),
        };
        let signal = CtpSignal::WindowChanged(ctx);
        assert!(signal.timestamp().is_some());
    }

    #[test]
    fn manual_tick_has_no_timestamp() {
        let signal = CtpSignal::ManualTick;
        assert!(signal.timestamp().is_none());
    }

    #[test]
    fn keystroke_pattern_signal_extracts_timestamp() {
        let now = Instant::now();
        let cadence = KeystrokeCadence {
            events_per_minute: 120.0,
            burst_detected: false,
            idle_duration: Duration::from_secs(5),
            timestamp: now,
        };
        let signal = CtpSignal::KeystrokePattern(cadence);
        assert_eq!(signal.timestamp(), Some(now));
    }
}
