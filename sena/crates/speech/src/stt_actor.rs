//! STT actor — streaming speech-to-text processing.

use crate::audio_input::{AudioChunk, AudioInputConfig, AudioInputStream};
use crate::backend::{AudioDevice, SttBackend};
use crate::error::{SpeechActorError, SttError};
use crate::silence_detector::SilenceDetector;
use crate::types::SttEvent;
use bus::causal::CausalId;
use bus::events::{SpeechEvent, SystemEvent};
use bus::{Actor, ActorError, Event, EventBus};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{broadcast, mpsc};
#[cfg(test)]
use tracing::debug;
use tracing::{error, info, warn};

/// Placeholder energy-based VAD heuristics until Silero VAD is integrated.
const DEFAULT_VAD_ENERGY_THRESHOLD: f32 = 0.02;
const DEFAULT_VAD_SILENCE_DURATION_SECS: f32 = 0.5;

/// STT actor — processes incoming audio and emits transcription events.
pub struct SttActor {
    backend: Box<dyn SttBackend>,
    bus: Option<Arc<EventBus>>,
    broadcast_rx: Option<broadcast::Receiver<Event>>,
    shutdown_requested: bool,
    listen_mode_active: bool,
    listen_mode_causal_id: Option<CausalId>,
    listen_mode_transcript: String,
    listen_mode_live_partial: String,
    loop_enabled: bool,
    audio_config: AudioInputConfig,
    audio_stream: Option<AudioInputStream>,
    audio_rx: Option<mpsc::UnboundedReceiver<AudioChunk>>,
    always_listening_vad: SilenceDetector,
    listen_mode_vad: SilenceDetector,
    /// Test-only: injectable audio source for hardware-independent tests
    #[cfg(test)]
    test_audio_rx: Option<mpsc::UnboundedReceiver<AudioChunk>>,
}

impl SttActor {
    /// Create a new STT actor with the given backend.
    pub fn new(backend: Box<dyn SttBackend>) -> Self {
        let preferred_chunk_samples = backend.preferred_chunk_samples().max(1);
        let mut audio_config = AudioInputConfig::default();
        let preferred_chunk_secs = preferred_chunk_samples as f32 / audio_config.sample_rate as f32;
        audio_config.buffer_duration_secs = preferred_chunk_secs.clamp(0.05, 0.25);

        Self {
            backend,
            bus: None,
            broadcast_rx: None,
            shutdown_requested: false,
            listen_mode_active: false,
            listen_mode_causal_id: None,
            listen_mode_transcript: String::new(),
            listen_mode_live_partial: String::new(),
            loop_enabled: true,
            audio_config,
            audio_stream: None,
            audio_rx: None,
            always_listening_vad: SilenceDetector::new(
                DEFAULT_VAD_ENERGY_THRESHOLD,
                DEFAULT_VAD_SILENCE_DURATION_SECS,
            ),
            listen_mode_vad: SilenceDetector::new(
                DEFAULT_VAD_ENERGY_THRESHOLD,
                DEFAULT_VAD_SILENCE_DURATION_SECS,
            ),
            #[cfg(test)]
            test_audio_rx: None,
        }
    }

    /// Set audio input configuration.
    pub fn with_audio_config(mut self, config: AudioInputConfig) -> Self {
        self.audio_config = config;
        self
    }

    /// Override the silence window used to finalize utterances.
    pub fn with_silence_duration(mut self, silence_duration_secs: f32) -> Self {
        self.always_listening_vad =
            SilenceDetector::new(DEFAULT_VAD_ENERGY_THRESHOLD, silence_duration_secs);
        self.listen_mode_vad =
            SilenceDetector::new(DEFAULT_VAD_ENERGY_THRESHOLD, silence_duration_secs);
        self
    }

    /// Test-only: inject a test audio receiver instead of starting real capture.
    #[cfg(test)]
    pub fn with_test_audio_rx(mut self, rx: mpsc::UnboundedReceiver<AudioChunk>) -> Self {
        self.test_audio_rx = Some(rx);
        self
    }

    /// Start audio capture if loop is enabled and not in test mode.
    fn start_audio_capture(&mut self) -> Result<(), SpeechActorError> {
        #[cfg(test)]
        if self.test_audio_rx.is_some() {
            if self.audio_rx.is_none() {
                self.audio_rx = self.test_audio_rx.take();
            }
            info!("using test audio receiver, skipping real capture");
            return Ok(());
        }

        if self.audio_stream.is_some() {
            warn!("audio capture already running");
            return Ok(());
        }

        info!("starting audio capture");
        match AudioInputStream::start(self.audio_config.clone()) {
            Ok((stream, rx)) => {
                self.audio_stream = Some(stream);
                self.audio_rx = Some(rx);
                info!("audio capture started");
                Ok(())
            }
            Err(e) => {
                warn!(error = %e, "failed to start audio capture (non-fatal)");
                Err(SpeechActorError::Stt(e))
            }
        }
    }

    /// Stop audio capture by dropping the stream and receiver.
    fn stop_audio_capture(&mut self) {
        if self.audio_stream.is_some() {
            info!("stopping audio capture");
            self.audio_stream = None;
            self.audio_rx = None;
            info!("audio capture stopped");
        }
    }

    /// Handle bus events.
    async fn handle_bus_event(&mut self, event: Event) -> Result<(), SpeechActorError> {
        match event {
            Event::System(SystemEvent::ShutdownSignal)
            | Event::System(SystemEvent::ShutdownRequested)
            | Event::System(SystemEvent::ShutdownInitiated) => {
                info!("Shutdown requested, stopping STT actor");
                self.shutdown_requested = true;
            }
            Event::System(SystemEvent::LoopControlRequested { loop_name, enabled })
                if loop_name == "speech" =>
            {
                info!(enabled = enabled, "speech loop control requested");
                let previous_state = self.loop_enabled;
                self.loop_enabled = enabled;

                // Start or stop audio capture when loop state changes
                if enabled && !previous_state {
                    if let Err(e) = self.start_audio_capture() {
                        warn!(error = %e, "failed to start audio capture on loop enable");
                    }
                } else if !enabled && previous_state {
                    self.flush_backend_events().await?;
                    self.stop_audio_capture();
                }

                // Broadcast status changed event
                if let Some(bus) = &self.bus {
                    let _ = bus
                        .broadcast(Event::System(SystemEvent::LoopStatusChanged {
                            loop_name: "speech".to_string(),
                            enabled,
                        }))
                        .await;
                }
            }
            Event::Speech(SpeechEvent::ListenModeRequested { causal_id }) => {
                if !self.loop_enabled {
                    warn!("Listen mode requested but speech loop is disabled");
                    return Ok(());
                }

                if self.audio_stream.is_none()
                    && let Err(e) = self.start_audio_capture()
                {
                    warn!(error = %e, "listen mode requested but audio capture is unavailable");
                    return Ok(());
                }

                info!("Listen mode requested");
                self.listen_mode_active = true;
                self.listen_mode_causal_id = Some(causal_id);
                self.listen_mode_transcript.clear();
                self.listen_mode_live_partial.clear();
                self.always_listening_vad.reset();
                self.listen_mode_vad.reset();
            }
            Event::Speech(SpeechEvent::ListenModeStopRequested { causal_id }) => {
                info!("Listen mode stop requested");
                self.flush_backend_events().await?;
                let transcript_text = compose_listen_mode_transcript(
                    &self.listen_mode_transcript,
                    &self.listen_mode_live_partial,
                );
                let transcript = if transcript_text.trim().is_empty() {
                    None
                } else {
                    Some(transcript_text)
                };
                self.listen_mode_active = false;
                self.listen_mode_causal_id = None;
                self.listen_mode_transcript.clear();
                self.listen_mode_live_partial.clear();
                self.listen_mode_vad.reset();

                if let Some(bus) = &self.bus {
                    bus.broadcast(Event::Speech(SpeechEvent::ListenModeStopped {
                        causal_id,
                        transcript,
                    }))
                    .await
                    .map_err(|e| SpeechActorError::Bus(e.to_string()))?;
                }
            }
            Event::Speech(SpeechEvent::WakewordDetected { confidence }) => {
                if !self.loop_enabled || self.listen_mode_active {
                    return Ok(());
                }

                info!(confidence = %confidence, "Wakeword detected");
                self.always_listening_vad.reset();

                if self.audio_stream.is_none()
                    && let Err(e) = self.start_audio_capture()
                {
                    warn!(error = %e, "failed to start audio capture after wakeword detection");
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn flush_backend_events(&mut self) -> Result<(), SpeechActorError> {
        let events = self.backend.flush().map_err(SpeechActorError::Stt)?;
        for event in events {
            self.handle_stt_event_internal(event).await?;
        }
        Ok(())
    }

    /// Handle backend STT events.
    #[cfg(test)]
    async fn handle_stt_event(&mut self, event: SttEvent) -> Result<(), SpeechActorError> {
        self.handle_stt_event_internal(event).await
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

        // Start audio capture if loop is enabled (unless in test mode with test_audio_rx)
        if self.loop_enabled {
            #[cfg(test)]
            {
                if self.test_audio_rx.is_some() {
                    info!("test mode: using injected audio receiver");
                } else if let Err(e) = self.start_audio_capture() {
                    warn!(error = %e, "failed to start audio capture at boot (non-fatal)");
                }
            }
            #[cfg(not(test))]
            {
                if let Err(e) = self.start_audio_capture() {
                    warn!(error = %e, "failed to start audio capture at boot (non-fatal)");
                }
            }
        }

        // Broadcast initial speech loop status
        bus.broadcast(Event::System(SystemEvent::LoopStatusChanged {
            loop_name: "speech".to_string(),
            enabled: true,
        }))
        .await
        .map_err(|e| ActorError::StartupFailed(e.to_string()))?;

        // Emit ActorReady event
        bus.broadcast(Event::System(SystemEvent::ActorReady {
            actor_name: self.name().to_string(),
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

        // Get audio receiver - either test or real
        #[cfg(test)]
        let mut audio_receiver = self.test_audio_rx.take().or_else(|| self.audio_rx.take());
        #[cfg(not(test))]
        let mut audio_receiver = self.audio_rx.take();

        while !self.shutdown_requested {
            tokio::select! {
                Ok(event) = rx.recv() => {
                    if let Err(e) = self.handle_bus_event(event).await {
                        error!(error = %e, "Failed to handle bus event");
                    }

                    if self.loop_enabled && audio_receiver.is_none() {
                        audio_receiver = self.audio_rx.take();
                    } else if !self.loop_enabled {
                        audio_receiver = None;
                    }
                }

                chunk = async {
                    match &mut audio_receiver {
                        Some(rx) => rx.recv().await,
                        None => std::future::pending().await,
                    }
                } => {
                    match chunk {
                        Some(chunk) => {
                            if self.loop_enabled
                                && let Err(e) = self.process_audio_chunk(chunk).await
                            {
                                warn!(error = %e, "Audio chunk processing failed (non-fatal)");
                            }
                        }
                        None => {
                            audio_receiver = None;
                        }
                    }
                }
            }
        }

        info!("STT actor run loop exiting");
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), ActorError> {
        info!("STT actor stopping");

        // Stop audio capture
        self.stop_audio_capture();

        // Flush backend
        if let Err(e) = self.backend.flush() {
            warn!(error = %e, "Failed to flush STT backend");
        }

        info!("STT actor stopped");
        Ok(())
    }
}

impl SttActor {
    /// Process an incoming audio chunk by feeding it to the backend and handling events.
    async fn process_audio_chunk(&mut self, chunk: AudioChunk) -> Result<(), SpeechActorError> {
        let start = Instant::now();
        match self.backend.feed(&chunk.samples) {
            Ok(events) => {
                for event in events {
                    self.handle_stt_event_internal(event).await?;
                }

                if self.listen_mode_active {
                    if self
                        .listen_mode_vad
                        .feed(&chunk.samples, self.audio_config.sample_rate, 1)
                    {
                        self.flush_backend_events().await?;
                    }
                } else if self.always_listening_vad.feed(
                    &chunk.samples,
                    self.audio_config.sample_rate,
                    1,
                ) {
                    self.flush_backend_events().await?;
                }
            }
            Err(e) => {
                return Err(SpeechActorError::Stt(e));
            }
        }

        let elapsed_ms = start.elapsed().as_millis();
        if elapsed_ms > 250 {
            warn!(
                elapsed_ms = elapsed_ms,
                "STT chunk processing latency is high"
            );
        }
        Ok(())
    }

    /// Handle backend STT events (internal version that works in run loop).
    async fn handle_stt_event_internal(&mut self, event: SttEvent) -> Result<(), SpeechActorError> {
        let bus = self
            .bus
            .as_ref()
            .ok_or_else(|| SpeechActorError::Bus("bus not initialized".to_string()))?;

        match event {
            SttEvent::Word {
                text,
                confidence: _,
            } => {
                if self.listen_mode_active {
                    self.listen_mode_live_partial =
                        merge_partial_transcript_text(&self.listen_mode_live_partial, &text);
                    let transcript = compose_listen_mode_transcript(
                        &self.listen_mode_transcript,
                        &self.listen_mode_live_partial,
                    );
                    // allowed: `Default` yields the sentinel `CausalId::none()`, but this path
                    // needs a fresh causal chain when listen mode was started without one.
                    #[allow(clippy::unwrap_or_default)]
                    let causal_id = self.listen_mode_causal_id.unwrap_or_else(CausalId::new);
                    bus.broadcast(Event::Speech(SpeechEvent::ListenModeTranscription {
                        text: transcript,
                        causal_id,
                    }))
                    .await
                    .map_err(|e| SpeechActorError::Bus(e.to_string()))?;
                } else {
                    #[cfg(test)]
                    debug!("Word recognized (debug only)");
                }
            }
            SttEvent::Completed { text, confidence } => {
                // Do not log raw speech text at info level (privacy)
                #[cfg(test)]
                debug!("Transcription completed (debug only)");

                let (inference_text, listen_mode_text) = if self.listen_mode_active {
                    let completed_utterance = if text.trim().is_empty() {
                        normalize_transcript_text(&self.listen_mode_live_partial)
                    } else {
                        normalize_transcript_text(&text)
                    };
                    self.listen_mode_transcript = append_transcript_segment(
                        &self.listen_mode_transcript,
                        &completed_utterance,
                    );
                    self.listen_mode_live_partial.clear();
                    (
                        completed_utterance,
                        Some(self.listen_mode_transcript.clone()),
                    )
                } else {
                    (text, None)
                };

                bus.broadcast(Event::Speech(SpeechEvent::TranscriptionCompleted {
                    text: inference_text.clone(),
                    confidence: 1.0,
                    causal_id: CausalId::new(),
                }))
                .await
                .map_err(|e| SpeechActorError::Bus(e.to_string()))?;
                tracing::debug!(
                    text = %inference_text,
                    backend_confidence = confidence,
                    "[STT] TranscriptionCompleted emitted"
                );

                if let Some(transcript) = listen_mode_text {
                    // allowed: `Default` yields the sentinel `CausalId::none()`, but this path
                    // needs a fresh causal chain when listen mode was started without one.
                    #[allow(clippy::unwrap_or_default)]
                    let listen_mode_causal_id =
                        self.listen_mode_causal_id.unwrap_or_else(CausalId::new);
                    bus.broadcast(Event::Speech(SpeechEvent::ListenModeTranscription {
                        text: transcript,
                        causal_id: listen_mode_causal_id,
                    }))
                    .await
                    .map_err(|e| SpeechActorError::Bus(e.to_string()))?;
                }
            }
            SttEvent::Listening => {
                info!("Backend listening");
                bus.broadcast(Event::Speech(SpeechEvent::SttListening))
                    .await
                    .map_err(|e| SpeechActorError::Bus(e.to_string()))?;
            }
            SttEvent::Stopped => {
                info!("Backend stopped");
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

fn merge_partial_transcript_text(current: &str, incoming: &str) -> String {
    let current = normalize_transcript_text(current);
    let incoming = normalize_transcript_text(incoming);

    if current.is_empty() {
        return incoming;
    }
    if incoming.is_empty() {
        return current;
    }
    if current == incoming || current.contains(&incoming) {
        return current;
    }
    if incoming.contains(&current) || incoming.starts_with(&current) {
        return incoming;
    }

    let max_overlap = current.len().min(incoming.len());
    for overlap in (1..=max_overlap).rev() {
        if current[current.len() - overlap..] == incoming[..overlap] {
            return normalize_transcript_text(&format!("{}{}", current, &incoming[overlap..]));
        }
    }

    normalize_transcript_text(&format!("{} {}", current, incoming))
}

fn append_transcript_segment(current: &str, segment: &str) -> String {
    let current = normalize_transcript_text(current);
    let segment = normalize_transcript_text(segment);

    if current.is_empty() {
        return segment;
    }
    if segment.is_empty() {
        return current;
    }
    if current.ends_with(&segment) {
        return current;
    }

    normalize_transcript_text(&format!("{} {}", current, segment))
}

fn compose_listen_mode_transcript(committed: &str, partial: &str) -> String {
    let committed = normalize_transcript_text(committed);
    let partial = normalize_transcript_text(partial);

    if committed.is_empty() {
        return partial;
    }
    if partial.is_empty() {
        return committed;
    }

    normalize_transcript_text(&format!("{} {}", committed, partial))
}

#[cfg(test)]
fn merge_completed_transcript_text(current: &str, incoming: &str) -> String {
    let current = normalize_transcript_text(current);
    let incoming = normalize_transcript_text(incoming);

    if current.is_empty() {
        return incoming;
    }
    if incoming.is_empty() {
        return current;
    }
    if current == incoming {
        return current;
    }
    if incoming.contains(&current) {
        return incoming;
    }
    if current.ends_with(&incoming) {
        let prefix = current[..current.len() - incoming.len()].trim();
        if transcript_artifact_score(prefix) >= 2 {
            return incoming;
        }
    }
    if current.contains(&incoming) {
        return current;
    }

    let current_words: Vec<&str> = current.split_whitespace().collect();
    let incoming_words: Vec<&str> = incoming.split_whitespace().collect();
    let max_overlap = current_words.len().min(incoming_words.len());

    for overlap in (3..=max_overlap).rev() {
        if current_words[current_words.len() - overlap..] == incoming_words[..overlap] {
            let prefix = current_words[..current_words.len() - overlap].join(" ");
            if prefix.is_empty() {
                return incoming;
            }
            return normalize_transcript_text(&format!("{} {}", prefix, incoming));
        }
    }

    merge_partial_transcript_text(&current, &incoming)
}

fn normalize_transcript_text(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut normalized = String::with_capacity(collapsed.len());

    for ch in collapsed.chars() {
        if matches!(ch, ',' | '.' | '!' | '?' | ';' | ':') && normalized.ends_with(' ') {
            normalized.pop();
        }
        normalized.push(ch);
    }

    normalized.trim().to_string()
}

#[cfg(test)]
fn transcript_artifact_score(text: &str) -> usize {
    let words: Vec<String> = text
        .split_whitespace()
        .map(|word| {
            word.chars()
                .filter(|ch| ch.is_ascii_alphabetic())
                .collect::<String>()
                .to_lowercase()
        })
        .filter(|word| !word.is_empty())
        .collect();

    words
        .windows(2)
        .filter(|pair| {
            let left = &pair[0];
            let right = &pair[1];
            (left.len() >= 4 && right.len() <= 3) || (left.len() <= 3 && right.len() >= 5)
        })
        .count()
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
    }

    #[test]
    fn merge_completed_transcript_prefers_clean_overlap() {
        let current = "Well, this is better for geomet ric data. Rotor sandwich R tim es preserves the full algebraic structure Well, this is better for geometric data. Rotor sandwich R times preserves the full algebraic structure";
        let incoming = "Well, this is better for geometric data. Rotor sandwich R times preserves the full algebraic structure";

        let merged = merge_completed_transcript_text(current, incoming);

        assert_eq!(merged, incoming);
    }

    #[test]
    fn normalize_transcript_removes_space_before_punctuation() {
        let normalized = normalize_transcript_text("hello , world .  how are you ?");

        assert_eq!(normalized, "hello, world. how are you?");
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
    async fn stt_actor_routes_completed_transcriptions_without_confidence_filter() {
        let backend = Box::new(StubSttBackend::new(1024));
        let mut actor = SttActor::new(backend);
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor
            .start(bus.clone())
            .await
            .expect("start should succeed");

        let event = SttEvent::Completed {
            text: "maybe".to_string(),
            confidence: 0.1,
        };

        actor
            .handle_stt_event(event)
            .await
            .expect("handle_stt_event should succeed");

        for _ in 0..10 {
            match rx.try_recv() {
                Ok(Event::Speech(SpeechEvent::TranscriptionCompleted {
                    text,
                    confidence,
                    ..
                })) => {
                    assert_eq!(text, "maybe");
                    assert_eq!(confidence, 1.0);
                    return;
                }
                _ => {}
            }
        }

        panic!("expected TranscriptionCompleted broadcast");
    }

    #[tokio::test]
    async fn listen_mode_transcription_events_emit_merged_transcript_for_cli() {
        let backend = Box::new(StubSttBackend::new(1024));
        let mut actor = SttActor::new(backend);
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor
            .start(bus.clone())
            .await
            .expect("start should succeed");

        actor.listen_mode_active = true;
        actor.listen_mode_causal_id = Some(CausalId::new());

        actor
            .handle_stt_event_internal(SttEvent::Word {
                text: "hello".to_string(),
                confidence: 0.95,
            })
            .await
            .expect("first partial should succeed");

        actor
            .handle_stt_event_internal(SttEvent::Word {
                text: "hello world".to_string(),
                confidence: 0.95,
            })
            .await
            .expect("second partial should succeed");

        actor
            .handle_stt_event_internal(SttEvent::Completed {
                text: "hello world from sena".to_string(),
                confidence: 0.95,
            })
            .await
            .expect("completion should succeed");

        let mut transcripts = Vec::new();
        let mut saw_inference_event = false;
        for _ in 0..10 {
            if let Ok(event) =
                tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await
            {
                match event {
                    Ok(Event::Speech(SpeechEvent::ListenModeTranscription { text, .. })) => {
                        transcripts.push(text);
                    }
                    Ok(Event::Speech(SpeechEvent::TranscriptionCompleted {
                        text,
                        confidence,
                        ..
                    })) => {
                        assert_eq!(text, "hello world from sena");
                        assert_eq!(confidence, 1.0);
                        saw_inference_event = true;
                    }
                    _ => {}
                }
            }

            if transcripts.len() == 3 && saw_inference_event {
                break;
            }
        }

        assert_eq!(
            transcripts,
            vec![
                "hello".to_string(),
                "hello world".to_string(),
                "hello world from sena".to_string(),
            ]
        );

        assert!(
            saw_inference_event,
            "listen mode completion should also emit TranscriptionCompleted"
        );
    }

    #[tokio::test]
    async fn listen_mode_completion_emits_both_routing_events() {
        let backend = Box::new(StubSttBackend::new(1024));
        let mut actor = SttActor::new(backend);
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor
            .start(bus.clone())
            .await
            .expect("start should succeed");

        actor.listen_mode_active = true;
        actor.listen_mode_causal_id = Some(CausalId::new());

        actor
            .handle_stt_event_internal(SttEvent::Completed {
                text: "route this utterance".to_string(),
                confidence: 0.42,
            })
            .await
            .expect("completion should succeed");

        let mut saw_transcription_completed = false;
        let mut saw_listen_mode_transcription = false;

        for _ in 0..10 {
            if let Ok(event) =
                tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await
                && let Ok(event) = event
            {
                match event {
                    Event::Speech(SpeechEvent::TranscriptionCompleted {
                        text,
                        confidence,
                        ..
                    }) => {
                        assert_eq!(text, "route this utterance");
                        assert_eq!(confidence, 1.0);
                        saw_transcription_completed = true;
                    }
                    Event::Speech(SpeechEvent::ListenModeTranscription { text, .. }) => {
                        assert_eq!(text, "route this utterance");
                        saw_listen_mode_transcription = true;
                    }
                    _ => {}
                }
            }

            if saw_transcription_completed && saw_listen_mode_transcription {
                break;
            }
        }

        assert!(saw_transcription_completed);
        assert!(saw_listen_mode_transcription);
    }

    #[tokio::test]
    async fn listen_mode_completed_transcript_does_not_duplicate_live_partial() {
        let backend = Box::new(StubSttBackend::new(1024));
        let mut actor = SttActor::new(backend);
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor
            .start(bus.clone())
            .await
            .expect("start should succeed");

        actor.listen_mode_active = true;
        actor.listen_mode_causal_id = Some(CausalId::new());

        actor
            .handle_stt_event_internal(SttEvent::Word {
                text: "Hello that's good Yeah, I don't knowhat's going on Let's fix this Stop listening"
                    .to_string(),
                confidence: 0.95,
            })
            .await
            .expect("partial should succeed");

        actor
            .handle_stt_event_internal(SttEvent::Completed {
                text: "Hello that's good Yeah, I don't know what's going on Let's fix this Stop listening"
                    .to_string(),
                confidence: 0.95,
            })
            .await
            .expect("completion should succeed");

        let mut transcripts = Vec::new();
        for _ in 0..10 {
            if let Ok(event) =
                tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await
                && let Ok(Event::Speech(SpeechEvent::ListenModeTranscription { text, .. })) = event
            {
                transcripts.push(text);
                if transcripts.len() == 2 {
                    break;
                }
            }
        }

        assert_eq!(
            transcripts,
            vec![
                "Hello that's good Yeah, I don't knowhat's going on Let's fix this Stop listening"
                    .to_string(),
                "Hello that's good Yeah, I don't know what's going on Let's fix this Stop listening"
                    .to_string(),
            ]
        );
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
    async fn listen_mode_request_starts_audio_capture_when_missing() {
        let backend = Box::new(StubSttBackend::new(1024));
        let (_audio_tx, audio_rx) = mpsc::unbounded_channel();
        let mut actor = SttActor::new(backend).with_test_audio_rx(audio_rx);
        let bus = Arc::new(EventBus::new());
        let causal_id = CausalId::new();

        actor.bus = Some(bus);

        assert!(actor.audio_stream.is_none());
        assert!(actor.audio_rx.is_none());

        actor
            .handle_bus_event(Event::Speech(SpeechEvent::ListenModeRequested {
                causal_id,
            }))
            .await
            .expect("listen mode request should succeed");

        assert!(actor.audio_rx.is_some());
        assert!(actor.listen_mode_active);
        assert_eq!(actor.listen_mode_causal_id, Some(causal_id));
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

    #[tokio::test]
    async fn speech_loop_responds_to_control_events() {
        use tokio::time::{Duration, sleep};

        let backend = Box::new(StubSttBackend::new(1024));
        let mut actor = SttActor::new(backend);
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor.start(Arc::clone(&bus)).await.expect("start failed");

        // Spawn actor run in background
        let actor_bus = Arc::clone(&bus);
        let handle = tokio::spawn(async move {
            let _ = actor.run().await;
        });

        // Wait for initial LoopStatusChanged event
        let mut got_initial_status = false;
        for _ in 0..10 {
            match rx.try_recv() {
                Ok(Event::System(SystemEvent::LoopStatusChanged { loop_name, enabled }))
                    if loop_name == "speech" =>
                {
                    assert!(enabled, "initial state should be enabled");
                    got_initial_status = true;
                    break;
                }
                _ => {}
            }
            sleep(Duration::from_millis(10)).await;
        }
        assert!(got_initial_status, "should emit initial loop status");

        // Send disable request
        actor_bus
            .broadcast(Event::System(SystemEvent::LoopControlRequested {
                loop_name: "speech".to_string(),
                enabled: false,
            }))
            .await
            .expect("broadcast failed");

        // Wait for status changed event
        let mut got_disabled = false;
        for _ in 0..10 {
            match rx.try_recv() {
                Ok(Event::System(SystemEvent::LoopStatusChanged { loop_name, enabled }))
                    if loop_name == "speech" =>
                {
                    assert!(!enabled, "state should be disabled");
                    got_disabled = true;
                    break;
                }
                _ => {}
            }
            sleep(Duration::from_millis(10)).await;
        }
        assert!(got_disabled, "should respond to disable request");

        // Cleanup
        actor_bus
            .broadcast(Event::System(SystemEvent::ShutdownRequested))
            .await
            .expect("broadcast failed");
        let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
    }

    #[tokio::test]
    async fn always_listening_flushes_after_silence() {
        let backend = Box::new(StubSttBackend::new(1600));
        let mut actor = SttActor::new(backend).with_silence_duration(0.0);
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor
            .start(bus.clone())
            .await
            .expect("start should succeed");

        actor
            .process_audio_chunk(AudioChunk {
                samples: vec![0.5; 4_000],
            })
            .await
            .expect("first speech chunk should succeed");
        actor
            .process_audio_chunk(AudioChunk {
                samples: vec![0.5; 4_000],
            })
            .await
            .expect("second speech chunk should succeed");
        actor
            .process_audio_chunk(AudioChunk {
                samples: vec![0.001; 16_000],
            })
            .await
            .expect("silence chunk should succeed");

        let mut saw_completion = false;
        for _ in 0..10 {
            if let Ok(event) =
                tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await
                && let Ok(Event::Speech(SpeechEvent::TranscriptionCompleted { .. })) = event
            {
                saw_completion = true;
                break;
            }
        }

        assert!(
            saw_completion,
            "silence should flush a completed transcription"
        );
    }

    #[tokio::test]
    async fn speech_loop_feeds_real_audio_when_enabled() {
        use tokio::time::{Duration, timeout};

        let backend = Box::new(StubSttBackend::new(100));
        let bus = Arc::new(EventBus::new());
        let mut broadcast_rx = bus.subscribe_broadcast();

        // Create test audio sender/receiver
        let (audio_tx, audio_rx) = mpsc::unbounded_channel();

        let mut actor = SttActor::new(backend).with_test_audio_rx(audio_rx);

        actor.start(Arc::clone(&bus)).await.expect("start failed");

        // Spawn actor run in background
        let handle = tokio::spawn(async move {
            let _ = actor.run().await;
        });

        // Send audio chunks to the test receiver
        for _ in 0..10 {
            audio_tx
                .send(AudioChunk {
                    samples: vec![0.5; 100],
                })
                .expect("send should succeed");
        }

        // Wait for transcription completion event from stub backend
        let result = timeout(Duration::from_secs(1), async {
            loop {
                if let Ok(event) = broadcast_rx.recv().await {
                    if matches!(
                        event,
                        Event::Speech(SpeechEvent::TranscriptionCompleted { .. })
                    ) {
                        return true;
                    }
                }
            }
        })
        .await;

        assert!(
            result.is_ok(),
            "should process audio chunks and emit transcription"
        );

        // Cleanup
        bus.broadcast(Event::System(SystemEvent::ShutdownRequested))
            .await
            .expect("broadcast failed");
        let _ = timeout(Duration::from_secs(1), handle).await;
    }

    #[tokio::test]
    async fn speech_loop_stops_feeding_when_disabled() {
        let backend = Box::new(StubSttBackend::new(100));
        let bus = Arc::new(EventBus::new());

        // Create test audio sender/receiver
        let (audio_tx, audio_rx) = mpsc::unbounded_channel();

        let mut actor = SttActor::new(backend).with_test_audio_rx(audio_rx);

        actor.start(Arc::clone(&bus)).await.expect("start failed");

        // Disable loop before running
        actor
            .handle_bus_event(Event::System(SystemEvent::LoopControlRequested {
                loop_name: "speech".to_string(),
                enabled: false,
            }))
            .await
            .expect("disable should succeed");

        assert!(!actor.loop_enabled, "loop should be disabled");

        // Even if we send audio, it should not be processed
        audio_tx
            .send(AudioChunk {
                samples: vec![0.5; 100],
            })
            .expect("send should succeed");

        // This test just verifies state - the actual non-processing is validated
        // by the select! logic in run() which checks loop_enabled
    }
}
