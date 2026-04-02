//! macOS platform adapter implementation.

#[cfg(target_os = "macos")]
use async_trait::async_trait;
#[cfg(target_os = "macos")]
use bus::events::platform::{ClipboardDigest, FileEvent, KeystrokeCadence, WindowContext};
#[cfg(target_os = "macos")]
use bus::events::platform_vision::ImageDigest;
#[cfg(target_os = "macos")]
use tokio::sync::mpsc;

#[cfg(target_os = "macos")]
use crate::adapter::PlatformAdapter;
#[cfg(target_os = "macos")]
use crate::error::PlatformError;

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

    fn screen_capture(&self) -> Result<ImageDigest, PlatformError> {
        use sha2::{Digest, Sha256};

        // Use Core Graphics to capture the main display.
        // Safety: This uses the core-graphics crate which provides safe FFI bindings.

        // Get the main display
        let display_id = unsafe { core_graphics::display::CGMainDisplayID() };

        // Capture the screen image
        let image = unsafe { core_graphics::display::CGDisplayCreateImage(display_id) };

        if image.is_null() {
            return Err(PlatformError::NotAvailable(
                "failed to capture screen image".to_string(),
            ));
        }

        // Get image properties
        let width = unsafe { core_graphics::sys::CGImageGetWidth(image) };
        let height = unsafe { core_graphics::sys::CGImageGetHeight(image) };
        let bytes_per_row = unsafe { core_graphics::sys::CGImageGetBytesPerRow(image) };

        // Get the data provider and pixel data
        let data_provider = unsafe { core_graphics::sys::CGImageGetDataProvider(image) };
        if data_provider.is_null() {
            unsafe { core_graphics::sys::CGImageRelease(image) };
            return Err(PlatformError::NotAvailable(
                "failed to get image data provider".to_string(),
            ));
        }

        let data = unsafe { core_graphics::sys::CGDataProviderCopyData(data_provider) };
        if data.is_null() {
            unsafe { core_graphics::sys::CGImageRelease(image) };
            return Err(PlatformError::NotAvailable(
                "failed to copy image data".to_string(),
            ));
        }

        // Get the raw bytes
        let length = unsafe { core_graphics::sys::CFDataGetLength(data) };
        let bytes_ptr = unsafe { core_graphics::sys::CFDataGetBytePtr(data) };

        // CRITICAL: Hash pixels immediately — never return or store them.
        let pixels = unsafe { std::slice::from_raw_parts(bytes_ptr, length as usize) };
        let mut hasher = Sha256::new();
        hasher.update(pixels);
        let hash_result = hasher.finalize();

        // Clean up Core Graphics resources
        unsafe {
            core_graphics::sys::CFRelease(data as *const _);
            core_graphics::sys::CGImageRelease(image);
        }

        // Copy hash into fixed-size array
        let mut digest_bytes = [0u8; 32];
        digest_bytes.copy_from_slice(&hash_result);

        Ok(ImageDigest::new(digest_bytes))
    }

    fn screen_capture_png(&self, _max_dim: u32) -> Result<Vec<u8>, PlatformError> {
        // TODO M5.1: implement via ScreenCaptureKit on macOS
        Err(PlatformError::NotAvailable(
            "screen_capture_png not yet implemented on macOS".to_string(),
        ))
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

    #[test]
    fn screen_capture_returns_digest_or_error() {
        // On a real macOS machine with a display, this should succeed.
        // In headless CI, it may fail but must not panic.
        let platform = MacOSPlatform::new();
        let result = platform.screen_capture();

        match result {
            Ok(digest) => {
                // Digest should be 32 bytes
                assert_eq!(digest.as_bytes().len(), 32);
                // Debug output should be redacted
                let debug_str = format!("{:?}", digest);
                assert!(debug_str.contains("REDACTED"));
            }
            Err(e) => {
                // On headless or if capture fails, error must be descriptive
                let err_str = format!("{}", e);
                assert!(!err_str.is_empty());
            }
        }
    }
}
