//! CTP actor implementation.

use bus::{Actor, ActorError, CTPEvent, Event, EventBus};
use platform::PlatformBackend;
use soul::SoulStore;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::error::CtpError;
use crate::signal::CtpSignal;
use crate::snapshot::SnapshotAssembler;

/// CTP actor: continuous thought processing via signal ingestion and snapshot assembly.
pub struct CtpActor {
    /// Signal receiver channel.
    signal_rx: mpsc::UnboundedReceiver<CtpSignal>,
    /// Snapshot assembler with platform and soul backend dependencies.
    assembler: Option<SnapshotAssembler>,
    /// Bus reference for broadcasting events.
    bus: Option<Arc<EventBus>>,
}

impl CtpActor {
    /// Create a new CTP actor with injected platform and soul backends.
    pub fn new(
        platform: Arc<dyn PlatformBackend>,
        soul: Arc<dyn SoulStore>,
    ) -> (Self, mpsc::UnboundedSender<CtpSignal>) {
        let (signal_tx, signal_rx) = mpsc::unbounded_channel();
        let assembler = SnapshotAssembler::new(platform, soul);

        let actor = Self {
            signal_rx,
            assembler: Some(assembler),
            bus: None,
        };

        (actor, signal_tx)
    }

    /// Process a single signal: assemble snapshot, log counts, emit thought event.
    async fn process_signal(&self, signal: CtpSignal) -> Result<(), CtpError> {
        let assembler = self
            .assembler
            .as_ref()
            .ok_or_else(|| CtpError::SnapshotAssemblyFailed("assembler not initialized".into()))?;

        let bus = self
            .bus
            .as_ref()
            .ok_or_else(|| CtpError::BusError("bus not initialized".into()))?;

        // Assemble snapshot from current platform state.
        let snapshot = assembler.assemble().await?;

        // Count populated vs missing fields.
        let populated_count = self.count_populated_fields(&snapshot);
        let total_fields = 6;
        let missing_count = total_fields - populated_count;

        // Log stub CTP tick.
        info!(
            "CTP TICK: snapshot assembled [signal: {:?}, populated: {}, missing: {}]",
            signal.signal_type(),
            populated_count,
            missing_count
        );

        // Emit thought-trigger event carrying the snapshot.
        bus.broadcast(Event::CTP(Box::new(CTPEvent::ThoughtEventTriggered(
            snapshot,
        ))))
        .await?;

        Ok(())
    }

    /// Count how many fields in the snapshot are meaningfully populated.
    fn count_populated_fields(&self, snapshot: &bus::ContextSnapshot) -> usize {
        let mut count = 0;

        if snapshot.active_app.app_name != "unknown" {
            count += 1;
        }

        if !snapshot.recent_files.is_empty() {
            count += 1;
        }

        if snapshot.clipboard_digest.is_some() {
            count += 1;
        }

        if snapshot.keystroke_cadence.events_per_minute > 0.0 {
            count += 1;
        }

        count += 1; // session_duration always present
        count += 1; // timestamp always present

        count
    }
}

impl Actor for CtpActor {
    fn name(&self) -> &'static str {
        "CtpActor"
    }

    async fn start(&mut self, bus: Arc<EventBus>) -> Result<(), ActorError> {
        info!("CtpActor starting");
        self.bus = Some(bus.clone());

        bus.broadcast(Event::CTP(Box::new(CTPEvent::LoopStarted)))
            .await
            .map_err(|e| ActorError::StartupFailed(e.to_string()))?;

        Ok(())
    }

    async fn run(&mut self) -> Result<(), ActorError> {
        info!("CtpActor run loop started");

        loop {
            tokio::select! {
                Some(signal) = self.signal_rx.recv() => {
                    if let Err(e) = self.process_signal(signal).await {
                        warn!("CTP signal processing error: {}", e);
                    }
                }
                else => {
                    info!("CtpActor signal channel closed, stopping");
                    break;
                }
            }
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), ActorError> {
        info!("CtpActor stopping");

        if let Some(bus) = &self.bus {
            if let Err(e) = bus
                .broadcast(Event::CTP(Box::new(CTPEvent::LoopStopped)))
                .await
            {
                warn!("Failed to broadcast CTP loop stopped event: {}", e);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform::{ClipboardDigest, PlatformError, PlatformSignal, WindowContext};
    use soul::SoulSummary;
    use std::time::{Duration, Instant};

    struct StubPlatform;

    impl PlatformBackend for StubPlatform {
        fn active_window(&self) -> Result<PlatformSignal, PlatformError> {
            Ok(PlatformSignal::Window(WindowContext {
                app_name: "TestApp".to_string(),
                window_title: Some("Test Window".to_string()),
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
            Ok(PlatformSignal::Keystroke(platform::KeystrokeCadence {
                events_per_minute: 60.0,
                burst_detected: false,
                idle_duration: Duration::from_secs(10),
                timestamp: Instant::now(),
            }))
        }

        fn screen_frame(&self) -> Result<PlatformSignal, PlatformError> {
            Ok(PlatformSignal::ScreenFrame(platform::ScreenFrame {
                width: 1,
                height: 1,
                rgb_data: vec![0, 0, 0],
                timestamp: Instant::now(),
            }))
        }
    }

    struct StubSoul;

    impl soul::SoulStore for StubSoul {
        fn write_event(
            &mut self,
            _description: String,
            _app_context: Option<String>,
            _timestamp: std::time::SystemTime,
        ) -> Result<u64, soul::SoulError> {
            Ok(1)
        }

        fn read_summary(
            &self,
            _max_events: usize,
            _max_chars: Option<usize>,
        ) -> Result<SoulSummary, soul::SoulError> {
            Ok(SoulSummary {
                content: "test summary".to_string(),
                event_count: 0,
            })
        }

        fn read_event(
            &self,
            _row_id: u64,
        ) -> Result<Option<soul::SoulEventRecord>, soul::SoulError> {
            Ok(None)
        }

        fn write_identity_signal(
            &mut self,
            _key: &str,
            _value: &str,
        ) -> Result<(), soul::SoulError> {
            Ok(())
        }

        fn read_identity_signal(&self, _key: &str) -> Result<Option<String>, soul::SoulError> {
            Ok(None)
        }

        fn read_all_identity_signals(&self) -> Result<Vec<soul::IdentitySignal>, soul::SoulError> {
            Ok(vec![])
        }

        fn increment_identity_counter(
            &mut self,
            _key: &str,
            _delta: u64,
        ) -> Result<(), soul::SoulError> {
            Ok(())
        }

        fn write_temporal_pattern(
            &mut self,
            _pattern: soul::TemporalPattern,
        ) -> Result<(), soul::SoulError> {
            Ok(())
        }

        fn read_temporal_patterns(&self) -> Result<Vec<soul::TemporalPattern>, soul::SoulError> {
            Ok(vec![])
        }

        fn initialize(&mut self) -> Result<(), soul::SoulError> {
            Ok(())
        }

        fn close(&mut self) -> Result<(), soul::SoulError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn actor_constructs_with_dependencies() {
        let platform = Arc::new(StubPlatform) as Arc<dyn PlatformBackend>;
        let soul = Arc::new(StubSoul) as Arc<dyn soul::SoulStore>;
        let (actor, _signal_tx) = CtpActor::new(platform, soul);

        assert_eq!(actor.name(), "CtpActor");
    }

    #[tokio::test]
    async fn actor_starts_and_broadcasts_loop_started() {
        let platform = Arc::new(StubPlatform) as Arc<dyn PlatformBackend>;
        let soul = Arc::new(StubSoul) as Arc<dyn soul::SoulStore>;
        let (mut actor, _signal_tx) = CtpActor::new(platform, soul);

        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor.start(bus).await.unwrap();

        let event = rx.recv().await.unwrap();
        match event {
            Event::CTP(boxed) => match *boxed {
                CTPEvent::LoopStarted => {}
                _ => panic!("expected LoopStarted"),
            },
            _ => panic!("expected CTP event"),
        }
    }

    #[tokio::test]
    async fn actor_processes_signal_and_emits_thought_event() {
        let platform = Arc::new(StubPlatform) as Arc<dyn PlatformBackend>;
        let soul = Arc::new(StubSoul) as Arc<dyn soul::SoulStore>;
        let (mut actor, signal_tx) = CtpActor::new(platform, soul);

        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor.start(bus.clone()).await.unwrap();

        let _ = rx.recv().await;

        signal_tx.send(CtpSignal::ManualTick).unwrap();

        let actor_handle = tokio::spawn(async move { actor.run().await });

        let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap();

        match event {
            Event::CTP(boxed) => match *boxed {
                CTPEvent::ThoughtEventTriggered(snapshot) => {
                    assert_eq!(snapshot.active_app.app_name, "TestApp");
                }
                _ => panic!("expected ThoughtEventTriggered"),
            },
            _ => panic!("expected CTP event"),
        }

        drop(signal_tx);
        actor_handle.await.unwrap().unwrap();
    }
}
