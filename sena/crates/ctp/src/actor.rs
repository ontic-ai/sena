//! CTP actor implementation.

use bus::{Actor, ActorError, CTPEvent, ContextSnapshot, Event, EventBus};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::context_assembler::ContextAssembler;
use crate::error::CtpError;
use crate::pattern_engine::PatternEngine;
use crate::signal::CtpSignal;
use crate::signal_buffer::SignalBuffer;
use crate::trigger_gate::TriggerGate;
use crate::user_state::UserStateClassifier;

/// CTP actor: continuous thought processing via signal ingestion and snapshot assembly.
pub struct CtpActor {
    /// Signal receiver channel.
    signal_rx: mpsc::UnboundedReceiver<CtpSignal>,
    /// Bus reference for broadcasting events.
    bus: Option<Arc<EventBus>>,
    /// Bus broadcast receiver for ingesting platform/soul events.
    bus_rx: Option<tokio::sync::broadcast::Receiver<Event>>,
    /// Signal buffer with rolling time window.
    signal_buffer: SignalBuffer,
    /// Context assembler.
    context_assembler: ContextAssembler,
    /// Trigger gate for deciding when to emit thought events.
    trigger_gate: TriggerGate,
    /// Pattern engine for detecting behavioral signals.
    pattern_engine: PatternEngine,
    /// User state classifier.
    user_state_classifier: UserStateClassifier,
    /// Session start time for calculating session duration.
    session_start: Instant,
    /// Last assembled snapshot (for context preservation).
    last_snapshot: Option<ContextSnapshot>,
    /// Cached identity signal from Soul (preserved across snapshots).
    cached_identity_signal: Option<bus::events::soul::DistilledIdentitySignal>,
}

impl CtpActor {
    /// Create a new CTP actor.
    ///
    /// Returns the actor instance and a signal sender for manual injection.
    /// The actor will also subscribe to the bus broadcast stream on start.
    pub fn new() -> (Self, mpsc::UnboundedSender<CtpSignal>) {
        let (signal_tx, signal_rx) = mpsc::unbounded_channel();

        let actor = Self {
            signal_rx,
            bus: None,
            bus_rx: None,
            signal_buffer: SignalBuffer::new(Duration::from_secs(300)), // 5-minute window
            context_assembler: ContextAssembler::new(),
            trigger_gate: TriggerGate::new(Duration::from_secs(600)) // 10-minute default interval
                .with_sensitivity(0.5),
            pattern_engine: PatternEngine::new(),
            user_state_classifier: UserStateClassifier::new(),
            session_start: Instant::now(),
            last_snapshot: None,
            cached_identity_signal: None,
        };

        (actor, signal_tx)
    }

    /// Process a single signal: ingest into buffer, assemble snapshot, check trigger.
    async fn process_signal(&mut self, signal: CtpSignal) -> Result<(), CtpError> {
        debug!("CTP processing signal: {:?}", signal.signal_type());

        // Ingest signal into buffer
        self.ingest_signal(signal);

        let bus = self
            .bus
            .as_ref()
            .ok_or_else(|| CtpError::BusError("bus not initialized".into()))?;

        // Prune old events from buffer
        self.signal_buffer.prune();

        // Assemble snapshot from current buffer state
        let mut snapshot = self.context_assembler.assemble_with_previous(
            &self.signal_buffer,
            self.session_start,
            self.last_snapshot.as_ref(),
        );

        // Inject cached identity signal if available
        if self.cached_identity_signal.is_some() {
            snapshot.soul_identity_signal = self.cached_identity_signal.clone();
        }

        // Detect patterns from buffer
        let patterns = self.pattern_engine.detect(&self.signal_buffer, &snapshot);

        // Emit pattern events
        for pattern in &patterns {
            bus.broadcast(Event::CTP(Box::new(CTPEvent::SignalPatternDetected(
                pattern.clone(),
            ))))
            .await?;
        }

        // Compute user state from snapshot and patterns
        let user_state = self.user_state_classifier.classify(&snapshot, &patterns);

        // Attach user state to snapshot
        snapshot.user_state = Some(user_state.clone());

        // Emit user state event
        bus.broadcast(Event::CTP(Box::new(CTPEvent::UserStateComputed(
            user_state,
        ))))
        .await?;

        // Emit snapshot ready event
        bus.broadcast(Event::CTP(Box::new(CTPEvent::ContextSnapshotReady(
            snapshot.clone(),
        ))))
        .await?;

        // Check if we should trigger a thought event
        let should_trigger = self.trigger_gate.should_trigger(&snapshot, &patterns);

        if should_trigger {
            info!(
                "CTP THOUGHT TRIGGERED: app={}, task={:?}, patterns={}",
                snapshot.active_app.app_name,
                snapshot.inferred_task.as_ref().map(|t| &t.category),
                patterns.len()
            );

            bus.broadcast(Event::CTP(Box::new(CTPEvent::ThoughtEventTriggered(
                snapshot.clone(),
            ))))
            .await?;
        } else {
            debug!("CTP tick: snapshot assembled, no trigger");
        }

        // Store snapshot for next cycle
        self.last_snapshot = Some(snapshot);

        Ok(())
    }

    /// Ingest a signal into the signal buffer.
    fn ingest_signal(&mut self, signal: CtpSignal) {
        match signal {
            CtpSignal::WindowChanged(ctx) => {
                self.signal_buffer.push_window(ctx);
            }
            CtpSignal::ClipboardChanged(digest) => {
                self.signal_buffer.push_clipboard(digest);
            }
            CtpSignal::FileEvent(event) => {
                self.signal_buffer.push_file_event(event);
            }
            CtpSignal::KeystrokePattern(cadence) => {
                self.signal_buffer.push_keystroke(cadence);
            }
            CtpSignal::ManualTick => {
                // Manual tick doesn't add to buffer, just triggers processing
            }
        }
    }

    /// Process a bus event: extract relevant signals and process them.
    async fn process_bus_event(&mut self, event: Event) -> Result<(), CtpError> {
        match event {
            Event::Platform(platform_event) => {
                use bus::events::platform::PlatformEvent;

                #[allow(deprecated)]
                match platform_event {
                    PlatformEvent::ActiveWindowChanged(ctx) | PlatformEvent::WindowChanged(ctx) => {
                        self.process_signal(CtpSignal::WindowChanged(ctx)).await?;
                    }
                    PlatformEvent::ClipboardChanged(digest) => {
                        self.process_signal(CtpSignal::ClipboardChanged(digest))
                            .await?;
                    }
                    PlatformEvent::FileEvent(file_event) => {
                        self.process_signal(CtpSignal::FileEvent(file_event))
                            .await?;
                    }
                    PlatformEvent::KeystrokeCadenceUpdated(cadence)
                    | PlatformEvent::KeystrokePattern(cadence) => {
                        self.process_signal(CtpSignal::KeystrokePattern(cadence))
                            .await?;
                    }
                    PlatformEvent::VisionFrameAvailable { .. } => {
                        // Vision frames are handled separately if needed
                    }
                }
            }
            Event::Soul(soul_event) => {
                use bus::events::soul::SoulEvent;

                match soul_event {
                    SoulEvent::IdentitySignalDistilled { signal, .. } => {
                        debug!("CTP received identity signal: {:?}", signal.signal_key);
                        self.cached_identity_signal = Some(signal);
                    }
                    SoulEvent::TemporalPatternDetected { pattern, .. } => {
                        debug!("CTP received temporal pattern: {:?}", pattern.pattern_type);
                        // Temporal patterns are logged but not cached currently.
                        // Future enhancement: could use for trigger gating or user state.
                    }
                    _ => {
                        // Ignore other soul events
                    }
                }
            }
            _ => {
                // Ignore other event types
            }
        }

        Ok(())
    }
}

impl Actor for CtpActor {
    fn name(&self) -> &'static str {
        "ctp"
    }

    async fn start(&mut self, bus: Arc<EventBus>) -> Result<(), ActorError> {
        info!("CtpActor starting");
        self.bus = Some(bus.clone());

        // Subscribe to bus broadcast
        let bus_rx = bus.subscribe_broadcast();
        self.bus_rx = Some(bus_rx);

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
                Some(event) = async {
                    match &mut self.bus_rx {
                        Some(rx) => rx.recv().await.ok(),
                        None => None,
                    }
                } => {
                    if let Err(e) = self.process_bus_event(event).await {
                        warn!("CTP bus event processing error: {}", e);
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

        if let Some(bus) = &self.bus
            && let Err(e) = bus
                .broadcast(Event::CTP(Box::new(CTPEvent::LoopStopped)))
                .await
        {
            warn!("Failed to broadcast CTP loop stopped event: {}", e);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn actor_constructs() {
        let (actor, _signal_tx) = CtpActor::new();
        assert_eq!(actor.name(), "ctp");
    }

    #[tokio::test]
    async fn actor_starts_and_broadcasts_loop_started() {
        let (mut actor, _signal_tx) = CtpActor::new();

        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor.start(bus).await.unwrap();

        let event = rx.recv().await.unwrap();
        match event {
            Event::CTP(boxed) => match *boxed {
                CTPEvent::LoopStarted => {}
                _ => panic!("Expected LoopStarted event"),
            },
            _ => panic!("Expected CTP event"),
        }
    }

    #[tokio::test]
    async fn actor_processes_manual_signal() {
        let (mut actor, signal_tx) = CtpActor::new();

        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor.start(bus.clone()).await.unwrap();

        // Spawn the run loop in a separate task
        tokio::spawn(async move {
            let _ = actor.run().await;
        });

        // Send manual tick signal
        signal_tx.send(CtpSignal::ManualTick).unwrap();

        // Wait for events
        let mut snapshot_ready = false;

        for _ in 0..10 {
            if let Ok(event) =
                tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await
            {
                if let Ok(Event::CTP(boxed)) = event {
                    match *boxed {
                        CTPEvent::ContextSnapshotReady(_) => snapshot_ready = true,
                        _ => {}
                    }
                }
            }
        }

        // Should have snapshot ready (first tick doesn't trigger thought due to warm-up)
        assert!(
            snapshot_ready,
            "ContextSnapshotReady event should be emitted"
        );
    }

    #[tokio::test]
    async fn actor_ingests_platform_events_from_bus() {
        let (mut actor, _signal_tx) = CtpActor::new();

        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor.start(bus.clone()).await.unwrap();

        // Spawn the run loop
        tokio::spawn(async move {
            let _ = actor.run().await;
        });

        // Broadcast a platform event
        bus.broadcast(Event::Platform(
            bus::events::platform::PlatformEvent::ActiveWindowChanged(
                bus::events::platform::WindowContext {
                    app_name: "TestApp".to_string(),
                    window_title: None,
                    bundle_id: None,
                    timestamp: Instant::now(),
                },
            ),
        ))
        .await
        .unwrap();

        // Wait for CTP to process and emit snapshot
        let mut found_snapshot = false;
        for _ in 0..10 {
            if let Ok(Ok(Event::CTP(boxed))) =
                tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await
            {
                if matches!(*boxed, CTPEvent::ContextSnapshotReady(_)) {
                    found_snapshot = true;
                    break;
                }
            }
        }

        assert!(
            found_snapshot,
            "CTP should emit ContextSnapshotReady after processing platform event"
        );
    }

    #[tokio::test]
    async fn actor_receives_soul_identity_signal_and_includes_in_snapshot() {
        let (mut actor, signal_tx) = CtpActor::new();

        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor.start(bus.clone()).await.unwrap();

        // Spawn the run loop
        tokio::spawn(async move {
            let _ = actor.run().await;
        });

        // Broadcast a Soul identity signal
        let identity_signal = bus::events::soul::DistilledIdentitySignal {
            signal_key: "voice::rate".to_string(),
            signal_value: "1.2".to_string(),
            confidence: 1.0,
        };

        bus.broadcast(Event::Soul(
            bus::events::soul::SoulEvent::IdentitySignalDistilled {
                signal: identity_signal.clone(),
                causal_id: bus::CausalId::new(),
            },
        ))
        .await
        .unwrap();

        // Give CTP time to process the soul event
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Trigger a manual tick to force snapshot assembly
        signal_tx.send(CtpSignal::ManualTick).unwrap();

        // Wait for snapshot with identity signal
        let mut found_snapshot_with_signal = false;
        for _ in 0..10 {
            if let Ok(Ok(Event::CTP(boxed))) =
                tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await
            {
                if let CTPEvent::ContextSnapshotReady(snapshot) = *boxed {
                    if let Some(signal) = &snapshot.soul_identity_signal {
                        assert_eq!(signal.signal_key, "voice::rate");
                        assert_eq!(signal.signal_value, "1.2");
                        assert_eq!(signal.confidence, 1.0);
                        found_snapshot_with_signal = true;
                        break;
                    }
                }
            }
        }

        assert!(
            found_snapshot_with_signal,
            "CTP should include received Soul identity signal in snapshot"
        );
    }
}
