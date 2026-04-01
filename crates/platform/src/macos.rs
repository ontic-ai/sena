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
        use std::hash::{Hash, Hasher};
        use std::time::Instant;

        let text = arboard::Clipboard::new()
            .ok()
            .and_then(|mut cb| cb.get_text().ok())?;

        if text.is_empty() {
            return None;
        }

        let char_count = text.chars().count();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        let digest_hex = format!("{:016x}", hasher.finish());

        Some(ClipboardDigest {
            digest: Some(digest_hex),
            char_count,
            timestamp: Instant::now(),
        })
    }

    fn subscribe_file_events(&self, tx: mpsc::Sender<FileEvent>, paths: &[std::path::PathBuf]) {
        crate::adapter::spawn_file_event_watcher(tx, paths.to_vec());
    }

    fn subscribe_keystroke_patterns(&self, tx: mpsc::Sender<KeystrokeCadence>) {
        crate::adapter::spawn_keystroke_pattern_monitor(tx, "macos");
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
