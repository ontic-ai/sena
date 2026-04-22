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
/// BONES stub: returns typed defaults for all signals.
/// Real implementation will use the Win32 API for active-window, clipboard, etc.
#[cfg(target_os = "windows")]
pub struct WindowsBackend;

#[cfg(target_os = "windows")]
impl WindowsBackend {
    /// Construct the Windows backend.
    pub fn new() -> Result<Self, PlatformError> {
        debug!("WindowsBackend initializing (BONES stub)");
        Ok(Self)
    }
}

#[cfg(target_os = "windows")]
impl PlatformBackend for WindowsBackend {
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
