//! Platform actor — owns the platform backend and processes signal requests.

use crate::backend::PlatformBackend;
use crate::error::PlatformError;
use crate::types::PlatformSignal;
use tracing::{debug, info};

/// Platform actor — holds a boxed PlatformBackend and processes signal requests.
pub struct PlatformActor {
    backend: Box<dyn PlatformBackend>,
}

impl PlatformActor {
    /// Create a new platform actor with the given backend.
    pub fn new(backend: Box<dyn PlatformBackend>) -> Self {
        info!("PlatformActor initialized");
        Self { backend }
    }

    /// Request the current active window context.
    pub fn get_active_window(&self) -> Result<PlatformSignal, PlatformError> {
        debug!("PlatformActor: get_active_window requested");
        self.backend.active_window()
    }

    /// Request the current clipboard content digest.
    pub fn get_clipboard_content(&self) -> Result<PlatformSignal, PlatformError> {
        debug!("PlatformActor: get_clipboard_content requested");
        self.backend.clipboard_content()
    }

    /// Request the current keystroke cadence pattern.
    pub fn get_keystroke_cadence(&self) -> Result<PlatformSignal, PlatformError> {
        debug!("PlatformActor: get_keystroke_cadence requested");
        self.backend.keystroke_cadence()
    }

    /// Request a screen frame capture.
    pub fn get_screen_frame(&self) -> Result<PlatformSignal, PlatformError> {
        debug!("PlatformActor: get_screen_frame requested");
        self.backend.screen_frame()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ClipboardDigest, KeystrokeCadence, ScreenFrame, WindowContext};
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
        let actor = PlatformActor::new(backend);
        // Actor construction should succeed
        assert!(actor.get_active_window().is_ok());
    }

    #[test]
    fn actor_delegates_to_backend() {
        let backend = Box::new(StubBackend);
        let actor = PlatformActor::new(backend);

        let window = actor.get_active_window();
        assert!(window.is_ok());

        let clipboard = actor.get_clipboard_content();
        assert!(clipboard.is_ok());

        let keystroke = actor.get_keystroke_cadence();
        assert!(keystroke.is_ok());

        let screen = actor.get_screen_frame();
        assert!(screen.is_ok());
    }

    #[test]
    fn actor_returns_correct_signal_types() {
        let backend = Box::new(StubBackend);
        let actor = PlatformActor::new(backend);

        match actor.get_active_window() {
            Ok(PlatformSignal::Window(w)) => assert_eq!(w.app_name, "StubApp"),
            _ => panic!("expected Window signal"),
        }

        match actor.get_clipboard_content() {
            Ok(PlatformSignal::Clipboard(c)) => assert_eq!(c.char_count, 0),
            _ => panic!("expected Clipboard signal"),
        }

        match actor.get_keystroke_cadence() {
            Ok(PlatformSignal::Keystroke(k)) => assert_eq!(k.events_per_minute, 0.0),
            _ => panic!("expected Keystroke signal"),
        }

        match actor.get_screen_frame() {
            Ok(PlatformSignal::ScreenFrame(f)) => assert_eq!(f.width, 1),
            _ => panic!("expected ScreenFrame signal"),
        }
    }
}
