//! Signal buffer — rolling time-window accumulator for CTP platform signals.
//!
//! The signal buffer collects typed platform signals within a configurable
//! time window. Signals older than the window are pruned via `prune()`.
//! The `ContextAssembler` reads the buffer to assemble `ContextSnapshot`s.

use platform::{ClipboardDigest, FileEvent, KeystrokeCadence, WindowContext};
use std::time::{Duration, Instant};

/// An entry in the signal buffer with its capture timestamp.
#[derive(Debug, Clone)]
struct Timestamped<T> {
    value: T,
    captured_at: Instant,
}

impl<T> Timestamped<T> {
    fn new(value: T) -> Self {
        Self {
            value,
            captured_at: Instant::now(),
        }
    }

    fn is_within_window(&self, window: Duration) -> bool {
        self.captured_at.elapsed() <= window
    }
}

/// Rolling time-window accumulator for CTP platform signals.
///
/// Holds the last N seconds (configurable via `window`) of each signal type.
/// `prune()` must be called periodically to remove stale entries.
pub struct SignalBuffer {
    /// Time window for signal retention.
    window: Duration,
    /// Window context history.
    windows: Vec<Timestamped<WindowContext>>,
    /// Clipboard digest history.
    clipboard: Vec<Timestamped<ClipboardDigest>>,
    /// File system event history.
    file_events: Vec<Timestamped<FileEvent>>,
    /// Keystroke cadence history.
    keystrokes: Vec<Timestamped<KeystrokeCadence>>,
}

impl SignalBuffer {
    /// Create a new signal buffer with the given time window.
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            windows: Vec::new(),
            clipboard: Vec::new(),
            file_events: Vec::new(),
            keystrokes: Vec::new(),
        }
    }

    /// Push a window context signal into the buffer.
    pub fn push_window(&mut self, ctx: WindowContext) {
        self.windows.push(Timestamped::new(ctx));
    }

    /// Push a clipboard digest signal into the buffer.
    pub fn push_clipboard(&mut self, digest: ClipboardDigest) {
        self.clipboard.push(Timestamped::new(digest));
    }

    /// Push a file system event into the buffer.
    pub fn push_file_event(&mut self, event: FileEvent) {
        self.file_events.push(Timestamped::new(event));
    }

    /// Push a keystroke cadence signal into the buffer.
    pub fn push_keystroke(&mut self, cadence: KeystrokeCadence) {
        self.keystrokes.push(Timestamped::new(cadence));
    }

    /// Remove signals older than the configured time window.
    pub fn prune(&mut self) {
        let window = self.window;
        self.windows.retain(|e| e.is_within_window(window));
        self.clipboard.retain(|e| e.is_within_window(window));
        self.file_events.retain(|e| e.is_within_window(window));
        self.keystrokes.retain(|e| e.is_within_window(window));
    }

    /// Most recent window context, if any.
    pub fn latest_window(&self) -> Option<&WindowContext> {
        self.windows.last().map(|e| &e.value)
    }

    /// Most recent clipboard digest, if any.
    pub fn latest_clipboard(&self) -> Option<&ClipboardDigest> {
        self.clipboard.last().map(|e| &e.value)
    }

    /// Most recent keystroke cadence, if any.
    pub fn latest_keystroke(&self) -> Option<&KeystrokeCadence> {
        self.keystrokes.last().map(|e| &e.value)
    }

    /// All file events within the window.
    pub fn file_events(&self) -> impl Iterator<Item = &FileEvent> {
        self.file_events.iter().map(|e| &e.value)
    }

    /// Number of window change events in the buffer.
    pub fn window_event_count(&self) -> usize {
        self.windows.len()
    }

    /// Number of keystroke events in the buffer.
    pub fn keystroke_event_count(&self) -> usize {
        self.keystrokes.len()
    }

    /// Total number of buffered signals across all types.
    pub fn total_count(&self) -> usize {
        self.windows.len()
            + self.clipboard.len()
            + self.file_events.len()
            + self.keystrokes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn make_window(app: &str) -> WindowContext {
        WindowContext {
            app_name: app.to_string(),
            window_title: None,
            bundle_id: None,
            timestamp: Instant::now(),
        }
    }

    #[test]
    fn buffer_stores_and_retrieves_window() {
        let mut buf = SignalBuffer::new(Duration::from_secs(60));
        buf.push_window(make_window("TestApp"));
        assert_eq!(buf.latest_window().map(|w| w.app_name.as_str()), Some("TestApp"));
        assert_eq!(buf.window_event_count(), 1);
    }

    #[test]
    fn buffer_prune_removes_nothing_within_window() {
        let mut buf = SignalBuffer::new(Duration::from_secs(60));
        buf.push_window(make_window("App1"));
        buf.prune();
        assert_eq!(buf.window_event_count(), 1);
    }
}
