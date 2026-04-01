//! TTS Actor — text-to-speech generation and playback.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::broadcast;

use bus::{Actor, ActorError, Event, EventBus, SystemEvent};

use crate::TtsBackend;

/// TTS Actor — handles text-to-speech generation and playback.
///
/// Pipeline: SpeakRequested → TTS Backend → Audio Playback → SpeechOutputCompleted
///
/// This is a stub implementation. Actual TTS processing will be added in M4.6.
pub struct TtsActor {
    #[allow(dead_code)] // TODO M4.6: backend will be used when TTS is implemented
    backend: TtsBackend,
    bus: Option<Arc<EventBus>>,
    bus_rx: Option<broadcast::Receiver<Event>>,
}

impl TtsActor {
    /// Create a new TTS actor with the specified backend.
    pub fn new(backend: TtsBackend) -> Self {
        Self {
            backend,
            bus: None,
            bus_rx: None,
        }
    }
}

#[async_trait]
impl Actor for TtsActor {
    fn name(&self) -> &'static str {
        "tts"
    }

    async fn start(&mut self, bus: Arc<EventBus>) -> Result<(), ActorError> {
        let rx = bus.subscribe_broadcast();
        self.bus_rx = Some(rx);
        self.bus = Some(bus.clone());

        bus.broadcast(Event::System(SystemEvent::ActorReady { actor_name: "TTS" }))
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
                    // TODO M4.6: handle SpeakRequested events
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
    async fn tts_actor_boots_and_stops_cleanly() {
        let bus = Arc::new(EventBus::new());
        let mut actor = TtsActor::new(TtsBackend::Mock);

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
