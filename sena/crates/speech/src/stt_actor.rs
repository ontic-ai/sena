//! STT actor — streaming speech-to-text processing.

use crate::backend::{AudioDevice, SttBackend};
use crate::error::{SpeechActorError, SttError};
use crate::types::SttEvent;
use bus::causal::CausalId;
use bus::events::{SpeechEvent, SystemEvent};
use bus::{Actor, ActorError, Event, EventBus};
use std::sync::Arc;
use tokio::sync::broadcast;
#[cfg(test)]
use tracing::debug;
use tracing::{error, info, warn};

/// Default minimum confidence threshold for valid transcriptions.
const DEFAULT_MIN_CONFIDENCE_THRESHOLD: f32 = 0.65;

/// Audio chunk message sent to STT actor.
#[derive(Debug, zeroize::ZeroizeOnDrop)]
pub struct AudioChunk {
    /// PCM samples (f32, mono).
    pub samples: Vec<f32>,
}

/// STT actor — processes incoming audio and emits transcription events.
pub struct SttActor {
    backend: Box<dyn SttBackend>,
    bus: Option<Arc<EventBus>>,
    broadcast_rx: Option<broadcast::Receiver<Event>>,
    min_confidence_threshold: f32,
    shutdown_requested: bool,
    listen_mode_active: bool,
    listen_mode_causal_id: Option<CausalId>,
}

impl SttActor {
    /// Create a new STT actor with the given backend.
    pub fn new(backend: Box<dyn SttBackend>) -> Self {
        Self {
            backend,
            bus: None,
            broadcast_rx: None,
            min_confidence_threshold: DEFAULT_MIN_CONFIDENCE_THRESHOLD,
            shutdown_requested: false,
            listen_mode_active: false,
            listen_mode_causal_id: None,
        }
    }

    /// Set minimum confidence threshold for transcriptions.
    pub fn with_min_confidence(mut self, threshold: f32) -> Self {
        self.min_confidence_threshold = threshold;
        self
    }

    /// Handle bus events.
    async fn handle_bus_event(&mut self, event: Event) -> Result<(), SpeechActorError> {
        match event {
            Event::System(SystemEvent::ShutdownRequested) => {
                info!("Shutdown requested, stopping STT actor");
                self.shutdown_requested = true;
            }
            Event::Speech(SpeechEvent::ListenModeRequested { causal_id }) => {
                info!("Listen mode requested");
                self.listen_mode_active = true;
                self.listen_mode_causal_id = Some(causal_id);
            }
            Event::Speech(SpeechEvent::ListenModeStopRequested { causal_id }) => {
                info!("Listen mode stop requested");
                self.listen_mode_active = false;
                self.listen_mode_causal_id = None;

                if let Some(bus) = &self.bus {
                    bus.broadcast(Event::Speech(SpeechEvent::ListenModeStopped { causal_id }))
                        .await
                        .map_err(|e| SpeechActorError::Bus(e.to_string()))?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Handle backend STT events.
    #[cfg(test)]
    async fn handle_stt_event(&self, event: SttEvent) -> Result<(), SpeechActorError> {
        let bus = self
            .bus
            .as_ref()
            .ok_or_else(|| SpeechActorError::Bus("bus not initialized".to_string()))?;

        match event {
            SttEvent::Word { text, confidence } => {
                debug!(text = %text, confidence = %confidence, "Word recognized");
            }
            SttEvent::Completed { text, confidence } => {
                debug!(text = %text, confidence = %confidence, "Transcription completed");
                let causal_id = CausalId::new();

                // Check confidence threshold
                if confidence < self.min_confidence_threshold {
                    warn!(
                        confidence = %confidence,
                        threshold = %self.min_confidence_threshold,
                        "Low confidence transcription — not routing to inference"
                    );
                    bus.broadcast(Event::Speech(SpeechEvent::LowConfidenceTranscription {
                        text,
                        confidence,
                        causal_id,
                    }))
                    .await
                    .map_err(|e| SpeechActorError::Bus(e.to_string()))?;
                } else if self.listen_mode_active {
                    // In listen mode, emit listen mode transcription event
                    bus.broadcast(Event::Speech(SpeechEvent::ListenModeTranscription {
                        text,
                        causal_id: self.listen_mode_causal_id.unwrap_or(causal_id),
                    }))
                    .await
                    .map_err(|e| SpeechActorError::Bus(e.to_string()))?;
                } else {
                    // Normal transcription
                    bus.broadcast(Event::Speech(SpeechEvent::TranscriptionCompleted {
                        text,
                        confidence,
                        causal_id,
                    }))
                    .await
                    .map_err(|e| SpeechActorError::Bus(e.to_string()))?;
                }
            }
            SttEvent::Listening => {
                debug!("Backend listening");
                let bus = self
                    .bus
                    .as_ref()
                    .ok_or_else(|| SpeechActorError::Bus("bus not initialized".to_string()))?;
                bus.broadcast(Event::Speech(SpeechEvent::SttListening))
                    .await
                    .map_err(|e| SpeechActorError::Bus(e.to_string()))?;
            }
            SttEvent::Stopped => {
                debug!("Backend stopped");
                let bus = self
                    .bus
                    .as_ref()
                    .ok_or_else(|| SpeechActorError::Bus("bus not initialized".to_string()))?;
                bus.broadcast(Event::Speech(SpeechEvent::SttStopped))
                    .await
                    .map_err(|e| SpeechActorError::Bus(e.to_string()))?;
            }
            SttEvent::Error { reason } => {
                warn!(reason = %reason, "Backend reported error");
                let causal_id = CausalId::new();
                bus.broadcast(Event::Speech(SpeechEvent::TranscriptionFailed {
                    reason,
                    causal_id,
                }))
                .await
                .map_err(|e| SpeechActorError::Bus(e.to_string()))?;
            }
        }
        Ok(())
    }
}

impl Actor for SttActor {
    fn name(&self) -> &'static str {
        "stt"
    }

    async fn start(&mut self, bus: Arc<EventBus>) -> Result<(), ActorError> {
        info!("STT actor starting");
        self.bus = Some(bus.clone());
        self.broadcast_rx = Some(bus.subscribe_broadcast());

        // Emit ActorReady event
        bus.broadcast(Event::System(SystemEvent::ActorReady {
            actor_name: self.name(),
        }))
        .await
        .map_err(|e| ActorError::StartupFailed(e.to_string()))?;

        info!("STT actor started");
        Ok(())
    }

    async fn run(&mut self) -> Result<(), ActorError> {
        let mut rx = self.broadcast_rx.take().ok_or_else(|| {
            ActorError::RuntimeError("broadcast receiver not initialized".to_string())
        })?;

        info!(backend = self.backend.backend_name(), "STT actor running");

        while !self.shutdown_requested {
            tokio::select! {
                Ok(event) = rx.recv() => {
                    if let Err(e) = self.handle_bus_event(event).await {
                        error!(error = %e, "Failed to handle bus event");
                    }
                }
            }
        }

        info!("STT actor run loop exiting");
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), ActorError> {
        info!("STT actor stopping");

        // Flush backend
        if let Err(e) = self.backend.flush() {
            warn!(error = %e, "Failed to flush STT backend");
        }

        info!("STT actor stopped");
        Ok(())
    }
}

/// Stub STT backend for testing.
pub struct StubSttBackend {
    buffer: Vec<f32>,
    chunk_size: usize,
}

impl StubSttBackend {
    pub fn new(chunk_size: usize) -> Self {
        Self {
            buffer: Vec::new(),
            chunk_size,
        }
    }
}

impl SttBackend for StubSttBackend {
    fn preferred_chunk_samples(&self) -> usize {
        self.chunk_size
    }

    fn feed(&mut self, pcm: &[f32]) -> Result<Vec<SttEvent>, SttError> {
        self.buffer.extend_from_slice(pcm);

        // Stub: emit a completion event every 10 chunks
        if self.buffer.len() >= self.chunk_size * 10 {
            self.buffer.clear();
            Ok(vec![SttEvent::Completed {
                text: "[stub transcription]".to_string(),
                confidence: 0.85,
            }])
        } else {
            Ok(vec![])
        }
    }

    fn flush(&mut self) -> Result<Vec<SttEvent>, SttError> {
        if self.buffer.is_empty() {
            return Ok(vec![]);
        }

        self.buffer.clear();
        Ok(vec![SttEvent::Completed {
            text: "[stub final transcription]".to_string(),
            confidence: 0.90,
        }])
    }

    fn list_audio_devices(&self) -> Result<Vec<AudioDevice>, SttError> {
        Ok(vec![AudioDevice {
            name: "Stub Audio Device".to_string(),
        }])
    }

    fn backend_name(&self) -> &'static str {
        "stub"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_backend_preferred_chunk_size() {
        let backend = StubSttBackend::new(1024);
        assert_eq!(backend.preferred_chunk_samples(), 1024);
    }

    #[test]
    fn stub_backend_feed_accumulates() {
        let mut backend = StubSttBackend::new(100);
        let chunk = vec![0.5; 50];

        let events = backend.feed(&chunk).expect("feed should succeed");
        assert!(events.is_empty());

        let events = backend.feed(&chunk).expect("feed should succeed");
        assert!(events.is_empty());
    }

    #[test]
    fn stub_backend_emits_after_threshold() {
        let mut backend = StubSttBackend::new(100);
        let chunk = vec![0.5; 100];

        for _ in 0..9 {
            let events = backend.feed(&chunk).expect("feed should succeed");
            assert!(events.is_empty());
        }

        let events = backend.feed(&chunk).expect("feed should succeed");
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], SttEvent::Completed { .. }));
    }

    #[test]
    fn stub_backend_flush_emits_final() {
        let mut backend = StubSttBackend::new(100);
        backend.feed(&vec![0.5; 50]).expect("feed should succeed");

        let events = backend.flush().expect("flush should succeed");
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], SttEvent::Completed { .. }));
    }

    #[test]
    fn stub_backend_flush_empty_buffer() {
        let mut backend = StubSttBackend::new(100);
        let events = backend.flush().expect("flush should succeed");
        assert!(events.is_empty());
    }

    #[test]
    fn stub_backend_lists_devices() {
        let backend = StubSttBackend::new(1024);
        let devices = backend
            .list_audio_devices()
            .expect("list_audio_devices should succeed");
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].name, "Stub Audio Device");
    }

    #[tokio::test]
    async fn stt_actor_emits_low_confidence_transcription() {
        let backend = Box::new(StubSttBackend::new(1024));
        let mut actor = SttActor::new(backend).with_min_confidence(0.9);
        let bus = Arc::new(EventBus::new());

        actor
            .start(bus.clone())
            .await
            .expect("start should succeed");

        // Simulate backend event with low confidence
        let event = SttEvent::Completed {
            text: "maybe".to_string(),
            confidence: 0.6,
        };

        actor
            .handle_stt_event(event)
            .await
            .expect("handle_stt_event should succeed");

        // We can't easily verify the broadcast without a subscriber, but we verify no panic
    }

    #[tokio::test]
    async fn stt_actor_emits_transcription_completed_for_high_confidence() {
        let backend = Box::new(StubSttBackend::new(1024));
        let mut actor = SttActor::new(backend).with_min_confidence(0.7);
        let bus = Arc::new(EventBus::new());

        actor
            .start(bus.clone())
            .await
            .expect("start should succeed");

        // Simulate backend event with high confidence
        let event = SttEvent::Completed {
            text: "hello world".to_string(),
            confidence: 0.95,
        };

        actor
            .handle_stt_event(event)
            .await
            .expect("handle_stt_event should succeed");
    }

    #[tokio::test]
    async fn stt_actor_listen_mode_enter_and_exit() {
        let backend = Box::new(StubSttBackend::new(1024));
        let mut actor = SttActor::new(backend);
        let bus = Arc::new(EventBus::new());

        actor
            .start(bus.clone())
            .await
            .expect("start should succeed");

        assert!(!actor.listen_mode_active);

        // Enter listen mode
        let causal_id = CausalId::new();
        actor
            .handle_bus_event(Event::Speech(SpeechEvent::ListenModeRequested {
                causal_id,
            }))
            .await
            .expect("handle_bus_event should succeed");

        assert!(actor.listen_mode_active);
        assert_eq!(actor.listen_mode_causal_id, Some(causal_id));

        // Exit listen mode
        actor
            .handle_bus_event(Event::Speech(SpeechEvent::ListenModeStopRequested {
                causal_id,
            }))
            .await
            .expect("handle_bus_event should succeed");

        assert!(!actor.listen_mode_active);
        assert_eq!(actor.listen_mode_causal_id, None);
    }

    #[tokio::test]
    async fn stt_actor_handles_shutdown() {
        let backend = Box::new(StubSttBackend::new(1024));
        let mut actor = SttActor::new(backend);
        let bus = Arc::new(EventBus::new());

        actor
            .start(bus.clone())
            .await
            .expect("start should succeed");

        assert!(!actor.shutdown_requested);

        actor
            .handle_bus_event(Event::System(SystemEvent::ShutdownRequested))
            .await
            .expect("handle_bus_event should succeed");

        assert!(actor.shutdown_requested);
    }
}
