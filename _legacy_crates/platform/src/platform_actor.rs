//! Platform actor: polls OS signals and emits events on the bus.

use async_trait::async_trait;
use bus::events::platform::{ClipboardDigest, FileEvent, KeystrokeCadence, WindowContext};
use bus::events::platform_vision::{CaptureReason, ImageDigest, ScreenCaptureEvent};
use bus::{
    Actor, ActorError, Event, EventBus, PlatformEvent,
    PlatformVisionEvent as BusPlatformVisionEvent, SystemEvent,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use sysinfo::System;
use tokio::sync::{broadcast, mpsc};

use crate::adapter::PlatformAdapter;

/// Platform actor polls the platform adapter and emits events on the bus.
pub struct PlatformActor {
    adapter: Box<dyn PlatformAdapter>,
    bus_rx: Option<broadcast::Receiver<Event>>,
    bus: Option<Arc<EventBus>>,
    poll_interval: Duration,
    last_window: Option<WindowContext>,
    last_clipboard: Option<ClipboardDigest>,
    clipboard_enabled: bool,
    system_info: System,
    idle_threshold: f32,
    normal_poll_interval: Duration,
    /// First CPU reading is always inaccurate — skip threshold logic on first tick
    first_tick: bool,
    keystroke_rx: Option<tokio::sync::mpsc::Receiver<KeystrokeCadence>>,
    file_rx: Option<mpsc::Receiver<FileEvent>>,
    file_events_enabled: bool,
    file_watch_paths: Vec<PathBuf>,
    /// Whether screen capture is enabled (from config).
    screen_capture_enabled: bool,
    /// Whether the platform polling loop is enabled (pause/resume via LoopControlRequested).
    platform_polling_enabled: bool,
    /// Whether the screen capture loop specifically is enabled (in addition to screen_capture_enabled).
    screen_capture_loop_enabled: bool,
    /// Shared storage for the latest in-memory PNG frame.
    /// This is NEVER broadcast on the bus — inference actor pulls from it directly.
    latest_jpeg_frame: Arc<Mutex<Option<Vec<u8>>>>,
    /// Track last screenshot digest for change detection.
    last_screenshot_digest: Option<[u8; 32]>,
    /// Keystroke events-per-minute threshold for burst detection.
    burst_threshold_epm: f64,
    /// Path fragments whose presence in a file event path causes the event to be dropped.
    file_watch_exclude_patterns: Vec<String>,
    /// Tracks when the latest vision frame was written.
    last_capture_time: Option<Instant>,
    /// Maximum age a vision frame is kept before being cleared.
    vision_frame_max_age: Duration,
}

impl PlatformActor {
    /// Create a new platform actor with the given adapter.
    pub fn new(adapter: Box<dyn PlatformAdapter>) -> Self {
        let default_poll_interval = Duration::from_millis(500);
        Self {
            adapter,
            bus_rx: None,
            bus: None,
            poll_interval: default_poll_interval,
            last_window: None,
            last_clipboard: None,
            clipboard_enabled: true,
            system_info: System::new_all(),
            idle_threshold: 10.0,
            normal_poll_interval: default_poll_interval,
            first_tick: true,
            keystroke_rx: None,
            file_rx: None,
            file_events_enabled: false,
            file_watch_paths: Vec::new(),
            screen_capture_enabled: false,
            latest_jpeg_frame: Arc::new(Mutex::new(None)),
            last_screenshot_digest: None,
            burst_threshold_epm: 200.0,
            file_watch_exclude_patterns: Vec::new(),
            last_capture_time: None,
            vision_frame_max_age: Duration::from_secs(30),
            platform_polling_enabled: true,
            screen_capture_loop_enabled: true,
        }
    }

    /// Set the polling interval for checking platform signals.
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self.normal_poll_interval = interval;
        self
    }

    /// Set the CPU idle threshold percentage for dynamic polling.
    /// When CPU usage falls below this value, poll interval increases to 2 seconds.
    pub fn with_idle_threshold(mut self, threshold: f32) -> Self {
        self.idle_threshold = threshold;
        self
    }

    /// Enable or disable clipboard observation.
    ///
    /// When disabled, the platform actor will not poll or emit clipboard events.
    /// This respects user privacy preferences from the config.
    pub fn with_clipboard_enabled(mut self, enabled: bool) -> Self {
        self.clipboard_enabled = enabled;
        self
    }

    /// Enable file event observation when watch paths are configured.
    pub fn with_file_watch_paths(mut self, paths: Vec<PathBuf>) -> Self {
        self.file_events_enabled = !paths.is_empty();
        self.file_watch_paths = paths;
        self
    }

    /// Enable or disable periodic screen capture polling.
    pub fn with_screen_capture_enabled(mut self, enabled: bool) -> Self {
        self.screen_capture_enabled = enabled;
        self
    }

    /// Set the keystroke events-per-minute rate above which burst is flagged.
    pub fn with_burst_threshold_epm(mut self, threshold: f64) -> Self {
        self.burst_threshold_epm = threshold;
        self
    }

    /// Set path fragment patterns that cause file events to be dropped.
    pub fn with_file_watch_exclude_patterns(mut self, patterns: Vec<String>) -> Self {
        self.file_watch_exclude_patterns = patterns;
        self
    }

    /// Set the maximum age of a cached vision frame before it is cleared.
    pub fn with_vision_frame_max_age(mut self, max_age: Duration) -> Self {
        self.vision_frame_max_age = max_age;
        self
    }

    /// Return the shared in-memory frame store used for latest screen PNG bytes.
    pub fn latest_frame_store(&self) -> Arc<Mutex<Option<Vec<u8>>>> {
        Arc::clone(&self.latest_jpeg_frame)
    }

    /// Get the current poll interval (test-only).
    #[cfg(test)]
    pub(crate) fn current_poll_interval(&self) -> Duration {
        self.poll_interval
    }

    /// Get the configured idle threshold (test-only).
    #[cfg(test)]
    pub(crate) fn current_idle_threshold(&self) -> f32 {
        self.idle_threshold
    }

    /// Get the normal poll interval (test-only).
    #[cfg(test)]
    pub(crate) fn current_normal_poll_interval(&self) -> Duration {
        self.normal_poll_interval
    }

    /// Check for window changes and emit event if changed.
    async fn check_window_change(&mut self) -> Result<(), ActorError> {
        if let Some(current) = self.adapter.active_window() {
            let should_emit = self
                .last_window
                .as_ref()
                .map(|last| last.app_name != current.app_name)
                .unwrap_or(true);

            if should_emit {
                if let Some(bus) = &self.bus {
                    bus.broadcast(Event::Platform(PlatformEvent::WindowChanged(
                        current.clone(),
                    )))
                    .await
                    .map_err(|e| ActorError::RuntimeError(format!("broadcast failed: {}", e)))?;
                }
                self.last_window = Some(current);
            }
        }
        Ok(())
    }

    /// Check for clipboard changes and emit event if changed.
    /// No-op if clipboard observation is disabled via config.
    async fn check_clipboard_change(&mut self) -> Result<(), ActorError> {
        if !self.clipboard_enabled {
            return Ok(());
        }
        if let Some(current) = self.adapter.clipboard_digest() {
            let should_emit = self
                .last_clipboard
                .as_ref()
                .map(|last| last.digest != current.digest)
                .unwrap_or(true);

            if should_emit {
                if let Some(bus) = &self.bus {
                    bus.broadcast(Event::Platform(PlatformEvent::ClipboardChanged(
                        current.clone(),
                    )))
                    .await
                    .map_err(|e| ActorError::RuntimeError(format!("broadcast failed: {}", e)))?;
                }
                self.last_clipboard = Some(current);
            }
        }
        Ok(())
    }

    async fn poll_screen_capture(&mut self, bus: &Arc<EventBus>) {
        match self.adapter.screen_capture() {
            Ok(digest) => {
                let digest: ImageDigest = digest;
                let digest_bytes = *digest.as_bytes();
                if self.last_screenshot_digest.as_ref() != Some(&digest_bytes) {
                    self.last_screenshot_digest = Some(digest_bytes);

                    match self.adapter.screen_capture_png(512) {
                        Ok(png_bytes) => {
                            if let Ok(mut frame) = self.latest_jpeg_frame.lock() {
                                *frame = Some(png_bytes);
                            }
                            self.last_capture_time = Some(Instant::now());
                        }
                        Err(_) => {
                            // Non-fatal: PNG capture unavailable on this platform.
                            // Clear stale frame so inference does not reuse old visual context.
                            if let Ok(mut frame) = self.latest_jpeg_frame.lock() {
                                *frame = None;
                            }
                        }
                    }

                    let event = BusPlatformVisionEvent::ScreenCaptureEvent(ScreenCaptureEvent {
                        timestamp: SystemTime::now(),
                        image_digest: digest,
                        resolution: (0, 0),
                        capture_reason: CaptureReason::ContextSwitch,
                    });

                    let _ = bus.broadcast(Event::PlatformVision(event)).await;
                }
            }
            Err(_) => {
                // Non-fatal: screen capture unavailable on this platform.
            }
        }
    }
}

#[async_trait]
impl Actor for PlatformActor {
    fn name(&self) -> &'static str {
        "platform"
    }

    async fn start(&mut self, bus: Arc<EventBus>) -> Result<(), ActorError> {
        self.bus_rx = Some(bus.subscribe_broadcast());
        self.bus = Some(bus.clone());

        // Start keystroke pattern subscription
        let (keystroke_tx, keystroke_rx) = tokio::sync::mpsc::channel(32);
        self.adapter.subscribe_keystroke_patterns(keystroke_tx);
        self.keystroke_rx = Some(keystroke_rx);

        if self.file_events_enabled {
            let (file_tx, file_rx) = mpsc::channel(64);
            self.adapter
                .subscribe_file_events(file_tx, &self.file_watch_paths);
            self.file_rx = Some(file_rx);
        }

        bus.broadcast(Event::System(SystemEvent::ActorReady {
            actor_name: "Platform",
        }))
        .await
        .map_err(|e| ActorError::StartupFailed(format!("broadcast ActorReady failed: {}", e)))?;

        tracing::info!("Platform actor ready");
        Ok(())
    }

    async fn run(&mut self) -> Result<(), ActorError> {
        let mut interval = tokio::time::interval(self.poll_interval);

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    // Skip all platform polling if disabled
                    if !self.platform_polling_enabled {
                        continue;
                    }

                    // Dynamic polling based on CPU usage
                    self.system_info.refresh_cpu_all();

                    if !self.first_tick {
                        let cpu_usage = self.system_info.global_cpu_usage();
                        let new_interval = if cpu_usage < self.idle_threshold {
                            Duration::from_secs(2)
                        } else {
                            self.normal_poll_interval
                        };

                        if new_interval != self.poll_interval {
                            self.poll_interval = new_interval;
                            interval = tokio::time::interval(new_interval);
                        }
                    } else {
                        self.first_tick = false;
                    }

                    self.check_window_change().await?;
                    // Only poll clipboard if enabled in config
                    if self.clipboard_enabled {
                        self.check_clipboard_change().await?;
                    }

                    // Screen capture requires both config flag AND loop flag to be enabled
                    if self.screen_capture_enabled && self.screen_capture_loop_enabled {
                        if let Some(bus) = self.bus.as_ref().cloned() {
                            self.poll_screen_capture(&bus).await;
                        }
                    }

                    // Clear stale vision frame regardless of capture state.
                    if let Some(last) = self.last_capture_time {
                        if last.elapsed() > self.vision_frame_max_age {
                            if let Ok(mut frame) = self.latest_jpeg_frame.lock() {
                                *frame = None;
                            }
                            self.last_capture_time = None;
                        }
                    }
                }
                event = async {
                    match &mut self.bus_rx {
                        Some(rx) => rx.recv().await,
                        None => Err(broadcast::error::RecvError::Closed),
                    }
                } => {
                    match event {
                        Ok(Event::System(SystemEvent::LoopControlRequested { loop_name, enabled })) => {
                            if loop_name == "platform_polling" {
                                self.platform_polling_enabled = enabled;
                                if let Some(bus) = self.bus.as_ref() {
                                    let _ = bus.broadcast(Event::System(SystemEvent::LoopStatusChanged {
                                        loop_name: "platform_polling".to_string(),
                                        enabled,
                                    })).await;
                                }
                            } else if loop_name == "screen_capture" {
                                self.screen_capture_loop_enabled = enabled;
                                if let Some(bus) = self.bus.as_ref() {
                                    let _ = bus.broadcast(Event::System(SystemEvent::LoopStatusChanged {
                                        loop_name: "screen_capture".to_string(),
                                        enabled,
                                    })).await;
                                }
                            }
                        }
                        Ok(Event::System(SystemEvent::ShutdownSignal)) => {
                            return Ok(());
                        }
                        Err(_) => {
                            return Err(ActorError::ChannelClosed("bus channel closed".to_string()));
                        }
                        _ => {}
                    }
                }
                cadence = async {
                    match &mut self.keystroke_rx {
                        Some(rx) => rx.recv().await,
                        None => None,
                    }
                } => {
                    if let Some(cadence) = cadence {
                        if let Some(bus) = &self.bus {
                            // Override burst_detected based on the configurable threshold.
                            let adjusted = bus::events::platform::KeystrokeCadence {
                                burst_detected: cadence.events_per_minute > self.burst_threshold_epm,
                                ..cadence
                            };
                            bus.broadcast(Event::Platform(PlatformEvent::KeystrokePattern(adjusted)))
                                .await
                                .map_err(|e| {
                                    ActorError::RuntimeError(format!(
                                        "broadcast keystroke pattern failed: {}",
                                        e
                                    ))
                                })?;
                        }
                    }
                }
                file_event = async {
                    match &mut self.file_rx {
                        Some(rx) => rx.recv().await,
                        None => None,
                    }
                }, if self.file_rx.is_some() => {
                    if let Some(file_event) = file_event {
                        // Apply exclusion patterns — drop events whose path contains any pattern.
                        let path_str = file_event.path.to_string_lossy();
                        let excluded = self
                            .file_watch_exclude_patterns
                            .iter()
                            .any(|pat| path_str.contains(pat.as_str()));
                        if !excluded {
                            if let Some(bus) = &self.bus {
                                bus.broadcast(Event::Platform(PlatformEvent::FileEvent(file_event)))
                                    .await
                                    .map_err(|e| {
                                        ActorError::RuntimeError(format!(
                                            "broadcast file event failed: {}",
                                            e
                                        ))
                                    })?;
                            }
                        }
                    }
                }
            }
        }
    }

    async fn stop(&mut self) -> Result<(), ActorError> {
        self.bus_rx = None;
        self.bus = None;
        self.file_rx = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::factory::create_platform_adapter;
    use bus::events::platform::PlatformEvent;
    use bus::events::platform_vision::ImageDigest;
    use std::time::Instant;
    use tokio::sync::mpsc;

    struct TestKeystrokeAdapter;

    impl PlatformAdapter for TestKeystrokeAdapter {
        fn active_window(&self) -> Option<WindowContext> {
            None
        }

        fn clipboard_digest(&self) -> Option<ClipboardDigest> {
            None
        }

        fn subscribe_file_events(
            &self,
            _tx: mpsc::Sender<bus::events::platform::FileEvent>,
            _paths: &[std::path::PathBuf],
        ) {
        }

        fn subscribe_keystroke_patterns(&self, tx: mpsc::Sender<KeystrokeCadence>) {
            std::thread::spawn(move || {
                let cadence = KeystrokeCadence {
                    events_per_minute: 132.0,
                    burst_detected: false,
                    idle_duration: Duration::from_millis(250),
                    timestamp: Instant::now(),
                };
                let _ = tx.blocking_send(cadence);
            });
        }

        fn screen_capture(
            &self,
        ) -> Result<bus::events::platform_vision::ImageDigest, crate::error::PlatformError>
        {
            Err(crate::error::PlatformError::ScreenCaptureNotImplemented)
        }

        fn screen_capture_png(
            &self,
            _max_dim: u32,
        ) -> Result<Vec<u8>, crate::error::PlatformError> {
            Err(crate::error::PlatformError::NotAvailable(
                "screen_capture_png not implemented in test adapter".to_string(),
            ))
        }
    }

    struct TestScreenCaptureAdapter {
        digest: ImageDigest,
        png: Vec<u8>,
    }

    impl PlatformAdapter for TestScreenCaptureAdapter {
        fn active_window(&self) -> Option<WindowContext> {
            None
        }

        fn clipboard_digest(&self) -> Option<ClipboardDigest> {
            None
        }

        fn subscribe_file_events(
            &self,
            _tx: mpsc::Sender<bus::events::platform::FileEvent>,
            _paths: &[std::path::PathBuf],
        ) {
        }

        fn subscribe_keystroke_patterns(&self, _tx: mpsc::Sender<KeystrokeCadence>) {}

        fn screen_capture(&self) -> Result<ImageDigest, crate::error::PlatformError> {
            Ok(self.digest.clone())
        }

        fn screen_capture_png(
            &self,
            _max_dim: u32,
        ) -> Result<Vec<u8>, crate::error::PlatformError> {
            Ok(self.png.clone())
        }
    }

    #[test]
    fn platform_actor_implements_actor_trait() {
        let adapter = create_platform_adapter();
        let actor = PlatformActor::new(adapter);
        assert_eq!(actor.name(), "platform");
    }

    #[tokio::test]
    async fn platform_actor_starts_and_stops() {
        let adapter = create_platform_adapter();
        let mut actor = PlatformActor::new(adapter);

        let bus = Arc::new(EventBus::new());
        actor.start(bus).await.expect("start should succeed");

        actor.stop().await.expect("stop should succeed");
        assert!(actor.bus_rx.is_none());
        assert!(actor.bus.is_none());
    }

    #[tokio::test]
    async fn platform_actor_stops_on_shutdown_signal() {
        let adapter = create_platform_adapter();
        let mut actor = PlatformActor::new(adapter).with_poll_interval(Duration::from_millis(100));

        let bus = Arc::new(EventBus::new());
        actor
            .start(bus.clone())
            .await
            .expect("start should succeed");

        // Spawn the run loop
        let run_handle = tokio::spawn(async move { actor.run().await });

        // Give it a moment to start
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Send shutdown signal
        bus.broadcast(Event::System(SystemEvent::ShutdownSignal))
            .await
            .expect("broadcast should succeed");

        // Run loop should exit cleanly
        let result = tokio::time::timeout(Duration::from_secs(1), run_handle).await;
        assert!(result.is_ok(), "actor should stop within timeout");
        assert!(result.unwrap().is_ok(), "run should return Ok");
    }

    #[test]
    fn with_idle_threshold_sets_threshold_correctly() {
        let adapter = create_platform_adapter();
        let actor = PlatformActor::new(adapter).with_idle_threshold(15.0);

        assert_eq!(
            actor.current_idle_threshold(),
            15.0,
            "idle threshold should be 15.0"
        );
    }

    #[test]
    fn with_poll_interval_sets_intervals_correctly() {
        let adapter = create_platform_adapter();
        let custom_interval = Duration::from_millis(200);
        let actor = PlatformActor::new(adapter).with_poll_interval(custom_interval);

        assert_eq!(
            actor.current_poll_interval(),
            custom_interval,
            "poll_interval should match configured value"
        );
        assert_eq!(
            actor.current_normal_poll_interval(),
            custom_interval,
            "normal_poll_interval should match configured value"
        );
    }

    #[test]
    fn default_idle_threshold_is_ten_percent() {
        let adapter = create_platform_adapter();
        let actor = PlatformActor::new(adapter);

        assert_eq!(
            actor.current_idle_threshold(),
            10.0,
            "default idle threshold should be 10.0%"
        );
    }

    #[test]
    fn default_poll_interval_is_500ms() {
        let adapter = create_platform_adapter();
        let actor = PlatformActor::new(adapter);

        assert_eq!(
            actor.current_poll_interval(),
            Duration::from_millis(500),
            "default poll interval should be 500ms"
        );
        assert_eq!(
            actor.current_normal_poll_interval(),
            Duration::from_millis(500),
            "default normal poll interval should be 500ms"
        );
    }

    #[tokio::test]
    async fn cpu_idle_polling_logic_exists_in_run_loop() {
        // This test verifies that the actor can start and run with CPU monitoring.
        // We cannot deterministically control CPU usage in a test, but we can verify
        // that the actor runs without panicking and that the idle threshold config
        // is wired correctly through the constructor.

        let adapter = create_platform_adapter();
        let mut actor = PlatformActor::new(adapter)
            .with_idle_threshold(20.0)
            .with_poll_interval(Duration::from_millis(50));

        // Verify threshold was set
        assert_eq!(actor.current_idle_threshold(), 20.0);

        let bus = Arc::new(EventBus::new());
        actor
            .start(bus.clone())
            .await
            .expect("start should succeed");

        // Spawn the run loop
        let run_handle = tokio::spawn(async move { actor.run().await });

        // Let the actor run through a few poll cycles
        // (enough time for CPU refresh and interval adaptation logic to execute)
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Send shutdown signal
        bus.broadcast(Event::System(SystemEvent::ShutdownSignal))
            .await
            .expect("broadcast should succeed");

        // Run loop should exit cleanly
        let result = tokio::time::timeout(Duration::from_secs(1), run_handle).await;
        assert!(result.is_ok(), "actor should stop within timeout");
        assert!(
            result.unwrap().is_ok(),
            "run loop should complete without error"
        );
    }

    #[tokio::test]
    async fn platform_actor_forwards_keystroke_patterns_to_bus() {
        let adapter: Box<dyn PlatformAdapter> = Box::new(TestKeystrokeAdapter);
        let mut actor = PlatformActor::new(adapter).with_poll_interval(Duration::from_millis(200));
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor
            .start(bus.clone())
            .await
            .expect("platform actor start should succeed");

        let run_handle = tokio::spawn(async move { actor.run().await });

        let mut observed = None;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            if let Ok(Ok(Event::Platform(PlatformEvent::KeystrokePattern(cadence)))) =
                tokio::time::timeout(Duration::from_millis(250), rx.recv()).await
            {
                observed = Some(cadence);
                break;
            }
        }

        bus.broadcast(Event::System(SystemEvent::ShutdownSignal))
            .await
            .expect("shutdown broadcast should succeed");

        let join_result = tokio::time::timeout(Duration::from_secs(1), run_handle).await;
        assert!(join_result.is_ok(), "run loop should exit after shutdown");
        assert!(
            join_result.expect("timeout already checked").is_ok(),
            "run loop should return Ok"
        );

        let cadence = observed.expect("expected KeystrokePattern event from platform actor");
        assert_eq!(cadence.events_per_minute, 132.0);
        assert!(!cadence.burst_detected);
        assert_eq!(cadence.idle_duration, Duration::from_millis(250));
    }

    #[tokio::test]
    async fn screen_capture_enabled_stores_frame_in_arc_when_capture_succeeds() {
        let expected_png = vec![137, 80, 78, 71, 13, 10, 26, 10, 1, 2, 3, 4];
        let adapter: Box<dyn PlatformAdapter> = Box::new(TestScreenCaptureAdapter {
            digest: ImageDigest::new([3u8; 32]),
            png: expected_png.clone(),
        });
        let mut actor = PlatformActor::new(adapter).with_screen_capture_enabled(true);
        let bus = Arc::new(EventBus::new());

        actor.poll_screen_capture(&bus).await;

        let frame_store = actor.latest_frame_store();
        let frame = frame_store
            .lock()
            .expect("frame store lock should succeed")
            .clone();
        assert_eq!(frame, Some(expected_png));
    }
}
