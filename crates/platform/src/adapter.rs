//! Platform adapter trait for OS signal collection.

use async_trait::async_trait;
use bus::events::platform::{ClipboardDigest, FileEvent, KeystrokeCadence, WindowContext};
use bus::events::platform_vision::ImageDigest;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use crate::error::PlatformError;

/// Platform adapter trait for OS signal collection.
///
/// Each OS implementation provides concrete signal collection.
/// All methods are designed to be privacy-preserving:
/// - Clipboard returns digest only, never raw content
/// - Keystroke patterns capture timing only, never characters
#[async_trait]
pub trait PlatformAdapter: Send + 'static {
    /// Get the currently active window context.
    ///
    /// Returns None if window information is unavailable or
    /// the implementation is not yet complete.
    fn active_window(&self) -> Option<WindowContext>;

    /// Get the current clipboard digest (never raw content).
    ///
    /// Returns None if clipboard is empty, unavailable, or
    /// the implementation is not yet complete.
    fn clipboard_digest(&self) -> Option<ClipboardDigest>;

    /// Subscribe to file system events on the given watch paths.
    ///
    /// The adapter spawns a background watcher thread that sends FileEvent instances
    /// to the provided channel when file system changes are detected on any of the
    /// provided paths. No-op when `paths` is empty.
    fn subscribe_file_events(&self, tx: mpsc::Sender<FileEvent>, paths: &[PathBuf]);

    /// Subscribe to keystroke cadence patterns (timing only, never characters).
    ///
    /// The adapter will send KeystrokeCadence instances to the provided channel.
    /// PRIVACY: No character data is ever captured or transmitted.
    fn subscribe_keystroke_patterns(&self, tx: mpsc::Sender<KeystrokeCadence>);

    /// Capture screen content and return a SHA256 digest.
    ///
    /// PRIVACY-CRITICAL: Raw pixels MUST be hashed immediately within this method.
    /// They are NEVER returned or stored. Only the digest is returned.
    ///
    /// Returns Err(PlatformError::ScreenCaptureNotImplemented) if not yet implemented
    /// on this platform.
    fn screen_capture(&self) -> Result<ImageDigest, PlatformError>;
}

/// Spawn a privacy-safe global keystroke cadence monitor.
///
/// The monitor records counts and timing only. It never stores key identity.
pub(crate) fn spawn_keystroke_pattern_monitor(
    tx: mpsc::Sender<KeystrokeCadence>,
    platform_label: &'static str,
) {
    let event_count = Arc::new(AtomicU64::new(0));
    let last_event_time = Arc::new(Mutex::new(Instant::now()));
    let listener_alive = Arc::new(AtomicBool::new(true));

    let reporter_count = Arc::clone(&event_count);
    let reporter_last_event = Arc::clone(&last_event_time);
    let reporter_alive = Arc::clone(&listener_alive);
    let reporter_tx = tx.clone();

    std::thread::spawn(move || {
        let mut last_report = Instant::now();

        loop {
            std::thread::sleep(Duration::from_secs(2));

            if !reporter_alive.load(Ordering::Relaxed) {
                break;
            }

            let now = Instant::now();
            let window_elapsed = now.saturating_duration_since(last_report);
            last_report = now;

            let count = reporter_count.swap(0, Ordering::Relaxed);
            let elapsed_mins = window_elapsed.as_secs_f64() / 60.0;
            let events_per_minute = if elapsed_mins > 0.0 {
                count as f64 / elapsed_mins
            } else {
                0.0
            };

            let idle_duration = match reporter_last_event.lock() {
                Ok(last) => last.elapsed(),
                Err(poisoned) => poisoned.into_inner().elapsed(),
            };

            let cadence = KeystrokeCadence {
                events_per_minute,
                burst_detected: events_per_minute > 200.0,
                idle_duration,
                timestamp: Instant::now(),
            };

            if reporter_tx.blocking_send(cadence).is_err() {
                break;
            }
        }
    });

    std::thread::spawn(move || {
        use rdev::{listen, EventType};

        let listener_count = Arc::clone(&event_count);
        let listener_last_event = Arc::clone(&last_event_time);

        let result = listen(move |event| {
            if matches!(
                event.event_type,
                EventType::KeyPress(_) | EventType::KeyRelease(_)
            ) {
                listener_count.fetch_add(1, Ordering::Relaxed);
                match listener_last_event.lock() {
                    Ok(mut last) => {
                        *last = Instant::now();
                    }
                    Err(poisoned) => {
                        let mut recovered = poisoned.into_inner();
                        *recovered = Instant::now();
                    }
                }
            }
        });

        listener_alive.store(false, Ordering::Relaxed);

        if let Err(err) = result {
            eprintln!(
                "[platform/{platform_label}] rdev listen failed; keystroke cadence disabled. Ensure global input permissions are granted: {err:?}"
            );
        }
    });
}

/// Map a `notify::EventKind` to our typed `FileEventKind`.
fn map_notify_kind(kind: &notify::EventKind) -> Option<bus::events::platform::FileEventKind> {
    use bus::events::platform::FileEventKind;
    use notify::EventKind;
    match kind {
        EventKind::Create(_) => Some(FileEventKind::Created),
        EventKind::Modify(_) => Some(FileEventKind::Modified),
        EventKind::Remove(_) => Some(FileEventKind::Deleted),
        EventKind::Access(_) => None, // access events are noise — skip
        _ => Some(FileEventKind::Modified),
    }
}

/// Spawn a cross-platform file watcher that forwards events to `tx`.
///
/// Uses the `notify` crate's `RecommendedWatcher` which selects the best backend
/// for the current OS (inotify on Linux, FSEvents on macOS, ReadDirectoryChangesW on
/// Windows).  Runs on a dedicated OS thread to avoid blocking the async runtime.
///
/// No-op when `paths` is empty.
pub(crate) fn spawn_file_event_watcher(tx: mpsc::Sender<FileEvent>, paths: Vec<PathBuf>) {
    if paths.is_empty() {
        return;
    }

    std::thread::spawn(move || {
        use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
        use std::sync::mpsc as std_mpsc;
        use std::time::Instant;

        let (ntx, nrx) = std_mpsc::channel::<notify::Result<notify::Event>>();
        let mut watcher = match RecommendedWatcher::new(ntx, Config::default()) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("[platform] file watcher creation failed: {e}");
                return;
            }
        };

        for path in &paths {
            if let Err(e) = watcher.watch(path.as_ref(), RecursiveMode::Recursive) {
                eprintln!(
                    "[platform] file watcher: failed to watch {}: {e}",
                    path.display()
                );
            }
        }

        loop {
            match nrx.recv() {
                Ok(Ok(event)) => {
                    let Some(file_kind) = map_notify_kind(&event.kind) else {
                        continue;
                    };
                    for path in event.paths {
                        let fe = FileEvent {
                            path,
                            event_kind: file_kind.clone(),
                            timestamp: Instant::now(),
                        };
                        if tx.blocking_send(fe).is_err() {
                            return; // receiver dropped — shut down
                        }
                    }
                }
                Ok(Err(e)) => {
                    eprintln!("[platform] file watcher error: {e}");
                }
                Err(_) => break, // std channel disconnected
            }
        }
        // watcher dropped here, unregistering all watches
    });
}
