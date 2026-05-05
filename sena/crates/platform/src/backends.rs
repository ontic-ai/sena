//! Native platform backend implementations.
//!
//! Provides OS-specific backends that implement `PlatformBackend`.
//! Each target platform has a concrete stub implementation.
//! Real OS-specific signal acquisition is deferred to later milestones.

use crate::backend::PlatformBackend;
use crate::error::PlatformError;
use crate::types::{ClipboardDigest, KeystrokeCadence, PlatformSignal, ScreenFrame, WindowContext};
use std::time::{Duration, Instant};
use tracing::debug;

// ─────────────────────────────────────────────────────────────────────────────
// Windows backend
// ─────────────────────────────────────────────────────────────────────────────

/// Windows-native platform backend.
///
/// Active window detection uses Win32 APIs (GetForegroundWindow + QueryFullProcessImageNameW).
/// Clipboard, keystrokes, and screen capture remain stubs pending full implementation.
#[cfg(target_os = "windows")]
pub struct WindowsBackend;

#[cfg(target_os = "windows")]
impl WindowsBackend {
    /// Construct the Windows backend.
    pub fn new() -> Result<Self, PlatformError> {
        debug!("WindowsBackend initializing");
        Ok(Self)
    }
}

#[cfg(target_os = "windows")]
impl PlatformBackend for WindowsBackend {
    fn active_window(&self) -> Result<PlatformSignal, PlatformError> {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::System::Threading::{
            OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
            QueryFullProcessImageNameW,
        };
        use windows::Win32::UI::WindowsAndMessaging::{
            GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
        };

        // SAFETY: Win32 calls follow documented contracts.
        let (app_name, window_title) = unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd == HWND(std::ptr::null_mut()) {
                return Ok(PlatformSignal::Window(WindowContext {
                    app_name: "Unknown".to_string(),
                    window_title: None,
                    bundle_id: None,
                    timestamp: Instant::now(),
                }));
            }

            // Window title
            let mut title_buf = [0u16; 512];
            let title_len = GetWindowTextW(hwnd, &mut title_buf) as usize;
            let window_title = if title_len > 0 {
                Some(String::from_utf16_lossy(&title_buf[..title_len]))
            } else {
                None
            };

            // Process name from PID
            let mut pid: u32 = 0;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            let app_name = if pid != 0 {
                match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                    Ok(handle) => {
                        let mut name_buf = [0u16; 260];
                        let mut size: u32 = 260;
                        let ok = QueryFullProcessImageNameW(
                            handle,
                            PROCESS_NAME_WIN32,
                            windows::core::PWSTR(name_buf.as_mut_ptr()),
                            &mut size,
                        );
                        let _ = windows::Win32::Foundation::CloseHandle(handle);
                        if ok.is_ok() {
                            let full_path = String::from_utf16_lossy(&name_buf[..size as usize]);
                            std::path::Path::new(&full_path)
                                .file_stem()
                                .and_then(|s| s.to_str())
                                .unwrap_or("Unknown")
                                .to_string()
                        } else {
                            "Unknown".to_string()
                        }
                    }
                    Err(_) => "Unknown".to_string(),
                }
            } else {
                "Unknown".to_string()
            };

            (app_name, window_title)
        };

        Ok(PlatformSignal::Window(WindowContext {
            app_name,
            window_title,
            bundle_id: None,
            timestamp: Instant::now(),
        }))
    }

    fn clipboard_content(&self) -> Result<PlatformSignal, PlatformError> {
        Ok(PlatformSignal::Clipboard(ClipboardDigest {
            digest: None,
            char_count: 0,
            timestamp: Instant::now(),
        }))
    }

    fn keystroke_cadence(&self) -> Result<PlatformSignal, PlatformError> {
        Ok(PlatformSignal::Keystroke(KeystrokeCadence {
            events_per_minute: 0.0,
            burst_detected: false,
            idle_duration: Duration::from_secs(0),
            timestamp: Instant::now(),
        }))
    }

    fn screen_frame(&self) -> Result<PlatformSignal, PlatformError> {
        Ok(PlatformSignal::ScreenFrame(ScreenFrame {
            width: 1,
            height: 1,
            rgb_data: vec![0, 0, 0],
            timestamp: Instant::now(),
        }))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// macOS backend
// ─────────────────────────────────────────────────────────────────────────────

/// macOS-native platform backend.
///
/// BONES stub: returns typed defaults for all signals.
/// Real implementation will use Core Graphics / core-foundation.
#[cfg(target_os = "macos")]
pub struct MacOsBackend;

#[cfg(target_os = "macos")]
impl MacOsBackend {
    /// Construct the macOS backend.
    pub fn new() -> Result<Self, PlatformError> {
        debug!("MacOsBackend initializing (BONES stub)");
        Ok(Self)
    }
}

#[cfg(target_os = "macos")]
impl PlatformBackend for MacOsBackend {
    fn active_window(&self) -> Result<PlatformSignal, PlatformError> {
        Ok(PlatformSignal::Window(WindowContext {
            app_name: "Unknown".to_string(),
            window_title: None,
            bundle_id: None,
            timestamp: Instant::now(),
        }))
    }

    fn clipboard_content(&self) -> Result<PlatformSignal, PlatformError> {
        Ok(PlatformSignal::Clipboard(ClipboardDigest {
            digest: None,
            char_count: 0,
            timestamp: Instant::now(),
        }))
    }

    fn keystroke_cadence(&self) -> Result<PlatformSignal, PlatformError> {
        Ok(PlatformSignal::Keystroke(KeystrokeCadence {
            events_per_minute: 0.0,
            burst_detected: false,
            idle_duration: Duration::from_secs(0),
            timestamp: Instant::now(),
        }))
    }

    fn screen_frame(&self) -> Result<PlatformSignal, PlatformError> {
        Ok(PlatformSignal::ScreenFrame(ScreenFrame {
            width: 1,
            height: 1,
            rgb_data: vec![0, 0, 0],
            timestamp: Instant::now(),
        }))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Linux backend
// ─────────────────────────────────────────────────────────────────────────────

/// Linux-native platform backend.
///
/// BONES stub: returns typed defaults.
/// TODO M1.5: implement active window via x11rb.
#[cfg(target_os = "linux")]
pub struct LinuxBackend;

#[cfg(target_os = "linux")]
impl LinuxBackend {
    /// Construct the Linux backend.
    pub fn new() -> Result<Self, PlatformError> {
        debug!("LinuxBackend initializing (BONES stub)");
        Ok(Self)
    }
}

#[cfg(target_os = "linux")]
impl PlatformBackend for LinuxBackend {
    fn active_window(&self) -> Result<PlatformSignal, PlatformError> {
        // TODO M1.5: implement via x11rb
        Ok(PlatformSignal::Window(WindowContext {
            app_name: "Unknown".to_string(),
            window_title: None,
            bundle_id: None,
            timestamp: Instant::now(),
        }))
    }

    fn clipboard_content(&self) -> Result<PlatformSignal, PlatformError> {
        Ok(PlatformSignal::Clipboard(ClipboardDigest {
            digest: None,
            char_count: 0,
            timestamp: Instant::now(),
        }))
    }

    fn keystroke_cadence(&self) -> Result<PlatformSignal, PlatformError> {
        Ok(PlatformSignal::Keystroke(KeystrokeCadence {
            events_per_minute: 0.0,
            burst_detected: false,
            idle_duration: Duration::from_secs(0),
            timestamp: Instant::now(),
        }))
    }

    fn screen_frame(&self) -> Result<PlatformSignal, PlatformError> {
        Ok(PlatformSignal::ScreenFrame(ScreenFrame {
            width: 1,
            height: 1,
            rgb_data: vec![0, 0, 0],
            timestamp: Instant::now(),
        }))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Platform-native type alias
// ─────────────────────────────────────────────────────────────────────────────

/// The native backend for the current target platform.
///
/// Resolves to the appropriate OS-specific backend struct at compile time.
/// `PlatformActor::native()` uses this alias to construct the correct backend
/// without conditional compilation at the call site.
#[cfg(target_os = "windows")]
pub type NativeBackend = WindowsBackend;

#[cfg(target_os = "macos")]
pub type NativeBackend = MacOsBackend;

#[cfg(target_os = "linux")]
pub type NativeBackend = LinuxBackend;
