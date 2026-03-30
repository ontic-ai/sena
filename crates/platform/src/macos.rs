//! macOS platform adapter implementation.

#[cfg(target_os = "macos")]
use async_trait::async_trait;
#[cfg(target_os = "macos")]
use bus::events::platform::{ClipboardDigest, FileEvent, KeystrokeCadence, WindowContext};
#[cfg(target_os = "macos")]
use tokio::sync::mpsc;

#[cfg(target_os = "macos")]
use crate::adapter::PlatformAdapter;

/// macOS platform adapter.
#[cfg(target_os = "macos")]
#[derive(Default)]
pub struct MacOSPlatform;

#[cfg(target_os = "macos")]
impl MacOSPlatform {
    /// Create a new macOS platform adapter.
    pub fn new() -> Self {
        Self
    }
}

#[cfg(target_os = "macos")]
#[async_trait]
impl PlatformAdapter for MacOSPlatform {
    fn active_window(&self) -> Option<WindowContext> {
        // TODO M1.5: implement via core-graphics / Accessibility API
        None
    }

    fn clipboard_digest(&self) -> Option<ClipboardDigest> {
        // TODO M1.5: implement via arboard
        None
    }

    fn subscribe_file_events(&self, _tx: mpsc::Sender<FileEvent>) {
        // TODO M1.5: implement via notify crate (kqueue)
    }

    fn subscribe_keystroke_patterns(&self, _tx: mpsc::Sender<KeystrokeCadence>) {
        // TODO M1.5: implement via rdev crate (timing only)
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn macos_platform_constructs() {
        let _platform = MacOSPlatform::new();
    }

    #[test]
    fn active_window_returns_none_stub() {
        let platform = MacOSPlatform::new();
        assert!(platform.active_window().is_none());
    }

    #[test]
    fn clipboard_digest_returns_none_stub() {
        let platform = MacOSPlatform::new();
        assert!(platform.clipboard_digest().is_none());
    }
}
