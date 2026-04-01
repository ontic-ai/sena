//! Windows platform adapter implementation.

#[cfg(target_os = "windows")]
use std::hash::{Hash, Hasher};
#[cfg(target_os = "windows")]
use std::time::Instant;

#[cfg(target_os = "windows")]
use async_trait::async_trait;
#[cfg(target_os = "windows")]
use bus::events::platform::{ClipboardDigest, FileEvent, KeystrokeCadence, WindowContext};
#[cfg(target_os = "windows")]
use tokio::sync::mpsc;

#[cfg(target_os = "windows")]
use crate::adapter::PlatformAdapter;

/// Windows platform adapter.
#[cfg(target_os = "windows")]
#[derive(Default)]
pub struct WindowsPlatform;

#[cfg(target_os = "windows")]
impl WindowsPlatform {
    /// Create a new Windows platform adapter.
    pub fn new() -> Self {
        Self
    }

    /// Get the foreground window title via Win32 API.
    fn foreground_window_title() -> Option<String> {
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW};

        // Safety: GetForegroundWindow is documented as safe to call from any thread.
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.is_null() {
            return None;
        }

        let mut buf = [0u16; 512];
        // Safety: hwnd is a valid nonzero window handle; buf is properly sized.
        let len = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
        if len <= 0 {
            return None;
        }

        let title = String::from_utf16_lossy(&buf[..len as usize]);
        if title.is_empty() {
            None
        } else {
            Some(title)
        }
    }
}

#[cfg(target_os = "windows")]
#[async_trait]
impl PlatformAdapter for WindowsPlatform {
    fn active_window(&self) -> Option<WindowContext> {
        let title = Self::foreground_window_title()?;
        Some(WindowContext {
            app_name: title.clone(),
            window_title: Some(title),
            bundle_id: None,
            timestamp: Instant::now(),
        })
    }

    fn clipboard_digest(&self) -> Option<ClipboardDigest> {
        let text = arboard::Clipboard::new()
            .ok()
            .and_then(|mut cb| cb.get_text().ok())?;

        if text.is_empty() {
            return None;
        }

        let char_count = text.chars().count();

        // Hash the content â€” never store raw clipboard text.
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        let digest_hex = format!("{:016x}", hasher.finish());

        Some(ClipboardDigest {
            digest: Some(digest_hex),
            char_count,
            timestamp: Instant::now(),
        })
    }

    fn subscribe_file_events(&self, _tx: mpsc::Sender<FileEvent>) {
        // TODO M1.5: implement via notify crate
    }

    fn subscribe_keystroke_patterns(&self, tx: mpsc::Sender<KeystrokeCadence>) {
        crate::adapter::spawn_keystroke_pattern_monitor(tx, "windows");
    }
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    #[test]
    fn windows_platform_constructs() {
        let _platform = WindowsPlatform::new();
    }

    #[test]
    fn active_window_does_not_panic() {
        // On a real Windows machine there should always be a foreground window,
        // but in headless CI there may not be. Either way must not panic.
        let platform = WindowsPlatform::new();
        let _ = platform.active_window();
    }
}
