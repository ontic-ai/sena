//! CTP actor implementation.

use bus::{Actor, ActorError, CTPEvent, ContextSnapshot, Event, EventBus};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::time::interval;
use tracing::{debug, info, warn};

use crate::context_assembler::ContextAssembler;
use crate::error::CtpError;
use crate::signal::CtpSignal;
use crate::signal_buffer::SignalBuffer;
use crate::transparency_query;
use crate::trigger_gate::TriggerGate;

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
    /// Session start time for calculating session duration.
    session_start: Instant,
    /// Last assembled snapshot (for context preservation).
    last_snapshot: Option<ContextSnapshot>,
    /// Cached identity signal from Soul (preserved across snapshots).
    cached_identity_signal: Option<bus::events::soul::DistilledIdentitySignal>,
    /// Loop enabled state (controlled by IPC).
    loop_enabled: bool,
    /// Periodic processing interval.
    poll_interval: Duration,
    /// True after boot is complete so proactive thoughts can fire safely.
    boot_complete: bool,
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
            session_start: Instant::now(),
            last_snapshot: None,
            cached_identity_signal: None,
            loop_enabled: true,
            poll_interval: Duration::from_secs(1),
            boot_complete: false,
        };

        (actor, signal_tx)
    }

    /// Process a single signal by ingesting it into the rolling buffer.
    async fn process_signal(&mut self, signal: CtpSignal) -> Result<(), CtpError> {
        debug!("CTP ingesting signal: {:?}", signal.signal_type());
        // Ingest signal into buffer
        self.ingest_signal(signal);

        Ok(())
    }

    async fn run_ctp_cycle(&mut self) -> Result<(), CtpError> {
        if !self.loop_enabled {
            debug!("CTP loop disabled, skipping periodic cycle");
            return Ok(());
        }

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

        // CTP emits raw context only. Interpretation is deferred to the model.
        snapshot.user_state = None;
        snapshot.inferred_task = None;

        // Emit snapshot ready event
        bus.broadcast(Event::CTP(Box::new(CTPEvent::ContextSnapshotReady(
            snapshot.clone(),
        ))))
        .await?;

        let should_trigger = self.trigger_gate.should_trigger(&snapshot);

        if self.boot_complete && should_trigger {
            info!(
                "CTP THOUGHT TRIGGERED: app={}, window={:?}",
                snapshot.active_app.app_name, snapshot.active_app.window_title
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
            Event::System(bus::SystemEvent::LoopControlRequested { loop_name, enabled })
                if loop_name == "ctp" =>
            {
                info!(
                    enabled = enabled,
                    "CTP loop control requested, updating state"
                );
                self.loop_enabled = enabled;

                // Broadcast status changed event
                if let Some(bus) = &self.bus {
                    let _ = bus
                        .broadcast(Event::System(bus::SystemEvent::LoopStatusChanged {
                            loop_name: "ctp".to_string(),
                            enabled,
                        }))
                        .await;
                }
            }
            Event::System(bus::SystemEvent::BootComplete) => {
                info!("CTP boot complete received, proactive triggering enabled");
                self.boot_complete = true;
            }
            Event::Transparency(bus::TransparencyEvent::QueryRequested(
                bus::TransparencyQuery::CurrentObservation,
            )) => {
                let bus_opt = self.bus.clone();
                if let Some(bus) = bus_opt
                    && let Err(e) = self.handle_observation_query(&bus).await
                {
                    warn!("CTP failed to handle observation query: {}", e);
                }
            }
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

    /// Handle a `CurrentObservation` transparency query and broadcast the response.
    async fn handle_observation_query(&mut self, bus: &Arc<EventBus>) -> Result<(), String> {
        // Assemble current state snapshot
        let snapshot = self.context_assembler.assemble_with_previous(
            &self.signal_buffer,
            self.session_start,
            self.last_snapshot.as_ref(),
        );

        let result = Box::new(bus::TransparencyResult::Observation(
            transparency_query::handle_current_observation(snapshot),
        ));

        bus.broadcast(Event::Transparency(bus::TransparencyEvent::QueryResponse {
            query: bus::TransparencyQuery::CurrentObservation,
            result,
        }))
        .await
        .map_err(|e| format!("failed to broadcast observation response: {}", e))
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
        let mut ticker = interval(self.poll_interval);

        loop {
            tokio::select! {
                Some(signal) = self.signal_rx.recv() => {
                    let is_manual_tick = matches!(signal, CtpSignal::ManualTick);
                    if let Err(e) = self.process_signal(signal).await {
                        warn!("CTP signal processing error: {}", e);
                    } else if is_manual_tick
                        && let Err(e) = self.run_ctp_cycle().await
                    {
                        warn!("CTP periodic cycle error after manual tick: {}", e);
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
                _ = ticker.tick() => {
                    if let Err(e) = self.run_ctp_cycle().await {
                        warn!("CTP periodic cycle error: {}", e);
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

    #[tokio::test]
    async fn actor_emits_raw_snapshot_without_interpretation_fields() {
        let (mut actor, _signal_tx) = CtpActor::new();

        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor.start(bus.clone()).await.expect("start failed");

        bus.broadcast(Event::Platform(
            bus::events::platform::PlatformEvent::ActiveWindowChanged(
                bus::events::platform::WindowContext {
                    app_name: "Code".to_string(),
                    window_title: Some("src/main.rs".to_string()),
                    bundle_id: None,
                    timestamp: Instant::now(),
                },
            ),
        ))
        .await
        .expect("window event should broadcast");

        tokio::spawn(async move {
            let _ = actor.run().await;
        });

        let mut observed_snapshot = false;
        for _ in 0..20 {
            if let Ok(Ok(Event::CTP(boxed))) =
                tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await
            {
                if let CTPEvent::ContextSnapshotReady(snapshot) = *boxed {
                    assert!(snapshot.inferred_task.is_none());
                    assert!(snapshot.user_state.is_none());
                    observed_snapshot = true;
                    break;
                }
            }
        }

        assert!(
            observed_snapshot,
            "CTP should emit raw snapshots without interpretive fields"
        );
    }

    #[tokio::test]
    async fn actor_triggers_thought_after_boot_using_raw_snapshot_gate() {
        let (mut actor, signal_tx) = CtpActor::new();

        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor.start(bus.clone()).await.expect("start failed");
        actor.boot_complete = true;
        actor.trigger_gate.reset();

        tokio::spawn(async move {
            let _ = actor.run().await;
        });

        bus.broadcast(Event::Platform(
            bus::events::platform::PlatformEvent::ActiveWindowChanged(
                bus::events::platform::WindowContext {
                    app_name: "Code".to_string(),
                    window_title: Some("src/main.rs".to_string()),
                    bundle_id: None,
                    timestamp: Instant::now(),
                },
            ),
        ))
        .await
        .expect("window event should broadcast");

        signal_tx
            .send(CtpSignal::ManualTick)
            .expect("manual tick should send");

        let mut observed = false;
        for _ in 0..30 {
            if let Ok(Ok(Event::CTP(boxed))) =
                tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await
            {
                if let CTPEvent::ThoughtEventTriggered(snapshot) = *boxed {
                    assert!(snapshot.inferred_task.is_none());
                    assert!(snapshot.user_state.is_none());
                    observed = true;
                    break;
                }
            }
        }

        assert!(
            observed,
            "CTP should trigger proactive thought from raw snapshot data after boot"
        );
    }
}
