//! TTS actor — text-to-speech synthesis and playback.

use crate::backend::TtsBackend;
use crate::error::{SpeechActorError, TtsError};
use crate::types::AudioStream;
use bus::causal::CausalId;
use bus::events::SpeechEvent;
use bus::EventBus;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info};

/// Speak request message sent to TTS actor.
#[derive(Debug)]
pub struct SpeakRequest {
    pub text: String,
    pub causal_id: CausalId,
}

/// TTS actor — processes speak requests and emits speech output events.
pub struct TtsActor {
    backend: Box<dyn TtsBackend>,
    bus: Arc<EventBus>,
    speak_rx: mpsc::UnboundedReceiver<SpeakRequest>,
}

impl TtsActor {
    /// Create a new TTS actor with the given backend.
    pub fn new(
        backend: Box<dyn TtsBackend>,
        bus: Arc<EventBus>,
        speak_rx: mpsc::UnboundedReceiver<SpeakRequest>,
    ) -> Self {
        Self {
            backend,
            bus,
            speak_rx,
        }
    }

    /// Run the TTS processing loop.
    pub async fn run(mut self) -> Result<(), SpeechActorError> {
        info!(backend = self.backend.backend_name(), "TTS actor started");

        loop {
            tokio::select! {
                Some(request) = self.speak_rx.recv() => {
                    if let Err(e) = self.process_speak_request(request).await {
                        error!(error = %e, "Failed to process speak request");
                    }
                }
                else => {
                    info!("Speak request channel closed, shutting down TTS actor");
                    break;
                }
            }
        }

        info!("TTS actor stopped");
        Ok(())
    }

    /// Process a single speak request.
    async fn process_speak_request(
        &mut self,
        request: SpeakRequest,
    ) -> Result<(), SpeechActorError> {
        info!(
            text_len = request.text.len(),
            causal_id = ?request.causal_id,
            "Processing speak request"
        );

        // Synthesize speech
        match self.backend.synthesize(&request.text) {
            Ok(audio) => {
                debug!(
                    samples = audio.samples.len(),
                    sample_rate = audio.sample_rate,
                    duration_ms = audio.duration_ms(),
                    "Speech synthesized"
                );

                // Stub: immediately emit completion without actual playback
                self.bus
                    .broadcast(bus::Event::Speech(SpeechEvent::SpeechOutputCompleted {
                        causal_id: request.causal_id,
                    }))
                    .await
                    .map_err(|e| SpeechActorError::Bus(e.to_string()))?;

                info!(causal_id = ?request.causal_id, "Speech output completed");
            }
            Err(e) => {
                error!(error = %e, causal_id = ?request.causal_id, "Speech synthesis failed");
                self.bus
                    .broadcast(bus::Event::Speech(SpeechEvent::SpeechFailed {
                        reason: e.to_string(),
                        causal_id: request.causal_id,
                    }))
                    .await
                    .map_err(|e| SpeechActorError::Bus(e.to_string()))?;
            }
        }

        Ok(())
    }
}

/// Stub TTS backend for testing.
pub struct StubTtsBackend {
    sample_rate: u32,
}

impl StubTtsBackend {
    pub fn new(sample_rate: u32) -> Self {
        Self { sample_rate }
    }
}

impl TtsBackend for StubTtsBackend {
    fn synthesize(&mut self, text: &str) -> Result<AudioStream, TtsError> {
        // Stub: generate silent audio proportional to text length
        let samples_per_char = 100;
        let sample_count = text.len() * samples_per_char;
        let samples = vec![0.0; sample_count];

        Ok(AudioStream::new(samples, self.sample_rate))
    }

    fn cancel(&mut self) {
        // Stub: no-op
    }

    fn backend_name(&self) -> &'static str {
        "stub"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_backend_synthesizes_proportional_audio() {
        let mut backend = StubTtsBackend::new(16000);
        let audio = backend
            .synthesize("hello")
            .expect("synthesis should succeed");

        assert_eq!(audio.sample_rate, 16000);
        assert_eq!(audio.samples.len(), 500); // 5 chars * 100 samples/char
    }

    #[test]
    fn stub_backend_empty_text() {
        let mut backend = StubTtsBackend::new(16000);
        let audio = backend.synthesize("").expect("synthesis should succeed");

        assert_eq!(audio.samples.len(), 0);
        assert!(audio.is_empty());
    }

    #[test]
    fn stub_backend_name() {
        let backend = StubTtsBackend::new(16000);
        assert_eq!(backend.backend_name(), "stub");
    }
}
