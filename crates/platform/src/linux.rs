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
