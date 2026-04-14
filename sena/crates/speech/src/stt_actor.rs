//! STT actor — streaming speech-to-text processing.

use crate::backend::SttBackend;
use crate::error::{SpeechActorError, SttError};
use crate::types::SttEvent;
use bus::causal::CausalId;
use bus::events::SpeechEvent;
use bus::EventBus;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// Audio chunk message sent to STT actor.
#[derive(Debug)]
pub struct AudioChunk {
    /// PCM samples (f32, mono).
    pub samples: Vec<f32>,
}

/// STT actor — processes incoming audio and emits transcription events.
pub struct SttActor {
    backend: Box<dyn SttBackend>,
    bus: Arc<EventBus>,
    audio_rx: mpsc::UnboundedReceiver<AudioChunk>,
    silence_threshold: f32,
}

impl SttActor {
    /// Create a new STT actor with the given backend.
    pub fn new(
        backend: Box<dyn SttBackend>,
        bus: Arc<EventBus>,
        audio_rx: mpsc::UnboundedReceiver<AudioChunk>,
    ) -> Self {
        Self {
            backend,
            bus,
            audio_rx,
            silence_threshold: 0.01, // Stub threshold
        }
    }

    /// Run the STT processing loop.
    pub async fn run(mut self) -> Result<(), SpeechActorError> {
        info!(backend = self.backend.backend_name(), "STT actor started");

        loop {
            tokio::select! {
                Some(chunk) = self.audio_rx.recv() => {
                    if let Err(e) = self.process_chunk(&chunk).await {
                        error!(error = %e, "Failed to process audio chunk");
                    }
                }
                else => {
                    info!("Audio channel closed, shutting down STT actor");
                    break;
                }
            }
        }

        info!("STT actor stopped");
        Ok(())
    }

    /// Process a single audio chunk.
    async fn process_chunk(&mut self, chunk: &AudioChunk) -> Result<(), SpeechActorError> {
        debug!(samples = chunk.samples.len(), "Processing audio chunk");

        // Stub: Check for silence using simple threshold
        let is_silent = chunk
            .samples
            .iter()
            .all(|&s| s.abs() < self.silence_threshold);

        if is_silent {
            debug!("Detected silence, skipping chunk");
            return Ok(());
        }

        // Feed to backend
        let events = self.backend.feed(&chunk.samples).map_err(|e| {
            error!(error = %e, "Backend feed failed");
            SpeechActorError::Stt(e)
        })?;

        // Process backend events
        for event in events {
            self.handle_backend_event(event).await?;
        }

        Ok(())
    }

    /// Handle events emitted by the backend.
    async fn handle_backend_event(&self, event: SttEvent) -> Result<(), SpeechActorError> {
        match event {
            SttEvent::Word { text, confidence } => {
                debug!(text = %text, confidence = %confidence, "Word recognized");
            }
            SttEvent::Completed { text, confidence } => {
                info!(text = %text, confidence = %confidence, "Transcription completed");
                let causal_id = CausalId::new();
                self.bus
                    .broadcast(bus::Event::Speech(SpeechEvent::TranscriptionCompleted {
                        text,
                        confidence,
                        causal_id,
                    }))
                    .await
                    .map_err(|e| SpeechActorError::Bus(e.to_string()))?;
            }
            SttEvent::Listening => {
                debug!("Backend listening");
            }
            SttEvent::Stopped => {
                debug!("Backend stopped");
            }
            SttEvent::Error { reason } => {
                warn!(reason = %reason, "Backend reported error");
                let causal_id = CausalId::new();
                self.bus
                    .broadcast(bus::Event::Speech(SpeechEvent::TranscriptionFailed {
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
}
