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
        std::thread::spawn(move || {
            use rdev::{listen, EventType};
            use std::sync::{Arc, Mutex};
            use std::time::{Duration, Instant};

            // Counts keypresses, never captures which key
            let event_count = Arc::new(Mutex::new(0u64));
            let last_event_time = Arc::new(Mutex::new(Instant::now()));
            let window_start = Arc::new(Mutex::new(Instant::now()));

            let event_count_clone = Arc::clone(&event_count);
            let last_event_clone = Arc::clone(&last_event_time);
            let window_clone = Arc::clone(&window_start);
            let tx_clone = tx.clone();

            // Spawn a reporter thread: every 10 seconds, emit a KeystrokeCadence
            std::thread::spawn(move || {
                loop {
                    std::thread::sleep(Duration::from_secs(10));

                    let count = {
                        let mut c = event_count_clone.lock().unwrap_or_else(|e| e.into_inner());
                        let val = *c;
                        *c = 0; // reset
                        val
                    };

                    let window_elapsed = {
                        let mut ws = window_clone.lock().unwrap_or_else(|e| e.into_inner());
                        let elapsed = ws.elapsed();
                        *ws = Instant::now();
                        elapsed
                    };

                    let elapsed_mins = window_elapsed.as_secs_f64() / 60.0;
                    let events_per_minute = if elapsed_mins > 0.0 {
                        count as f64 / elapsed_mins
                    } else {
                        0.0
                    };

                    let idle_duration = {
                        let last = last_event_clone.lock().unwrap_or_else(|e| e.into_inner());
                        last.elapsed()
                    };

                    let burst_detected = events_per_minute > 200.0; // typing burst threshold

                    let cadence = KeystrokeCadence {
                        events_per_minute,
                        burst_detected,
                        idle_duration,
                    };

                    // Send cadence - if channel is closed, stop the reporter
                    if tx_clone.blocking_send(cadence).is_err() {
                        break;
                    }
                }
            });

            // rdev listen loop - ONLY count events, NEVER capture char content
            // This callback receives EventType which includes KeyPress(Key) but we ONLY COUNT
            let result = listen(move |event| {
                if let EventType::KeyPress(_) = event.event_type {
                    // Count only - no character capture
                    if let Ok(mut c) = event_count.lock() {
                        *c += 1;
                    }
                    if let Ok(mut t) = last_event_time.lock() {
                        *t = Instant::now();
                    }
                }
            });

            if let Err(e) = result {
                eprintln!("[platform/linux] rdev listen error: {:?}", e);
            }
        });
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
