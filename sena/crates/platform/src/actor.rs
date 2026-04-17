//! Platform actor — owns the platform backend and manages signal monitoring.

use crate::adapter::PlatformAdapter;
use crate::backend::PlatformBackend;
use crate::backends::NativeBackend;
use crate::error::PlatformError;
use crate::monitor::VisionFrameCache;
use crate::types::{ClipboardDigest, FileEvent, KeystrokeCadence, PlatformSignal, WindowContext};
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::info;

/// Platform actor — holds a native backend and manages signal broadcasting.
pub struct PlatformActor {
    backend: Box<dyn PlatformBackend>,
    window_tx: broadcast::Sender<WindowContext>,
    clipboard_tx: broadcast::Sender<ClipboardDigest>,
    keystroke_tx: broadcast::Sender<KeystrokeCadence>,
    file_event_tx: broadcast::Sender<FileEvent>,
    vision_cache: Arc<VisionFrameCache>,
}

impl PlatformActor {
    /// Create a new platform actor with an injected backend.
    pub fn new(backend: Box<dyn PlatformBackend>) -> Self {
        info!("PlatformActor initialized with injected backend");

        let (window_tx, _) = broadcast::channel(32);
        let (clipboard_tx, _) = broadcast::channel(32);
        let (keystroke_tx, _) = broadcast::channel(32);
        let (file_event_tx, _) = broadcast::channel(128);
        let vision_cache = Arc::new(VisionFrameCache::new());

        Self {
            backend,
            window_tx,
            clipboard_tx,
            keystroke_tx,
            file_event_tx,
            vision_cache,
        }
    }

    /// Create a new platform actor with the native backend for this OS.
    pub fn native() -> Result<Self, PlatformError> {
        info!("PlatformActor initializing with native backend");
        let backend = Box::new(NativeBackend::new()?);
        Ok(Self::new(backend))
    }

    /// Create a platform actor with a custom backend (for testing).
    pub fn with_backend(backend: Box<dyn PlatformBackend>) -> Self {
        Self::new(backend)
    }

    /// Poll and broadcast active window changes.
    pub fn poll_active_window(&self) -> Result<(), PlatformError> {
        match self.backend.active_window()? {
            PlatformSignal::Window(ctx) => {
                let _ = self.window_tx.send(ctx);
                Ok(())
            }
            _ => Err(PlatformError::WindowContextFailed(
                "unexpected signal type".to_string(),
            )),
        }
    }

    /// Poll and broadcast clipboard changes.
    pub fn poll_clipboard(&self) -> Result<(), PlatformError> {
        match self.backend.clipboard_content() {
            Ok(PlatformSignal::Clipboard(digest)) => {
                let _ = self.clipboard_tx.send(digest);
                Ok(())
            }
            Err(PlatformError::ClipboardFailed(msg)) if msg.contains("no change") => {
                // Debounced or no change — not an error
                Ok(())
            }
            Err(e) => Err(e),
            _ => Err(PlatformError::ClipboardFailed(
                "unexpected signal type".to_string(),
            )),
        }
    }

    /// Poll and broadcast keystroke cadence.
    pub fn poll_keystroke_cadence(&self) -> Result<(), PlatformError> {
        match self.backend.keystroke_cadence() {
            Ok(PlatformSignal::Keystroke(cadence)) => {
                let _ = self.keystroke_tx.send(cadence);
                Ok(())
            }
            Err(PlatformError::KeystrokeCadenceFailed(msg)) if msg.contains("no keystroke") => {
                // No data yet — not an error
                Ok(())
            }
            Err(e) => Err(e),
            _ => Err(PlatformError::KeystrokeCadenceFailed(
                "unexpected signal type".to_string(),
            )),
        }
    }

    /// Capture and cache a screen frame.
    pub fn capture_screen_frame(&self) -> Result<(), PlatformError> {
        match self.backend.screen_frame()? {
            PlatformSignal::ScreenFrame(frame) => {
                self.vision_cache.add_frame(frame.rgb_data);
                Ok(())
            }
            _ => Err(PlatformError::ScreenCaptureFailed(
                "unexpected signal type".to_string(),
            )),
        }
    }
}

impl PlatformAdapter for PlatformActor {
    fn subscribe_active_window(&self) -> broadcast::Receiver<WindowContext> {
        self.window_tx.subscribe()
    }

    fn subscribe_clipboard(&self) -> broadcast::Receiver<ClipboardDigest> {
        self.clipboard_tx.subscribe()
    }

    fn subscribe_keystrokes(&self) -> broadcast::Receiver<KeystrokeCadence> {
        self.keystroke_tx.subscribe()
    }

    fn subscribe_file_events(&self) -> broadcast::Receiver<FileEvent> {
        self.file_event_tx.subscribe()
    }

    fn latest_vision_frame(&self) -> Option<Arc<Vec<u8>>> {
        self.vision_cache.latest_frame().map(Arc::new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ClipboardDigest, KeystrokeCadence, ScreenFrame};
    use std::time::{Duration, Instant};

    struct StubBackend;

    impl PlatformBackend for StubBackend {
        fn active_window(&self) -> Result<PlatformSignal, PlatformError> {
            Ok(PlatformSignal::Window(WindowContext {
                app_name: "StubApp".to_string(),
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

    #[test]
    fn actor_constructs_with_backend() {
        let backend = Box::new(StubBackend);
        let actor = PlatformActor::with_backend(backend);
        // Actor construction should succeed
        assert!(actor.poll_active_window().is_ok());
    }

    #[test]
    fn actor_implements_adapter_trait() {
        let backend = Box::new(StubBackend);
        let actor = PlatformActor::with_backend(backend);

        let _window_rx = actor.subscribe_active_window();
        let _clipboard_rx = actor.subscribe_clipboard();
        let _keystroke_rx = actor.subscribe_keystrokes();
        let _file_rx = actor.subscribe_file_events();
        let _frame = actor.latest_vision_frame();
    }

    #[test]
    fn actor_broadcasts_signals() {
        let backend = Box::new(StubBackend);
        let actor = PlatformActor::with_backend(backend);

        let mut window_rx = actor.subscribe_active_window();

        // Poll and broadcast
        assert!(actor.poll_active_window().is_ok());

        // Verify broadcast
        let received = window_rx.try_recv();
        assert!(received.is_ok());
    }

    #[test]
    fn actor_caches_vision_frames() {
        let backend = Box::new(StubBackend);
        let actor = PlatformActor::with_backend(backend);

        // Capture a frame
        actor.capture_screen_frame().unwrap();

        // Retrieve latest frame
        let frame = actor.latest_vision_frame();
        assert!(frame.is_some());
    }
}
