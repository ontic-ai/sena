//! STT Actor — speech-to-text processing.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::broadcast;

use bus::{Actor, ActorError, Event, EventBus, SystemEvent};

use crate::SttBackend;

/// STT Actor — handles speech-to-text transcription.
///
/// Pipeline: VoiceInputDetected → STT Backend → TranscriptionCompleted
///
/// This is a stub implementation. Actual STT processing will be added in M4.5.
pub struct SttActor {
    #[allow(dead_code)] // TODO M4.5: backend will be used when STT is implemented
    backend: SttBackend,
    bus: Option<Arc<EventBus>>,
    bus_rx: Option<broadcast::Receiver<Event>>,
}

impl SttActor {
    /// Create a new STT actor with the specified backend.
    pub fn new(backend: SttBackend) -> Self {
        Self {
            backend,
            bus: None,
            bus_rx: None,
        }
    }
}

#[async_trait]
impl Actor for SttActor {
    fn name(&self) -> &'static str {
        "stt"
    }

    async fn start(&mut self, bus: Arc<EventBus>) -> Result<(), ActorError> {
        let rx = bus.subscribe_broadcast();
        self.bus_rx = Some(rx);
        self.bus = Some(bus.clone());

        bus.broadcast(Event::System(SystemEvent::ActorReady { actor_name: "STT" }))
            .await
            .map_err(|e| {
                ActorError::StartupFailed(format!("broadcast ActorReady failed: {}", e))
            })?;

        Ok(())
    }

    async fn run(&mut self) -> Result<(), ActorError> {
        let mut bus_rx = self.bus_rx.take().ok_or_else(|| {
            ActorError::RuntimeError("bus_rx not initialized in start()".to_string())
        })?;

        loop {
            match bus_rx.recv().await {
                Ok(Event::System(SystemEvent::ShutdownSignal)) => {
                    break;
                }
                Ok(_event) => {
                    // TODO M4.5: handle VoiceInputDetected events
                    // For now, no-op — just wait for shutdown
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    // Lagged — missed some events, continue
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(ActorError::ChannelClosed("bus_rx closed".to_string()));
                }
            }
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), ActorError> {
        // Clean shutdown — no resources to release yet
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stt_actor_boots_and_stops_cleanly() {
        let bus = Arc::new(EventBus::new());
        let mut actor = SttActor::new(SttBackend::Mock);

        // Start actor
        actor.start(Arc::clone(&bus)).await.unwrap();

        // Spawn run loop
        let run_handle = tokio::spawn(async move { actor.run().await });

        // Send shutdown signal
        bus.broadcast(Event::System(SystemEvent::ShutdownSignal))
            .await
            .unwrap();

        // Wait for actor to stop
        let result = run_handle.await.unwrap();
        assert!(result.is_ok());
    }
}
