//! TTS actor — text-to-speech synthesis and playback.

use crate::backend::TtsBackend;
use crate::error::SpeechActorError;
use crate::types::{AudioStream, PendingSentence};
use bus::causal::CausalId;
use bus::events::{InferenceEvent, SoulEvent, SpeechEvent, SystemEvent};
use bus::{Actor, ActorError, Event, EventBus};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

/// Default maximum TTS queue depth.
const DEFAULT_TTS_QUEUE_DEPTH: usize = 5;

/// Speak request message sent to TTS actor.
#[derive(Debug)]
pub struct SpeakRequest {
    pub text: String,
    pub causal_id: CausalId,
}

/// TTS actor — processes speak requests and emits speech output events.
pub struct TtsActor {
    backend: Box<dyn TtsBackend>,
    bus: Option<Arc<EventBus>>,
    broadcast_rx: Option<broadcast::Receiver<Event>>,
    /// Ordered queue of pending sentences keyed by sentence_index.
    pub(crate) queue: BTreeMap<u32, PendingSentence>,
    /// Queue depth limit.
    max_queue_depth: usize,
    /// Active synthesis tasks.
    active_tasks: Vec<JoinHandle<()>>,
    /// Whether actor is currently speaking.
    is_speaking: bool,
    /// Shutdown requested flag.
    shutdown_requested: bool,
    /// Next expected sentence index for playback.
    pub(crate) next_playback_index: u32,
}

impl TtsActor {
    /// Create a new TTS actor with the given backend.
    pub fn new(backend: Box<dyn TtsBackend>) -> Self {
        Self {
            backend,
            bus: None,
            broadcast_rx: None,
            queue: BTreeMap::new(),
            max_queue_depth: DEFAULT_TTS_QUEUE_DEPTH,
            active_tasks: Vec::new(),
            is_speaking: false,
            shutdown_requested: false,
            next_playback_index: 0,
        }
    }

    /// Set maximum queue depth.
    pub fn with_max_queue_depth(mut self, depth: usize) -> Self {
        self.max_queue_depth = depth;
        self
    }

    /// Handle bus events.
    async fn handle_bus_event(&mut self, event: Event) -> Result<(), SpeechActorError> {
        match event {
            Event::System(SystemEvent::ShutdownRequested) => {
                info!("Shutdown requested, stopping TTS actor");
                self.shutdown_requested = true;
            }
            Event::Inference(InferenceEvent::InferenceSentenceReady {
                text,
                sentence_index,
                causal_id,
            }) => {
                self.handle_sentence_ready(text, sentence_index, causal_id)
                    .await?;
            }
            Event::Speech(SpeechEvent::SpeakRequested { text, causal_id }) => {
                self.handle_speak_requested(text, causal_id).await?;
            }
            Event::Speech(SpeechEvent::TranscriptionCompleted { .. }) if self.is_speaking => {
                info!("Transcription completed while speaking — interrupting TTS");
                self.interrupt_all().await?;
            }
            Event::Soul(SoulEvent::PersonalityUpdated { .. }) => {
                debug!("Personality updated — prosody parameters would be updated here");
                // In a real implementation, update prosody parameters on the backend
            }
            _ => {}
        }
        Ok(())
    }

    /// Handle InferenceSentenceReady event.
    async fn handle_sentence_ready(
        &mut self,
        text: String,
        sentence_index: u32,
        causal_id: CausalId,
    ) -> Result<(), SpeechActorError> {
        debug!(
            sentence_index = %sentence_index,
            text_len = %text.len(),
            "Sentence ready for synthesis"
        );

        // Check queue depth before inserting
        while self.queue.len() >= self.max_queue_depth {
            // Drop oldest pending sentence
            if let Some((&oldest_index, _)) = self.queue.iter().next() {
                warn!(
                    dropped_index = %oldest_index,
                    "TTS queue full, dropping oldest pending sentence"
                );
                self.queue.remove(&oldest_index);
            } else {
                break;
            }
        }

        // Insert sentence into queue (not yet synthesized)
        // Synthesis happens on-demand in play_ready_sentences
        let pending = PendingSentence {
            text: text.clone(),
            sentence_index,
            audio: None, // Will be synthesized when played
            ready: true, // Ready for synthesis
        };
        self.queue.insert(sentence_index, pending);

        // Try to play sentences in order
        self.play_ready_sentences(causal_id).await?;

        Ok(())
    }

    /// Handle SpeakRequested event (FIFO path).
    async fn handle_speak_requested(
        &mut self,
        text: String,
        causal_id: CausalId,
    ) -> Result<(), SpeechActorError> {
        info!(
            text_len = text.len(),
            causal_id = ?causal_id,
            "Processing speak request"
        );

        let bus = self
            .bus
            .as_ref()
            .ok_or_else(|| SpeechActorError::Bus("bus not initialized".to_string()))?;

        // Emit speaking started
        bus.broadcast(Event::Speech(SpeechEvent::SpeakingStarted { causal_id }))
            .await
            .map_err(|e| SpeechActorError::Bus(e.to_string()))?;

        self.is_speaking = true;

        // Synthesize speech
        // NOTE: Backend should complete synthesis quickly or be async internally
        match self.backend.synthesize(&text) {
            Ok(audio) => {
                debug!(
                    samples = audio.samples.len(),
                    sample_rate = audio.sample_rate,
                    duration_ms = audio.duration_ms(),
                    "Speech synthesized"
                );

                // Play the audio in spawn_blocking (contains thread::sleep)
                let audio_clone = audio.clone();
                match tokio::task::spawn_blocking(move || Self::play_audio_blocking(&audio_clone))
                    .await
                {
                    Ok(Ok(())) => {
                        bus.broadcast(Event::Speech(SpeechEvent::SpeakingCompleted { causal_id }))
                            .await
                            .map_err(|e| SpeechActorError::Bus(e.to_string()))?;

                        self.is_speaking = false;
                        info!(causal_id = ?causal_id, "Speech output completed");
                    }
                    Ok(Err(e)) => {
                        error!(error = %e, causal_id = ?causal_id, "Audio playback failed");
                        bus.broadcast(Event::Speech(SpeechEvent::SpeechFailed {
                            reason: format!("Playback failed: {}", e),
                            causal_id,
                        }))
                        .await
                        .map_err(|e| SpeechActorError::Bus(e.to_string()))?;
                        self.is_speaking = false;
                    }
                    Err(e) => {
                        error!(error = %e, causal_id = ?causal_id, "Playback task panicked");
                        bus.broadcast(Event::Speech(SpeechEvent::SpeechFailed {
                            reason: format!("Playback panicked: {}", e),
                            causal_id,
                        }))
                        .await
                        .map_err(|e| SpeechActorError::Bus(e.to_string()))?;
                        self.is_speaking = false;
                    }
                }
            }
            Err(e) => {
                error!(error = %e, causal_id = ?causal_id, "Speech synthesis failed");
                bus.broadcast(Event::Speech(SpeechEvent::SpeechFailed {
                    reason: e.to_string(),
                    causal_id,
                }))
                .await
                .map_err(|e| SpeechActorError::Bus(e.to_string()))?;
                self.is_speaking = false;
            }
        }

        Ok(())
    }

    /// Play ready sentences in index order.
    async fn play_ready_sentences(&mut self, causal_id: CausalId) -> Result<(), SpeechActorError> {
        let bus = self
            .bus
            .as_ref()
            .ok_or_else(|| SpeechActorError::Bus("bus not initialized".to_string()))?;

        // Find all ready sentences starting from next_playback_index
        let mut indices_to_play = Vec::new();

        // Collect indices in a loop, checking each expected index in sequence
        while let Some(sentence) = self.queue.get(&self.next_playback_index) {
            if sentence.ready {
                indices_to_play.push(self.next_playback_index);
                self.next_playback_index += 1;
            } else {
                // Next sentence in sequence is not ready yet, stop
                break;
            }
        }

        // Play each ready sentence in order
        for index in indices_to_play {
            if let Some(sentence) = self.queue.remove(&index) {
                debug!(sentence_index = %index, "Playing sentence");

                if !self.is_speaking {
                    bus.broadcast(Event::Speech(SpeechEvent::SpeakingStarted { causal_id }))
                        .await
                        .map_err(|e| SpeechActorError::Bus(e.to_string()))?;
                    self.is_speaking = true;
                }

                // Synthesize sentence audio
                // NOTE: Backend should complete synthesis quickly or be async internally
                match self.backend.synthesize(&sentence.text) {
                    Ok(audio) => {
                        // Play the audio in spawn_blocking (contains thread::sleep)
                        let audio_clone = audio.clone();
                        match tokio::task::spawn_blocking(move || {
                            Self::play_audio_blocking(&audio_clone)
                        })
                        .await
                        {
                            Ok(Ok(())) => {
                                debug!(sentence_index = %index, "Sentence playback complete");
                            }
                            Ok(Err(e)) => {
                                error!(error = %e, sentence_index = %index, "Audio playback failed");
                                bus.broadcast(Event::Speech(SpeechEvent::SpeechFailed {
                                    reason: format!("Playback failed: {}", e),
                                    causal_id,
                                }))
                                .await
                                .map_err(|e| SpeechActorError::Bus(e.to_string()))?;
                                self.is_speaking = false;
                                return Ok(());
                            }
                            Err(e) => {
                                error!(error = %e, sentence_index = %index, "Playback task panicked");
                                bus.broadcast(Event::Speech(SpeechEvent::SpeechFailed {
                                    reason: format!("Playback panicked: {}", e),
                                    causal_id,
                                }))
                                .await
                                .map_err(|e| SpeechActorError::Bus(e.to_string()))?;
                                self.is_speaking = false;
                                return Ok(());
                            }
                        }
                    }
                    Err(e) => {
                        error!(error = %e, sentence_index = %index, "Synthesis failed");
                        bus.broadcast(Event::Speech(SpeechEvent::SpeechFailed {
                            reason: e.to_string(),
                            causal_id,
                        }))
                        .await
                        .map_err(|e| SpeechActorError::Bus(e.to_string()))?;
                        self.is_speaking = false;
                        return Ok(());
                    }
                }
            }
        }

        // If queue is empty and we were speaking, emit completion
        if self.queue.is_empty() && self.is_speaking {
            bus.broadcast(Event::Speech(SpeechEvent::SpeakingCompleted { causal_id }))
                .await
                .map_err(|e| SpeechActorError::Bus(e.to_string()))?;
            self.is_speaking = false;
        }

        Ok(())
    }

    /// Play audio using cpal output device (blocking — must be called from spawn_blocking).
    fn play_audio_blocking(audio: &AudioStream) -> Result<(), SpeechActorError> {
        if audio.is_empty() {
            return Ok(());
        }

        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| SpeechActorError::AudioDevice("No default output device".to_string()))?;

        let supported_config = device.default_output_config().map_err(|e| {
            SpeechActorError::AudioDevice(format!("Failed to get default output config: {}", e))
        })?;

        let config = cpal::StreamConfig {
            channels: 1,
            sample_rate: supported_config.sample_rate(),
            buffer_size: cpal::BufferSize::Default,
        };

        let samples = audio.samples.clone();
        let mut sample_index = 0;
        let sample_count = samples.len();

        let stream = device
            .build_output_stream(
                &config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    for sample in data.iter_mut() {
                        *sample = if sample_index < sample_count {
                            let s = samples[sample_index];
                            sample_index += 1;
                            s
                        } else {
                            0.0
                        };
                    }
                },
                |err| error!("Audio output stream error: {}", err),
                None,
            )
            .map_err(|e| {
                SpeechActorError::AudioDevice(format!("Failed to build output stream: {}", e))
            })?;

        stream.play().map_err(|e| {
            SpeechActorError::AudioDevice(format!("Failed to start output stream: {}", e))
        })?;

        // Block until playback completes
        let duration = std::time::Duration::from_millis(audio.duration_ms() + 100);
        std::thread::sleep(duration);

        Ok(())
    }

    /// Interrupt all pending synthesis and playback.
    async fn interrupt_all(&mut self) -> Result<(), SpeechActorError> {
        info!("Interrupting all TTS tasks");

        // Cancel backend
        self.backend.cancel();

        // Flush audio buffer
        self.backend.flush_buffer();

        // Cancel active tasks
        for handle in self.active_tasks.drain(..) {
            handle.abort();
        }

        // Clear queue
        self.queue.clear();
        self.next_playback_index = 0;

        // Emit interrupted event if we were speaking
        if self.is_speaking {
            let bus = self
                .bus
                .as_ref()
                .ok_or_else(|| SpeechActorError::Bus("bus not initialized".to_string()))?;

            bus.broadcast(Event::Speech(SpeechEvent::SpeakingInterrupted {
                causal_id: CausalId::new(),
            }))
            .await
            .map_err(|e| SpeechActorError::Bus(e.to_string()))?;

            self.is_speaking = false;
        }

        Ok(())
    }
}

impl Actor for TtsActor {
    fn name(&self) -> &'static str {
        "tts"
    }

    async fn start(&mut self, bus: Arc<EventBus>) -> Result<(), ActorError> {
        info!("TTS actor starting");
        self.bus = Some(bus.clone());
        self.broadcast_rx = Some(bus.subscribe_broadcast());

        // Emit ActorReady event
        bus.broadcast(Event::System(SystemEvent::ActorReady {
            actor_name: self.name(),
        }))
        .await
        .map_err(|e| ActorError::StartupFailed(e.to_string()))?;

        info!("TTS actor started");
        Ok(())
    }

    async fn run(&mut self) -> Result<(), ActorError> {
        let mut rx = self.broadcast_rx.take().ok_or_else(|| {
            ActorError::RuntimeError("broadcast receiver not initialized".to_string())
        })?;

        info!(backend = self.backend.backend_name(), "TTS actor running");

        while !self.shutdown_requested {
            tokio::select! {
                Ok(event) = rx.recv() => {
                    if let Err(e) = self.handle_bus_event(event).await {
                        error!(error = %e, "Failed to handle bus event");
                    }
                }
            }
        }

        info!("TTS actor run loop exiting");
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), ActorError> {
        info!("TTS actor stopping");

        // Cancel all pending tasks
        if let Err(e) = self.interrupt_all().await {
            warn!(error = %e, "Failed to interrupt TTS tasks during shutdown");
        }

        info!("TTS actor stopped");
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
    fn synthesize(&mut self, text: &str) -> Result<AudioStream, crate::error::TtsError> {
        // Stub: generate silent audio proportional to text length
        let samples_per_char = 100;
        let sample_count = text.len() * samples_per_char;
        let samples = vec![0.0; sample_count];

        Ok(AudioStream::new(samples, self.sample_rate))
    }

    fn cancel(&mut self) {
        // Stub: no-op
    }

    fn flush_buffer(&mut self) {
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

    #[tokio::test]
    async fn tts_actor_queue_ordering() {
        let backend = Box::new(StubTtsBackend::new(16000));
        let mut actor = TtsActor::new(backend);
        let bus = Arc::new(EventBus::new());

        actor
            .start(bus.clone())
            .await
            .expect("start should succeed");

        // Add sentence 0 first for simplicity
        let causal_id = CausalId::new();
        actor
            .handle_sentence_ready("Sentence 0".to_string(), 0, causal_id)
            .await
            .expect("handle_sentence_ready should succeed");

        assert_eq!(actor.next_playback_index, 1, "After adding sentence 0");
        assert_eq!(
            actor.queue.len(),
            0,
            "Sentence 0 should have been played and removed"
        );

        // Add sentence 1
        actor
            .handle_sentence_ready("Sentence 1".to_string(), 1, causal_id)
            .await
            .expect("handle_sentence_ready should succeed");

        assert_eq!(actor.next_playback_index, 2, "After adding sentence 1");
        assert_eq!(
            actor.queue.len(),
            0,
            "Sentence 1 should have been played and removed"
        );

        // Add sentence 2
        actor
            .handle_sentence_ready("Sentence 2".to_string(), 2, causal_id)
            .await
            .expect("handle_sentence_ready should succeed");

        // Verify next_playback_index advanced correctly
        assert_eq!(actor.next_playback_index, 3);
        assert!(actor.queue.is_empty());
    }

    #[tokio::test]
    async fn tts_actor_interruption_clears_queue() {
        let backend = Box::new(StubTtsBackend::new(16000));
        let mut actor = TtsActor::new(backend);
        let bus = Arc::new(EventBus::new());

        actor
            .start(bus.clone())
            .await
            .expect("start should succeed");

        // Add pending sentences
        let causal_id = CausalId::new();
        actor
            .handle_sentence_ready("Sentence 0".to_string(), 0, causal_id)
            .await
            .expect("handle_sentence_ready should succeed");

        actor.queue.insert(
            1,
            PendingSentence {
                text: "Sentence 1".to_string(),
                sentence_index: 1,
                audio: None,
                ready: false,
            },
        );
        actor.queue.insert(
            2,
            PendingSentence {
                text: "Sentence 2".to_string(),
                sentence_index: 2,
                audio: None,
                ready: false,
            },
        );
        actor.is_speaking = true;

        assert_eq!(actor.queue.len(), 2);

        // Interrupt
        actor
            .interrupt_all()
            .await
            .expect("interrupt_all should succeed");

        assert!(actor.queue.is_empty());
        assert_eq!(actor.next_playback_index, 0);
        assert!(!actor.is_speaking);
    }

    #[tokio::test]
    async fn tts_actor_queue_depth_limit() {
        let backend = Box::new(StubTtsBackend::new(16000));
        let mut actor = TtsActor::new(backend).with_max_queue_depth(3);
        let bus = Arc::new(EventBus::new());

        actor
            .start(bus.clone())
            .await
            .expect("start should succeed");

        let causal_id = CausalId::new();

        // Add sentences, blocking playback by making them not ready
        for i in 10..15 {
            actor.queue.insert(
                i,
                PendingSentence {
                    text: format!("Sentence {}", i),
                    sentence_index: i,
                    audio: None,
                    ready: false,
                },
            );
        }

        assert_eq!(actor.queue.len(), 5);

        // Now add a new sentence — should drop oldest
        actor
            .handle_sentence_ready("New sentence".to_string(), 20, causal_id)
            .await
            .expect("handle_sentence_ready should succeed");

        // Queue should be capped at max_queue_depth
        assert!(actor.queue.len() <= actor.max_queue_depth);
    }

    #[tokio::test]
    async fn tts_actor_speak_requested_handling() {
        let backend = Box::new(StubTtsBackend::new(16000));
        let mut actor = TtsActor::new(backend);
        let bus = Arc::new(EventBus::new());

        actor
            .start(bus.clone())
            .await
            .expect("start should succeed");

        let causal_id = CausalId::new();
        actor
            .handle_speak_requested("Hello world".to_string(), causal_id)
            .await
            .expect("handle_speak_requested should succeed");

        // Verify speaking completed
        assert!(!actor.is_speaking);
    }

    #[tokio::test]
    async fn tts_actor_handles_shutdown() {
        let backend = Box::new(StubTtsBackend::new(16000));
        let mut actor = TtsActor::new(backend);
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

    #[tokio::test]
    async fn tts_actor_transcription_during_speech_interrupts() {
        let backend = Box::new(StubTtsBackend::new(16000));
        let mut actor = TtsActor::new(backend);
        let bus = Arc::new(EventBus::new());

        actor
            .start(bus.clone())
            .await
            .expect("start should succeed");

        // Set speaking state
        actor.is_speaking = true;
        actor.queue.insert(
            0,
            PendingSentence {
                text: "Test".to_string(),
                sentence_index: 0,
                audio: Some(vec![0u8; 100]),
                ready: true,
            },
        );

        assert!(!actor.queue.is_empty());

        // Simulate transcription completed event
        let causal_id = CausalId::new();
        actor
            .handle_bus_event(Event::Speech(SpeechEvent::TranscriptionCompleted {
                text: "User spoke".to_string(),
                confidence: 0.9,
                causal_id,
            }))
            .await
            .expect("handle_bus_event should succeed");

        // Queue should be cleared
        assert!(actor.queue.is_empty());
        assert!(!actor.is_speaking);
    }
}
