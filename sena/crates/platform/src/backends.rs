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

#[cfg(target_os = "windows")]
use arboard::Clipboard;
#[cfg(target_os = "windows")]
use sha2::{Digest, Sha256};
#[cfg(target_os = "windows")]
use std::collections::VecDeque;
#[cfg(target_os = "windows")]
use std::sync::Mutex;

// ─────────────────────────────────────────────────────────────────────────────
// Windows backend
// ─────────────────────────────────────────────────────────────────────────────

/// Windows-native platform backend.
///
/// Active window detection uses Win32 APIs (GetForegroundWindow + QueryFullProcessImageNameW).
/// Clipboard uses the system clipboard and keystrokes are sampled from Win32 key state.
/// Screen capture remains a typed placeholder.
#[cfg(target_os = "windows")]
pub struct WindowsBackend {
    keystroke_state: Mutex<WindowsKeystrokeState>,
}

#[cfg(target_os = "windows")]
struct WindowsKeystrokeState {
    previous_key_down: [bool; 256],
    recent_key_downs: VecDeque<Instant>,
    last_event_at: Option<Instant>,
    burst_idle_duration: Duration,
}

#[cfg(target_os = "windows")]
impl WindowsKeystrokeState {
    fn new() -> Self {
        Self {
            previous_key_down: [false; 256],
            recent_key_downs: VecDeque::new(),
            last_event_at: None,
            burst_idle_duration: Duration::from_secs(0),
        }
    }

    fn sample(&mut self) -> KeystrokeCadence {
        use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;

        const BURST_WINDOW: Duration = Duration::from_secs(3);
        const BURST_MIN_KEYS: usize = 6;
        const BURST_IDLE_RESET: Duration = Duration::from_secs(2);
        const HISTORY_WINDOW: Duration = Duration::from_secs(60);

        let now = Instant::now();

        for virtual_key in 0x08u16..=0xFEu16 {
            let key_index = virtual_key as usize;
            let key_down = unsafe { GetAsyncKeyState(virtual_key as i32) } < 0;

            if key_down && !self.previous_key_down[key_index] {
                let idle_gap = self
                    .last_event_at
                    .map(|last| now.saturating_duration_since(last))
                    .unwrap_or(Duration::from_secs(3600));
                if idle_gap >= BURST_IDLE_RESET {
                    self.burst_idle_duration = idle_gap;
                }

                self.recent_key_downs.push_back(now);
                self.last_event_at = Some(now);
            }

            self.previous_key_down[key_index] = key_down;
        }

        while self
            .recent_key_downs
            .front()
            .is_some_and(|timestamp| now.saturating_duration_since(*timestamp) > HISTORY_WINDOW)
        {
            self.recent_key_downs.pop_front();
        }

        let burst_count = self
            .recent_key_downs
            .iter()
            .rev()
            .take_while(|timestamp| now.saturating_duration_since(**timestamp) <= BURST_WINDOW)
            .count();
        let burst_detected = burst_count >= BURST_MIN_KEYS;

        let idle_duration = if burst_detected {
            self.burst_idle_duration
        } else {
            self.last_event_at
                .map(|last| now.saturating_duration_since(last))
                .unwrap_or(Duration::from_secs(3600))
        };

        KeystrokeCadence {
            events_per_minute: self.recent_key_downs.len() as f64,
            burst_detected,
            idle_duration,
            timestamp: now,
        }
    }
}

#[cfg(target_os = "windows")]
impl WindowsBackend {
    /// Construct the Windows backend.
    pub fn new() -> Result<Self, PlatformError> {
        debug!("WindowsBackend initializing");
        Ok(Self {
            keystroke_state: Mutex::new(WindowsKeystrokeState::new()),
        })
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
        let mut clipboard = Clipboard::new()
            .map_err(|e| PlatformError::ClipboardFailed(format!("clipboard open failed: {}", e)))?;
        let text = clipboard
            .get_text()
            .map_err(|e| PlatformError::ClipboardFailed(format!("clipboard read failed: {}", e)))?;
        let digest = if text.is_empty() {
            None
        } else {
            Some(format!("{:x}", Sha256::digest(text.as_bytes())))
        };

        Ok(PlatformSignal::Clipboard(ClipboardDigest {
            digest,
            char_count: text.chars().count(),
            timestamp: Instant::now(),
        }))
    }

    fn keystroke_cadence(&self) -> Result<PlatformSignal, PlatformError> {
        let mut state = self.keystroke_state.lock().map_err(|_| {
            PlatformError::KeystrokeCadenceFailed("keystroke state mutex poisoned".to_string())
        })?;

        Ok(PlatformSignal::Keystroke(KeystrokeCadence {
            ..state.sample()
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
