//! Platform adapter trait for OS signal collection.

use async_trait::async_trait;
use bus::events::platform::{ClipboardDigest, FileEvent, KeystrokeCadence, WindowContext};
use tokio::sync::mpsc;

/// Platform adapter trait for OS signal collection.
///
/// Each OS implementation provides concrete signal collection.
/// All methods are designed to be privacy-preserving:
/// - Clipboard returns digest only, never raw content
/// - Keystroke patterns capture timing only, never characters
#[async_trait]
pub trait PlatformAdapter: Send + 'static {
    /// Get the currently active window context.
    ///
    /// Returns None if window information is unavailable or
    /// the implementation is not yet complete.
    fn active_window(&self) -> Option<WindowContext>;

    /// Get the current clipboard digest (never raw content).
    ///
    /// Returns None if clipboard is empty, unavailable, or
    /// the implementation is not yet complete.
    fn clipboard_digest(&self) -> Option<ClipboardDigest>;

    /// Subscribe to file system events.
    ///
    /// The adapter will send FileEvent instances to the provided channel
    /// when file system changes are detected.
    fn subscribe_file_events(&self, tx: mpsc::Sender<FileEvent>);

    /// Subscribe to keystroke cadence patterns (timing only, never characters).
    ///
    /// The adapter will send KeystrokeCadence instances to the provided channel.
    /// PRIVACY: No character data is ever captured or transmitted.
    fn subscribe_keystroke_patterns(&self, tx: mpsc::Sender<KeystrokeCadence>);
}
