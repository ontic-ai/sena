//! CTP Actor — runs the Continuous Thought Processing pipeline.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bus::events::ctp::ContextSnapshot;
use tokio::sync::broadcast;
use tokio::time::interval;

use bus::events::transparency::TransparencyQuery;
use bus::events::{CTPEvent, PlatformEvent, SystemEvent, TransparencyEvent};
use bus::{Actor, ActorError, Event, EventBus};

use crate::context_assembler::ContextAssembler;
use crate::signal_buffer::SignalBuffer;
use crate::transparency_query::handle_current_observation;
use crate::trigger_gate::TriggerGate;

/// CTP Actor — orchestrates context assembly and thought triggering.
///
/// Pipeline: Platform Events → Signal Buffer → Context Assembler → Trigger Gate → ThoughtEvent
pub struct CTPActor {
    buffer: SignalBuffer,
    assembler: ContextAssembler,
    gate: TriggerGate,
    latest_snapshot: Option<ContextSnapshot>,
    session_start: Instant,
    bus: Option<Arc<EventBus>>,
    bus_rx: Option<broadcast::Receiver<Event>>,
    poll_interval: Duration,
}

impl CTPActor {
    /// Create a new CTP actor.
    ///
    /// # Arguments
    /// * `trigger_interval` - How often the trigger gate should fire
    /// * `buffer_window` - How long to keep events in the signal buffer
    /// * `poll_interval` - How often to check the trigger gate (default: 1 second)
    pub fn new(
        trigger_interval: Duration,
        buffer_window: Duration,
        poll_interval: Duration,
    ) -> Self {
        Self {
            buffer: SignalBuffer::new(buffer_window),
            assembler: ContextAssembler::new(),
            gate: TriggerGate::new(trigger_interval),
            latest_snapshot: None,
            session_start: Instant::now(),
            bus: None,
            bus_rx: None,
            poll_interval,
        }
    }

    /// Configure trigger sensitivity in [0.0, 1.0].
    pub fn with_trigger_sensitivity(mut self, sensitivity: f64) -> Self {
        self.gate.set_sensitivity(sensitivity);
        self
    }
}

#[async_trait]
impl Actor for CTPActor {
    fn name(&self) -> &'static str {
        "ctp"
    }

    async fn start(&mut self, bus: Arc<EventBus>) -> Result<(), ActorError> {
        // Subscribe to broadcast channel for all events
        let rx = bus.subscribe_broadcast();
        self.bus_rx = Some(rx);
        self.bus = Some(bus.clone());

        bus.broadcast(Event::System(SystemEvent::ActorReady { actor_name: "CTP" }))
            .await
            .map_err(|e| {
                ActorError::StartupFailed(format!("broadcast ActorReady failed: {}", e))
            })?;

        Ok(())
    }

    async fn run(&mut self) -> Result<(), ActorError> {
        let bus = self.bus.clone().ok_or_else(|| {
            ActorError::RuntimeError("bus not initialized in start()".to_string())
        })?;

        let mut bus_rx = self.bus_rx.take().ok_or_else(|| {
            ActorError::RuntimeError("bus_rx not initialized in start()".to_string())
        })?;

        // Create a ticker for periodic trigger checks
        let mut ticker = interval(self.poll_interval);

        loop {
            tokio::select! {
                // Handle incoming bus events
                event_result = bus_rx.recv() => {
                    match event_result {
                        Ok(event) => {
                            match event {
                                // Handle platform events
                                Event::Platform(platform_event) => {
                                    self.handle_platform_event(platform_event);
                                }
                                // Handle transparency queries
                                Event::Transparency(TransparencyEvent::QueryRequested(
                                    TransparencyQuery::CurrentObservation,
                                )) => {
                                    // Only CTP handles CurrentObservation; other queries are
                                    // handled by memory and inference actors respectively.
                                    if let Err(e) = self.handle_observation_query(&bus).await {
                                        eprintln!("CTP actor failed to handle observation query: {}", e);
                                    }
                                }
                                // Handle shutdown signal
                                Event::System(SystemEvent::ShutdownSignal) => {
                                    break;
                                }
                                // Ignore other events
                                _ => {}
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            // Log lag but continue
                            // In production this would go to a logger
                            eprintln!("CTP actor lagged behind by {} events", n);
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            return Err(ActorError::ChannelClosed("broadcast channel closed".to_string()));
                        }
                    }
                }

                // Periodic trigger check
                _ = ticker.tick() => {
                    let snapshot = self.refresh_snapshot();

                    // Emit ContextSnapshotReady event on each tick so downstream
                    // actors can observe context evolution even when no trigger fires.
                    bus.broadcast(Event::CTP(CTPEvent::ContextSnapshotReady(snapshot.clone())))
                        .await
                        .map_err(|e| ActorError::RuntimeError(format!("failed to broadcast ContextSnapshotReady: {}", e)))?;

                    // Check if we should trigger
                    if self.gate.should_trigger(&snapshot) {
                        // Emit ThoughtEventTriggered event
                        bus.broadcast(Event::CTP(CTPEvent::ThoughtEventTriggered(snapshot)))
                            .await
                            .map_err(|e| ActorError::RuntimeError(format!("failed to broadcast ThoughtEventTriggered: {}", e)))?;
                    }
                }
            }
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), ActorError> {
        // Clean up resources
        self.bus_rx = None;
        self.bus = None;
        Ok(())
    }
}

impl CTPActor {
    fn refresh_snapshot(&mut self) -> ContextSnapshot {
        self.buffer.prune();
        let snapshot = self.assembler.assemble_with_previous(
            &self.buffer,
            self.session_start,
            self.latest_snapshot.as_ref(),
        );
        self.latest_snapshot = Some(snapshot.clone());
        snapshot
    }

    /// Handle a platform event by pushing it into the signal buffer.
    fn handle_platform_event(&mut self, event: PlatformEvent) {
        match event {
            PlatformEvent::WindowChanged(ctx) => {
                self.buffer.push_window(ctx);
            }
            PlatformEvent::ClipboardChanged(digest) => {
                self.buffer.push_clipboard(digest);
            }
            PlatformEvent::FileEvent(file_event) => {
                self.buffer.push_file_event(file_event);
            }
            PlatformEvent::KeystrokePattern(cadence) => {
                self.buffer.push_keystroke(cadence);
            }
        }

        self.refresh_snapshot();
    }

    /// Handle a `CurrentObservation` transparency query and broadcast the response.
    async fn handle_observation_query(&mut self, bus: &Arc<EventBus>) -> Result<(), String> {
        let response = handle_current_observation(self.refresh_snapshot());

        bus.broadcast(Event::Transparency(
            TransparencyEvent::ObservationResponded(response),
        ))
        .await
        .map_err(|e| format!("failed to broadcast ObservationResponded: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bus::events::platform::{ClipboardDigest, KeystrokeCadence, PlatformEvent, WindowContext};
    use bus::events::transparency::TransparencyQuery;

    #[tokio::test]
    async fn test_ctp_actor_starts_and_stops() {
        let mut actor = CTPActor::new(
            Duration::from_secs(60),
            Duration::from_secs(300),
            Duration::from_millis(100),
        );

        let bus = Arc::new(EventBus::new());

        // Start the actor
        assert!(actor.start(bus.clone()).await.is_ok());

        // Stop the actor
        assert!(actor.stop().await.is_ok());
    }

    #[tokio::test]
    async fn test_ctp_actor_stops_on_shutdown() {
        let mut actor = CTPActor::new(
            Duration::from_secs(60),
            Duration::from_secs(300),
            Duration::from_millis(100),
        );

        let bus = Arc::new(EventBus::new());
        actor
            .start(bus.clone())
            .await
            .expect("actor start should succeed in test");

        // Spawn actor run in background
        let run_handle = tokio::spawn(async move { actor.run().await });

        // Give actor time to start listening
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Send shutdown signal
        bus.broadcast(Event::System(SystemEvent::ShutdownSignal))
            .await
            .expect("shutdown signal broadcast should succeed in test");

        // Wait for actor to complete
        let result = tokio::time::timeout(Duration::from_secs(2), run_handle).await;

        // Actor should have stopped cleanly
        assert!(result.is_ok());
        assert!(result.expect("run_handle should complete").is_ok());
    }

    #[test]
    fn test_ctp_actor_name() {
        let actor = CTPActor::new(
            Duration::from_secs(60),
            Duration::from_secs(300),
            Duration::from_millis(100),
        );
        assert_eq!(actor.name(), "ctp");
    }

    #[tokio::test]
    async fn test_ctp_actor_handles_transparency_query() {
        let mut actor = CTPActor::new(
            Duration::from_secs(60),
            Duration::from_secs(300),
            Duration::from_millis(100),
        );

        let bus = Arc::new(EventBus::new());
        let mut bus_rx = bus.subscribe_broadcast();

        // Start the actor
        actor
            .start(bus.clone())
            .await
            .expect("actor start should succeed in test");

        // Spawn actor run in background
        let run_handle = tokio::spawn(async move { actor.run().await });

        // Give actor time to start listening
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Send a transparency query
        let query = TransparencyQuery::CurrentObservation;
        bus.broadcast(Event::Transparency(TransparencyEvent::QueryRequested(
            query,
        )))
        .await
        .expect("transparency query broadcast should succeed in test");

        // Receive events until we get the response
        let mut found_response = false;
        for _ in 0..10 {
            match tokio::time::timeout(Duration::from_millis(500), bus_rx.recv()).await {
                Ok(Ok(event)) => {
                    if let Event::Transparency(TransparencyEvent::ObservationResponded(response)) =
                        event
                    {
                        // Verify snapshot is present
                        assert_eq!(response.snapshot.active_app.app_name, "Unknown");
                        found_response = true;
                        break;
                    }
                }
                _ => continue,
            }
        }

        assert!(found_response, "did not receive ObservationResponded event");

        // Send shutdown signal
        bus.broadcast(Event::System(SystemEvent::ShutdownSignal))
            .await
            .expect("shutdown signal broadcast should succeed in test");

        // Wait for actor to stop
        let result = tokio::time::timeout(Duration::from_secs(2), run_handle).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn observation_query_reflects_latest_platform_context() {
        let mut actor = CTPActor::new(
            Duration::from_secs(60),
            Duration::from_secs(300),
            Duration::from_millis(100),
        );

        let bus = Arc::new(EventBus::new());
        let mut bus_rx = bus.subscribe_broadcast();

        actor
            .start(bus.clone())
            .await
            .expect("actor start should succeed in test");

        let run_handle = tokio::spawn(async move { actor.run().await });

        tokio::time::sleep(Duration::from_millis(50)).await;

        bus.broadcast(Event::Platform(PlatformEvent::WindowChanged(
            WindowContext {
                app_name: "Code".to_string(),
                window_title: Some("shell.rs".to_string()),
                bundle_id: Some("com.microsoft.VSCode".to_string()),
                timestamp: Instant::now(),
            },
        )))
        .await
        .expect("window event broadcast should succeed in test");

        bus.broadcast(Event::Platform(PlatformEvent::ClipboardChanged(
            ClipboardDigest {
                digest: Some("digest-abc".to_string()),
                char_count: 12,
                timestamp: Instant::now(),
            },
        )))
        .await
        .expect("clipboard event broadcast should succeed in test");

        bus.broadcast(Event::Platform(PlatformEvent::KeystrokePattern(
            KeystrokeCadence {
                events_per_minute: 144.0,
                burst_detected: true,
                idle_duration: Duration::from_secs(3),
            },
        )))
        .await
        .expect("keystroke event broadcast should succeed in test");

        tokio::time::sleep(Duration::from_millis(50)).await;

        bus.broadcast(Event::Transparency(TransparencyEvent::QueryRequested(
            TransparencyQuery::CurrentObservation,
        )))
        .await
        .expect("transparency query broadcast should succeed in test");

        let mut response = None;
        for _ in 0..20 {
            match tokio::time::timeout(Duration::from_millis(250), bus_rx.recv()).await {
                Ok(Ok(Event::Transparency(TransparencyEvent::ObservationResponded(resp)))) => {
                    response = Some(resp);
                    break;
                }
                Ok(Ok(_)) => continue,
                Ok(Err(_)) | Err(_) => continue,
            }
        }

        let response = response.expect("observation response should be received");
        assert_eq!(response.snapshot.active_app.app_name, "Code");
        assert_eq!(
            response.snapshot.clipboard_digest.as_deref(),
            Some("digest-abc")
        );
        assert_eq!(response.snapshot.keystroke_cadence.events_per_minute, 144.0);
        assert!(response.snapshot.keystroke_cadence.burst_detected);

        bus.broadcast(Event::System(SystemEvent::ShutdownSignal))
            .await
            .expect("shutdown signal broadcast should succeed in test");

        let result = tokio::time::timeout(Duration::from_secs(2), run_handle).await;
        assert!(result.is_ok());
    }
}
