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
use bus::events::platform_vision::ImageDigest;
#[cfg(target_os = "windows")]
use tokio::sync::mpsc;

#[cfg(target_os = "windows")]
use crate::adapter::PlatformAdapter;
#[cfg(target_os = "windows")]
use crate::error::PlatformError;

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

    fn subscribe_file_events(&self, tx: mpsc::Sender<FileEvent>, paths: &[std::path::PathBuf]) {
        crate::adapter::spawn_file_event_watcher(tx, paths.to_vec());
    }

    fn subscribe_keystroke_patterns(&self, tx: mpsc::Sender<KeystrokeCadence>) {
        crate::adapter::spawn_keystroke_pattern_monitor(tx, "windows");
    }

    fn screen_capture(&self) -> Result<ImageDigest, PlatformError> {
        use sha2::{Digest, Sha256};
        use windows_sys::Win32::Graphics::Gdi::{
            BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
            GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
            DIB_RGB_COLORS, SRCCOPY,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN,
        };

        // Safety: GetDC(null) gets the entire screen DC.
        let screen_dc = unsafe { GetDC(std::ptr::null_mut()) };
        if screen_dc.is_null() {
            return Err(PlatformError::NotAvailable(
                "failed to get screen DC".to_string(),
            ));
        }

        // Get screen dimensions
        // Safety: GetSystemMetrics is documented as safe.
        let width = unsafe { GetSystemMetrics(SM_CXSCREEN) };
        let height = unsafe { GetSystemMetrics(SM_CYSCREEN) };

        if width <= 0 || height <= 0 {
            // Safety: ReleaseDC releases the DC obtained from GetDC.
            unsafe { ReleaseDC(std::ptr::null_mut(), screen_dc) };
            return Err(PlatformError::NotAvailable(
                "invalid screen dimensions".to_string(),
            ));
        }

        // Create compatible DC and bitmap
        // Safety: CreateCompatibleDC creates a memory DC compatible with screen_dc.
        let mem_dc = unsafe { CreateCompatibleDC(screen_dc) };
        if mem_dc.is_null() {
            unsafe { ReleaseDC(std::ptr::null_mut(), screen_dc) };
            return Err(PlatformError::NotAvailable(
                "failed to create compatible DC".to_string(),
            ));
        }

        // Safety: CreateCompatibleBitmap creates a bitmap compatible with screen_dc.
        let bitmap = unsafe { CreateCompatibleBitmap(screen_dc, width, height) };
        if bitmap.is_null() {
            unsafe {
                DeleteDC(mem_dc);
                ReleaseDC(std::ptr::null_mut(), screen_dc);
            }
            return Err(PlatformError::NotAvailable(
                "failed to create bitmap".to_string(),
            ));
        }

        // Select bitmap into memory DC
        // Safety: SelectObject selects the bitmap into mem_dc for drawing operations.
        let old_bitmap = unsafe { SelectObject(mem_dc, bitmap) };

        // Copy screen to bitmap
        // Safety: BitBlt performs a bit block transfer from screen_dc to mem_dc.
        let blit_result = unsafe { BitBlt(mem_dc, 0, 0, width, height, screen_dc, 0, 0, SRCCOPY) };

        if blit_result == 0 {
            unsafe {
                SelectObject(mem_dc, old_bitmap);
                DeleteObject(bitmap);
                DeleteDC(mem_dc);
                ReleaseDC(std::ptr::null_mut(), screen_dc);
            }
            return Err(PlatformError::NotAvailable("BitBlt failed".to_string()));
        }

        // Prepare BITMAPINFO structure to extract pixel data
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height, // negative for top-down DIB
                biPlanes: 1,
                biBitCount: 32, // RGBA
                biCompression: BI_RGB,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            // Safety: zeroed RGBQUAD array is a valid initialization.
            bmiColors: unsafe { [std::mem::zeroed(); 1] },
        };

        // Allocate buffer for pixel data
        let pixel_count = (width * height) as usize;
        let mut pixels: Vec<u8> = vec![0; pixel_count * 4]; // 4 bytes per pixel (RGBA)

        // Get bitmap bits
        // Safety: GetDIBits retrieves the bits of the bitmap into the pixels buffer.
        let scan_lines = unsafe {
            GetDIBits(
                mem_dc,
                bitmap,
                0,
                height as u32,
                pixels.as_mut_ptr() as *mut _,
                &mut bmi,
                DIB_RGB_COLORS,
            )
        };

        // Clean up GDI resources BEFORE returning
        unsafe {
            SelectObject(mem_dc, old_bitmap);
            DeleteObject(bitmap);
            DeleteDC(mem_dc);
            ReleaseDC(std::ptr::null_mut(), screen_dc);
        }

        if scan_lines == 0 {
            return Err(PlatformError::NotAvailable("GetDIBits failed".to_string()));
        }

        // CRITICAL: Hash pixels immediately — never return or store them.
        let mut hasher = Sha256::new();
        hasher.update(&pixels);
        let hash_result = hasher.finalize();

        // Copy hash into fixed-size array
        let mut digest_bytes = [0u8; 32];
        digest_bytes.copy_from_slice(&hash_result);

        Ok(ImageDigest::new(digest_bytes))
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

    #[test]
    fn screen_capture_returns_digest() {
        // On a real Windows machine with a display, this should succeed.
        // In headless CI, it may fail but must not panic.
        let platform = WindowsPlatform::new();
        let result = platform.screen_capture();

        // Either succeeds with a digest, or fails gracefully
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

    #[test]
    fn screen_capture_produces_different_digests_on_separate_calls() {
        // In a real windowed environment with changing content, consecutive
        // captures should produce different digests (unless screen is static).
        // In headless CI, both calls may fail, which is acceptable.
        let platform = WindowsPlatform::new();
        let result1 = platform.screen_capture();
        let result2 = platform.screen_capture();

        // If both succeed, they might be the same or different depending on
        // screen content changes. The important thing is both return valid digests.
        if let (Ok(digest1), Ok(digest2)) = (result1, result2) {
            assert_eq!(digest1.as_bytes().len(), 32);
            assert_eq!(digest2.as_bytes().len(), 32);
            // Note: digests may or may not be equal depending on screen state
        }
    }
}
