//! Platform adapter — subscribe to typed signal streams from the platform actor.

use crate::types::{ClipboardDigest, FileEvent, KeystrokeCadence, WindowContext};
use std::sync::Arc;
use tokio::sync::broadcast;

/// High-level subscription interface to platform signals.
///
/// Implemented by PlatformActor so callers can subscribe to individual signal
/// streams without knowing about the polling internals.
pub trait PlatformAdapter: Send + Sync {
    /// Subscribe to active window change notifications.
    fn subscribe_active_window(&self) -> broadcast::Receiver<WindowContext>;

    /// Subscribe to clipboard change notifications.
    fn subscribe_clipboard(&self) -> broadcast::Receiver<ClipboardDigest>;

    /// Subscribe to keystroke cadence notifications.
    fn subscribe_keystrokes(&self) -> broadcast::Receiver<KeystrokeCadence>;

    /// Subscribe to file system event notifications.
    fn subscribe_file_events(&self) -> broadcast::Receiver<FileEvent>;

    /// Return the most recent captured vision frame, if any.
    ///
    /// Returns `None` if no frame has been captured yet.
    fn latest_vision_frame(&self) -> Option<Arc<Vec<u8>>>;
}
