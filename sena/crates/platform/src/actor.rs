//! Platform actor — owns the platform backend and manages signal monitoring.

use crate::adapter::PlatformAdapter;
use crate::backend::PlatformBackend;
use crate::backends::NativeBackend;
use crate::error::PlatformError;
use crate::monitor::VisionFrameCache;
use crate::types::{ClipboardDigest, FileEvent, KeystrokeCadence, PlatformSignal, WindowContext};
use bus::events::platform::PlatformEvent;
use bus::{Event, EventBus};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

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

    /// Run the platform polling loop until a shutdown signal is received.
    ///
    /// Polls all backend signals at their configured intervals and broadcasts
    /// results on both the actor's internal channels AND the shared EventBus.
    /// This is the production entry point called by the runtime boot sequence.
    pub async fn run_polling_loop(
        &self,
        bus: Arc<EventBus>,
        window_interval: Duration,
        clipboard_interval: Duration,
        keystroke_interval: Duration,
        clipboard_enabled: bool,
    ) {
        use bus::events::system::SystemEvent;

        let mut shutdown_rx = bus.subscribe_broadcast();
        let mut window_tick = tokio::time::interval(window_interval);
        let mut clipboard_tick = tokio::time::interval(clipboard_interval);
        let mut keystroke_tick = tokio::time::interval(keystroke_interval);
        let mut screen_capture_tick = tokio::time::interval(Duration::from_secs(30));

        // Track last-seen window to deduplicate bus broadcasts.
        let mut last_app: Option<String> = None;
        let mut last_clipboard_signature: Option<(Option<String>, usize)> = None;

        // Loop enabled states (controlled by IPC)
        let mut platform_polling_enabled = true;
        let mut screen_capture_enabled = true;

        // Broadcast initial screen_capture loop status
        let _ = bus
            .broadcast(Event::System(SystemEvent::LoopStatusChanged {
                loop_name: "screen_capture".to_string(),
                enabled: true,
            }))
            .await;

        loop {
            tokio::select! {
                result = shutdown_rx.recv() => {
                    match result {
                        Ok(Event::System(SystemEvent::ShutdownSignal))
                        | Ok(Event::System(SystemEvent::ShutdownRequested))
                        | Ok(Event::System(SystemEvent::ShutdownInitiated)) => {
                            info!("PlatformActor: shutdown signal received");
                            break;
                        }
                        Ok(Event::System(SystemEvent::LoopControlRequested {
                            loop_name,
                            enabled,
                        })) if loop_name == "platform_polling" => {
                            info!(
                                enabled = enabled,
                                "PlatformActor: platform_polling loop control requested"
                            );
                            platform_polling_enabled = enabled;

                            // Broadcast status changed event
                            let _ = bus
                                .broadcast(Event::System(SystemEvent::LoopStatusChanged {
                                    loop_name: "platform_polling".to_string(),
                                    enabled,
                                }))
                                .await;
                        }
                        Ok(Event::System(SystemEvent::LoopControlRequested {
                            loop_name,
                            enabled,
                        })) if loop_name == "screen_capture" => {
                            info!(
                                enabled = enabled,
                                "PlatformActor: screen_capture loop control requested"
                            );
                            screen_capture_enabled = enabled;

                            // Broadcast status changed event
                            let _ = bus
                                .broadcast(Event::System(SystemEvent::LoopStatusChanged {
                                    loop_name: "screen_capture".to_string(),
                                    enabled,
                                }))
                                .await;
                        }
                        Err(_) => break,
                        Ok(_) => {}
                    }
                }

                _ = window_tick.tick() => {
                    if !platform_polling_enabled {
                        continue;
                    }
                    match self.backend.active_window() {
                        Ok(PlatformSignal::Window(ctx)) => {
                            // Only broadcast on change
                            let changed = last_app.as_deref() != Some(ctx.app_name.as_str());
                            if changed {
                                debug!(app = %ctx.app_name, "PlatformActor: window changed");
                                last_app = Some(ctx.app_name.clone());
                                let _ = self.window_tx.send(ctx.clone());
                                let _ = bus.broadcast(
                                    Event::Platform(PlatformEvent::ActiveWindowChanged(ctx))
                                ).await;
                            }
                        }
                        Err(e) => debug!(error = %e, "platform: window poll error"),
                        _ => {}
                    }
                }

                _ = clipboard_tick.tick() => {
                    if !platform_polling_enabled || !clipboard_enabled {
                        continue;
                    }
                    match self.backend.clipboard_content() {
                        Ok(PlatformSignal::Clipboard(digest)) => {
                            let signature = (digest.digest.clone(), digest.char_count);
                            let changed = last_clipboard_signature.as_ref() != Some(&signature);
                            if changed {
                                debug!(chars = digest.char_count, "PlatformActor: clipboard changed");
                                last_clipboard_signature = Some(signature);
                                let _ = self.clipboard_tx.send(digest.clone());
                                let _ = bus.broadcast(
                                    Event::Platform(PlatformEvent::ClipboardChanged(digest))
                                ).await;
                            }
                        }
                        Err(PlatformError::ClipboardFailed(msg)) if msg.contains("no change") => {}
                        Err(e) => debug!(error = %e, "platform: clipboard poll error"),
                        _ => {}
                    }
                }

                _ = keystroke_tick.tick() => {
                    if !platform_polling_enabled {
                        continue;
                    }
                    match self.backend.keystroke_cadence() {
                        Ok(PlatformSignal::Keystroke(cadence)) => {
                            let _ = self.keystroke_tx.send(cadence.clone());
                            let _ = bus.broadcast(
                                Event::Platform(PlatformEvent::KeystrokeCadenceUpdated(cadence))
                            ).await;
                        }
                        Err(PlatformError::KeystrokeCadenceFailed(msg)) if msg.contains("no keystroke") => {}
                        Err(e) => debug!(error = %e, "platform: keystroke poll error"),
                        _ => {}
                    }
                }

                _ = screen_capture_tick.tick() => {
                    if !screen_capture_enabled {
                        continue;
                    }
                    // Capture screen frame and cache it
                    match self.backend.screen_frame() {
                        Ok(PlatformSignal::ScreenFrame(frame)) => {
                            debug!(
                                width = frame.width,
                                height = frame.height,
                                "PlatformActor: screen captured"
                            );
                            self.vision_cache.add_frame(frame.rgb_data.clone());

                            // Broadcast vision frame available event
                            // Note: frame_data should be encoded (e.g., PNG), but for now we pass raw RGB
                            let _ = bus.broadcast(
                                Event::Platform(PlatformEvent::VisionFrameAvailable {
                                    frame_data: frame.rgb_data,
                                    screen_id: 0,
                                    timestamp: frame.timestamp,
                                })
                            ).await;
                        }
                        Err(e) => debug!(error = %e, "platform: screen capture error"),
                        _ => {}
                    }
                }
            }
        }

        warn!("PlatformActor polling loop exited");
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
