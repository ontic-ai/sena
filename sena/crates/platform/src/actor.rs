//! Platform actor — owns the platform backend and manages signal monitoring.

use crate::adapter::PlatformAdapter;
use crate::backend::PlatformBackend;
use crate::backends::NativeBackend;
use crate::error::PlatformError;
use crate::monitor::VisionFrameCache;
use crate::types::{
    ClipboardDigest, FileEvent, FileEventKind, KeystrokeCadence, PlatformSignal,
    WindowContext,
};
use bus::events::platform::PlatformEvent;
use bus::{Event, EventBus};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc};
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
        file_watch_paths: &[PathBuf],
    ) {
        use bus::events::system::SystemEvent;

        let mut shutdown_rx = bus.subscribe_broadcast();
        let mut window_tick = tokio::time::interval(window_interval);
        let mut clipboard_tick = tokio::time::interval(clipboard_interval);
        let mut keystroke_tick = tokio::time::interval(keystroke_interval);
        let mut screen_capture_tick = tokio::time::interval(Duration::from_secs(30));
        let (file_watcher, mut file_event_rx) = match Self::create_file_watcher(file_watch_paths) {
            Ok(state) => state,
            Err(error) => {
                warn!(error = %error, "PlatformActor: file watcher setup failed");
                (None, None)
            }
        };
        let file_watching_enabled = file_event_rx.is_some();
        let _file_watcher = file_watcher;

        // Track last-seen window to deduplicate bus broadcasts.
        let mut last_window_signature: Option<(String, Option<String>)> = None;
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
                            let signature = (ctx.app_name.clone(), ctx.window_title.clone());
                            let changed = last_window_signature.as_ref() != Some(&signature);
                            if changed {
                                debug!(app = %ctx.app_name, "PlatformActor: window changed");
                                last_window_signature = Some(signature);
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

                file_watch_result = async {
                    match &mut file_event_rx {
                        Some(rx) => rx.recv().await,
                        None => None,
                    }
                }, if file_watching_enabled => {
                    match file_watch_result {
                        Some(Ok(event)) => {
                            if let Some(event_kind) = map_notify_event_kind(&event.kind) {
                                for path in event.paths {
                                    let file_event = FileEvent {
                                        path,
                                        event_kind: event_kind.clone(),
                                        timestamp: Instant::now(),
                                    };
                                    debug!(
                                        path = %file_event.path.display(),
                                        kind = ?file_event.event_kind,
                                        "PlatformActor: file changed"
                                    );
                                    let _ = self.file_event_tx.send(file_event.clone());
                                    let _ = bus.broadcast(
                                        Event::Platform(PlatformEvent::FileEvent(file_event))
                                    ).await;
                                }
                            }
                        }
                        Some(Err(error)) => debug!(error = %error, "platform: file watch error"),
                        None => {}
                    }
                }
            }
        }

        warn!("PlatformActor polling loop exited");
    }

    fn create_file_watcher(
        watch_paths: &[PathBuf],
    ) -> Result<
        (
            Option<RecommendedWatcher>,
            Option<mpsc::UnboundedReceiver<notify::Result<notify::Event>>>,
        ),
        PlatformError,
    > {
        if watch_paths.is_empty() {
            return Ok((None, None));
        }

        let (tx, rx) = mpsc::unbounded_channel();
        let mut watcher = notify::recommended_watcher(move |result| {
            let _ = tx.send(result);
        })
        .map_err(|error| PlatformError::OsError(format!("failed to create file watcher: {}", error)))?;

        let mut watched_count = 0usize;
        for path in watch_paths {
            if !path.exists() {
                warn!(path = %path.display(), "PlatformActor: skipping missing watch path");
                continue;
            }

            watcher
                .watch(path, watch_mode(path))
                .map_err(|error| {
                    PlatformError::OsError(format!("failed to watch {}: {}", path.display(), error))
                })?;
            watched_count += 1;
        }

        if watched_count == 0 {
            warn!("PlatformActor: no valid file watch paths configured");
            return Ok((None, None));
        }

        Ok((Some(watcher), Some(rx)))
    }
}

fn watch_mode(path: &Path) -> RecursiveMode {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => RecursiveMode::Recursive,
        _ => RecursiveMode::NonRecursive,
    }
}

fn map_notify_event_kind(kind: &notify::EventKind) -> Option<FileEventKind> {
    match kind {
        notify::EventKind::Create(_) => Some(FileEventKind::Created),
        notify::EventKind::Modify(notify::event::ModifyKind::Name(_)) => {
            Some(FileEventKind::Renamed)
        }
        notify::EventKind::Modify(_) => Some(FileEventKind::Modified),
        notify::EventKind::Remove(_) => Some(FileEventKind::Deleted),
        _ => None,
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
    use bus::events::system::SystemEvent;
    use bus::{Event, EventBus};
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    use tempfile::tempdir;
    use tokio::time::timeout;

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

    struct WindowSequenceBackend {
        windows: Mutex<VecDeque<WindowContext>>,
    }

    impl WindowSequenceBackend {
        fn new(windows: Vec<WindowContext>) -> Self {
            Self {
                windows: Mutex::new(VecDeque::from(windows)),
            }
        }
    }

    impl PlatformBackend for WindowSequenceBackend {
        fn active_window(&self) -> Result<PlatformSignal, PlatformError> {
            let mut windows = self.windows.lock().expect("window sequence lock poisoned");
            let ctx = if windows.len() > 1 {
                windows.pop_front().expect("window sequence should not be empty")
            } else {
                windows
                    .front()
                    .cloned()
                    .expect("window sequence should not be empty")
            };

            Ok(PlatformSignal::Window(ctx))
        }

        fn clipboard_content(&self) -> Result<PlatformSignal, PlatformError> {
            Err(PlatformError::ClipboardFailed("no change".to_string()))
        }

        fn keystroke_cadence(&self) -> Result<PlatformSignal, PlatformError> {
            Err(PlatformError::KeystrokeCadenceFailed(
                "no keystroke".to_string(),
            ))
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

    #[tokio::test]
    async fn run_polling_loop_emits_window_events_for_title_changes() {
        let backend = Box::new(WindowSequenceBackend::new(vec![
            WindowContext {
                app_name: "Code".to_string(),
                window_title: Some("first.rs".to_string()),
                bundle_id: None,
                timestamp: Instant::now(),
            },
            WindowContext {
                app_name: "Code".to_string(),
                window_title: Some("second.rs".to_string()),
                bundle_id: None,
                timestamp: Instant::now(),
            },
        ]));
        let actor = PlatformActor::with_backend(backend);
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();
        let task_bus = bus.clone();

        let handle = tokio::spawn(async move {
            actor
                .run_polling_loop(
                    task_bus,
                    Duration::from_millis(20),
                    Duration::from_secs(60),
                    Duration::from_secs(60),
                    false,
                    &[],
                )
                .await;
        });

        let mut titles = Vec::new();
        while titles.len() < 2 {
            let event = timeout(Duration::from_secs(1), rx.recv())
                .await
                .expect("window event should arrive")
                .expect("broadcast receive should succeed");
            if let Event::Platform(PlatformEvent::ActiveWindowChanged(ctx)) = event {
                titles.push(ctx.window_title);
            }
        }

        assert_eq!(titles[0].as_deref(), Some("first.rs"));
        assert_eq!(titles[1].as_deref(), Some("second.rs"));

        bus.broadcast(Event::System(SystemEvent::ShutdownRequested))
            .await
            .expect("shutdown broadcast should succeed");
        timeout(Duration::from_secs(1), handle)
            .await
            .expect("platform task should stop")
            .expect("platform task should exit cleanly");
    }

    #[tokio::test]
    async fn run_polling_loop_emits_file_events_for_watched_paths() {
        let temp_dir = tempdir().expect("create tempdir");
        let watched_dir = temp_dir.path().to_path_buf();
        let changed_path = watched_dir.join("watched.txt");
        let actor = PlatformActor::with_backend(Box::new(StubBackend));
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();
        let task_bus = bus.clone();
        let watch_paths = vec![watched_dir.clone()];

        let handle = tokio::spawn(async move {
            actor
                .run_polling_loop(
                    task_bus,
                    Duration::from_secs(60),
                    Duration::from_secs(60),
                    Duration::from_secs(60),
                    false,
                    &watch_paths,
                )
                .await;
        });

        tokio::time::sleep(Duration::from_millis(150)).await;
        tokio::fs::write(&changed_path, b"hello")
            .await
            .expect("write watched file");

        let file_event = timeout(Duration::from_secs(5), async {
            loop {
                let event = rx.recv().await.expect("broadcast receive should succeed");
                if let Event::Platform(PlatformEvent::FileEvent(file_event)) = event
                    && file_event.path == changed_path
                    && matches!(
                        file_event.event_kind,
                        FileEventKind::Created | FileEventKind::Modified
                    )
                {
                    break file_event;
                }
            }
        })
        .await
        .expect("file event should arrive");

        assert_eq!(file_event.path, changed_path);

        bus.broadcast(Event::System(SystemEvent::ShutdownRequested))
            .await
            .expect("shutdown broadcast should succeed");
        timeout(Duration::from_secs(1), handle)
            .await
            .expect("platform task should stop")
            .expect("platform task should exit cleanly");
    }
}
