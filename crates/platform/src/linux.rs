//! Linux platform adapter implementation.

#[cfg(target_os = "linux")]
use async_trait::async_trait;
#[cfg(target_os = "linux")]
use bus::events::platform::{ClipboardDigest, FileEvent, KeystrokeCadence, WindowContext};
#[cfg(target_os = "linux")]
use tokio::sync::mpsc;

#[cfg(target_os = "linux")]
use crate::adapter::PlatformAdapter;

/// Linux platform adapter.
#[cfg(target_os = "linux")]
#[derive(Default)]
pub struct LinuxPlatform;

#[cfg(target_os = "linux")]
impl LinuxPlatform {
    /// Create a new Linux platform adapter.
    pub fn new() -> Self {
        Self
    }
}

#[cfg(target_os = "linux")]
#[async_trait]
impl PlatformAdapter for LinuxPlatform {
    fn active_window(&self) -> Option<WindowContext> {
        // TODO M1.5: implement via x11rb / atspi
        None
    }

    fn clipboard_digest(&self) -> Option<ClipboardDigest> {
        // TODO M1.5: implement via arboard
        None
    }

    fn subscribe_file_events(&self, _tx: mpsc::Sender<FileEvent>) {
        // TODO M1.5: implement via notify crate (inotify)
    }

    fn subscribe_keystroke_patterns(&self, tx: mpsc::Sender<KeystrokeCadence>) {
        crate::adapter::spawn_keystroke_pattern_monitor(tx, "linux");
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn linux_platform_constructs() {
        let _platform = LinuxPlatform::new();
    }

    #[test]
    fn active_window_returns_none_stub() {
        let platform = LinuxPlatform::new();
        assert!(platform.active_window().is_none());
    }

    #[test]
    fn clipboard_digest_returns_none_stub() {
        let platform = LinuxPlatform::new();
        assert!(platform.clipboard_digest().is_none());
    }
}
