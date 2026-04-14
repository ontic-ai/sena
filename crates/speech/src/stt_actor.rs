//! STT Actor - speech-to-text processing.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::{broadcast, mpsc};

use bus::{Actor, ActorError, Event, EventBus, SpeechEvent, SystemEvent};

use crate::audio_input::{AudioInputConfig, AudioInputStream};
use crate::stt::backend_trait::{SttBackend, SttEvent};
use crate::stt::factory::build_stt_backend;
use crate::{AudioBuffer, SpeechError, SttBackendKind};

const TRANSCRIPTION_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_BUFFER_DURATION_SECS: f32 = 3.0;
/// Fallback listen-mode buffer duration when the backend is not yet initialized.
const LISTEN_DEFAULT_BUFFER_DURATION_SECS: f32 = 1.0;
/// Silence threshold for listen-mode VAD. Higher than always-on (1.5s) to prevent
/// mid-sentence flushing during normal speech pauses.
const LISTEN_MODE_SILENCE_SECS: f32 = 2.5;
/// Gap between the last token and the next token that classifies as a new sentence.
/// Must be ≤ LISTEN_MODE_SILENCE_SECS so the VAD doesn't fire first.
const SENTENCE_BOUNDARY_SECS: f32 = 2.0;

/// STT Actor - handles speech-to-text transcription.
///
/// Pipeline:
/// 1. On start, build the STT backend via `build_stt_backend()` (model load).
/// 2. If always-listening is enabled, capture mic audio and buffer speech chunks.
/// 3. Also respond to `SpeechEvent::VoiceInputDetected` (on-demand mode).
/// 4. Audio is passed to the backend via `feed()` + `flush()` through `spawn_blocking`.
/// 5. Silence detection: accumulate audio during speech, transcribe after silence.
pub struct SttActor {
    backend_kind: SttBackendKind,
    backend: Option<Arc<Mutex<Box<dyn SttBackend>>>>,
    bus: Option<Arc<EventBus>>,
    bus_rx: Option<broadcast::Receiver<Event>>,
    audio_rx: Option<mpsc::UnboundedReceiver<AudioBuffer>>,
    audio_stream: Option<AudioInputStream>,
    request_id_counter: u64,
    voice_always_listening: bool,
    stt_energy_threshold: f32,
    /// Minimum confidence score for transcription output to be accepted.
    /// Range [0.0, 1.0]. Default: 0.5.
    confidence_threshold: f32,
    whisper_model_path: Option<String>,
    model_dir: Option<PathBuf>,
    silence_duration_secs: f32,
    // Silence detection state (separate instances prevent cross-contamination)
    always_listening_vad: crate::silence_detector::SilenceDetector,
    listen_mode_vad: crate::silence_detector::SilenceDetector,
    // Listen mode state (continuous transcription session)
    listen_session_id: Option<u64>,
    listen_audio_rx: Option<mpsc::UnboundedReceiver<AudioBuffer>>,
    listen_audio_stream: Option<AudioInputStream>,
    /// True while a /listen session is active. Suppresses always-on STT processing
    /// to prevent conflicting TranscriptionCompleted events during listen mode.
    listen_mode_active: bool,
    /// Preferred microphone device name (None = system default).
    microphone_device: Option<String>,
    /// Whether the speech loop is enabled (pause/resume via LoopControlRequested).
    speech_loop_enabled: bool,
    /// True while a backend switch is in progress. Suppresses audio processing.
    backend_switching: bool,
    /// True while wakeword detection is suppressed (e.g., during TTS playback).
    /// Audio processing is skipped when this flag is set.
    wakeword_suppressed: bool,
    /// Timestamp of the last non-empty partial token received in listen mode.
    /// Used to detect sentence boundaries: gap ≥ SENTENCE_BOUNDARY_SECS → is_new_sentence=true.
    listen_last_partial_instant: Option<Instant>,
}

impl SttActor {
    /// Create a new STT actor with backend kind and runtime config values.
    pub fn new(
        backend: SttBackendKind,
        voice_always_listening: bool,
        stt_energy_threshold: f32,
        whisper_model_path: Option<String>,
    ) -> Self {
        Self {
            backend_kind: backend,
            backend: None,
            bus: None,
            bus_rx: None,
            audio_rx: None,
            audio_stream: None,
            request_id_counter: 0,
            voice_always_listening,
            stt_energy_threshold,
            confidence_threshold: 0.5,
            whisper_model_path,
            model_dir: None,
            silence_duration_secs: 1.5,
            always_listening_vad: crate::silence_detector::SilenceDetector::new(
                stt_energy_threshold,
                1.5,
            ),
            listen_mode_vad: crate::silence_detector::SilenceDetector::new(
                stt_energy_threshold,
                LISTEN_MODE_SILENCE_SECS,
            ),
            listen_session_id: None,
            listen_audio_rx: None,
            listen_audio_stream: None,
            listen_mode_active: false,
            microphone_device: None,
            speech_loop_enabled: true,
            backend_switching: false,
            wakeword_suppressed: false,
            listen_last_partial_instant: None,
        }
    }

    /// Set the model directory path (where downloaded models are stored).
    pub fn with_model_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.model_dir = dir;
        self
    }

    /// Set the silence duration threshold in seconds for always-on mode.
    /// listen-mode always uses LISTEN_MODE_SILENCE_SECS to avoid mid-sentence VAD fires.
    pub fn with_silence_duration(mut self, secs: f32) -> Self {
        self.silence_duration_secs = secs;
        self.always_listening_vad =
            crate::silence_detector::SilenceDetector::new(self.stt_energy_threshold, secs);
        // listen_mode_vad intentionally NOT updated — it always uses LISTEN_MODE_SILENCE_SECS.
        self
    }

    /// Set the preferred microphone device name.
    /// A case-insensitive substring match is used so partial names work.
    pub fn with_microphone_device(mut self, device: Option<String>) -> Self {
        self.microphone_device = device;
        self
    }

    /// Set the minimum confidence threshold for accepted transcriptions.
    /// Transcriptions below this threshold are treated as low-confidence failures.
    pub fn with_confidence_threshold(mut self, threshold: f32) -> Self {
        self.confidence_threshold = threshold.clamp(0.0, 1.0);
        self
    }

    fn next_request_id(&mut self) -> u64 {
        let id = self.request_id_counter;
        self.request_id_counter = self.request_id_counter.saturating_add(1);
        id
    }

    /// Build and install the backend for the current `backend_kind`.
    async fn initialize_backend(&mut self) -> Result<(), SpeechError> {
        let backend = build_stt_backend(
            self.backend_kind,
            self.model_dir.as_deref(),
            self.whisper_model_path.as_deref(),
            self.stt_energy_threshold,
        )
        .await?;
        self.backend = Some(Arc::new(Mutex::new(backend)));
        Ok(())
    }

    fn maybe_start_audio_capture(&mut self) -> Result<(), SpeechError> {
        if !self.voice_always_listening {
            return Ok(());
        }

        let config = AudioInputConfig {
            sample_rate: 16_000,
            buffer_duration_secs: DEFAULT_BUFFER_DURATION_SECS,
            // Pass all audio to SilenceDetector - it handles voice/silence classification.
            // Setting energy_threshold > 0 here would drop silence frames and prevent
            // the SilenceDetector from ever detecting the speech?silence transition.
            energy_threshold: 0.0,
            device_name: self.microphone_device.clone(),
        };

        let (stream, rx) = AudioInputStream::start(config)?;
        self.audio_stream = Some(stream);
        self.audio_rx = Some(rx);
        Ok(())
    }

    /// Execute a one-shot transcription of `samples` and broadcast the result.
    ///
    /// Calls `backend.feed(samples)` then `backend.flush()` inside `spawn_blocking`.
    /// The first `SttEvent::Completed` in the combined output drives the bus event.
    async fn execute_transcription(
        &self,
        samples: Vec<f32>,
        request_id: u64,
        bus: &Arc<EventBus>,
    ) {
        let backend = match self.backend.as_ref() {
            Some(b) => Arc::clone(b),
            None => {
                let _ = bus
                    .broadcast(Event::Speech(SpeechEvent::TranscriptionFailed {
                        reason: "STT backend not initialized".to_string(),
                        request_id,
                    }))
                    .await;
                return;
            }
        };

        let chunk_duration_ms =
            (samples.len() as f64 / 16_000.0 * 1000.0) as u64;
        let start = std::time::Instant::now();

        let result = tokio::time::timeout(
            TRANSCRIPTION_TIMEOUT,
            tokio::task::spawn_blocking(move || {
                let mut b = backend.lock().map_err(|_| {
                    SpeechError::ChannelClosed("backend lock poisoned".to_string())
                })?;
                let name = b.backend_name();
                let vram = b.vram_mb();
                let mut events = b.feed(&samples)?;
                events.extend(b.flush()?);
                Ok::<_, SpeechError>((name, vram, events))
            }),
        )
        .await;

        let latency_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(Ok(Ok((backend_name, vram_mb, events)))) => {
                let completed = events.into_iter().find_map(|e| {
                    if let SttEvent::Completed { text, confidence } = e {
                        Some((text, confidence))
                    } else {
                        None
                    }
                });

                if let Some((text, confidence)) = completed {
                    if let Err(e) = crate::telemetry::log_stt_telemetry(
                        backend_name,
                        chunk_duration_ms,
                        latency_ms,
                        confidence as f64,
                        vram_mb as u32,
                    )
                    .await
                    {
                        tracing::warn!("telemetry write failed: {}", e);
                    }

                    let _ = bus
                        .broadcast(Event::Speech(SpeechEvent::SttTelemetryUpdate {
                            backend: backend_name.to_string(),
                            vram_mb: Some(vram_mb as f64),
                            latency_ms: latency_ms as f64,
                            avg_confidence: confidence as f64,
                            request_id,
                        }))
                        .await;

                    if confidence >= self.confidence_threshold {
                        let _ = bus
                            .broadcast(Event::Speech(SpeechEvent::TranscriptionCompleted {
                                text,
                                confidence,
                                request_id,
                            }))
                            .await;
                    } else {
                        let _ = bus
                            .broadcast(Event::Speech(SpeechEvent::LowConfidenceTranscription {
                                confidence,
                                request_id,
                            }))
                            .await;
                    }
                }
                // No Completed event = silence / empty audio — nothing to broadcast.
            }
            Ok(Ok(Err(e))) => {
                let _ = bus
                    .broadcast(Event::Speech(SpeechEvent::TranscriptionFailed {
                        reason: e.to_string(),
                        request_id,
                    }))
                    .await;
            }
            Ok(Err(e)) => {
                let _ = bus
                    .broadcast(Event::Speech(SpeechEvent::TranscriptionFailed {
                        reason: format!("backend task panicked: {}", e),
                        request_id,
                    }))
                    .await;
            }
            Err(_) => {
                let _ = bus
                    .broadcast(Event::Speech(SpeechEvent::TranscriptionFailed {
                        reason: "transcription timeout (>10s)".to_string(),
                        request_id,
                    }))
                    .await;
            }
        }
    }

    /// Handle an audio buffer for continuous listen mode.
    ///
    /// Feeds the chunk to the active backend (which manages streaming logic internally
    /// — rolling buffer for Sherpa/Whisper, chunk accumulation for Parakeet), then
    /// checks VAD for end-of-speech to trigger a final `flush()`.
    async fn handle_listen_audio_buffer(
        &mut self,
        buffer: AudioBuffer,
        session_id: u64,
        bus: &Arc<EventBus>,
    ) {
        let backend = match self.backend.as_ref() {
            Some(b) => Arc::clone(b),
            None => return,
        };

        let samples = buffer.samples.clone();

        // Feed chunk to backend — may return Partial events for streaming display.
        let feed_result = tokio::time::timeout(
            TRANSCRIPTION_TIMEOUT,
            tokio::task::spawn_blocking({
                let backend = Arc::clone(&backend);
                let s = samples.clone();
                move || {
                    let mut b = backend.lock().map_err(|_| {
                        SpeechError::ChannelClosed("backend lock poisoned".to_string())
                    })?;
                    b.feed(&s)
                }
            }),
        )
        .await;

        match feed_result {
            Ok(Ok(Ok(events))) => {
                let now = Instant::now();
                for event in events {
                    match event {
                        SttEvent::Partial { text, confidence } if !text.trim().is_empty() => {
                            // Detect sentence boundary: gap since last token ≥ SENTENCE_BOUNDARY_SECS.
                            let is_new_sentence = self
                                .listen_last_partial_instant
                                .map(|prev| prev.elapsed().as_secs_f32() >= SENTENCE_BOUNDARY_SECS)
                                .unwrap_or(false);
                            self.listen_last_partial_instant = Some(now);

                            let _ = bus
                                .broadcast(Event::Speech(SpeechEvent::ListenModeTranscription {
                                    text,
                                    is_final: false,
                                    is_new_sentence,
                                    confidence,
                                    session_id,
                                }))
                                .await;
                        }
                        _ => {}
                    }
                }
            }
            Ok(Ok(Err(e))) => tracing::warn!("listen[feed]: error: {}", e),
            Ok(Err(e)) => tracing::warn!("listen[feed]: task panicked: {}", e),
            Err(_) => tracing::warn!("listen[feed]: timeout"),
        }

        // VAD for end-of-speech detection — flush the backend when speech ends
        // (backup for backends without EOU or when EOU fires late).
        if let Some(_ready) =
            self.listen_mode_vad
                .feed(&buffer.samples, buffer.sample_rate, buffer.channels)
        {
            let flush_result = tokio::time::timeout(
                TRANSCRIPTION_TIMEOUT,
                tokio::task::spawn_blocking({
                    let backend = Arc::clone(&backend);
                    move || {
                        let mut b = backend.lock().map_err(|_| {
                            SpeechError::ChannelClosed("backend lock poisoned".to_string())
                        })?;
                        b.flush()
                    }
                }),
            )
            .await;

            match flush_result {
                Ok(Ok(Ok(events))) => {
                    for event in events {
                        if let SttEvent::Completed { text, confidence } = event {
                            if !text.trim().is_empty() {
                                self.listen_last_partial_instant = None;
                                let _ = bus
                                    .broadcast(Event::Speech(
                                        SpeechEvent::ListenModeTranscription {
                                            text,
                                            is_final: true,
                                            is_new_sentence: false,
                                            confidence,
                                            session_id,
                                        },
                                    ))
                                    .await;
                            }
                        }
                    }
                }
                Ok(Ok(Err(e))) => tracing::warn!("listen[flush]: error: {}", e),
                Ok(Err(e)) => tracing::warn!("listen[flush]: task panicked: {}", e),
                Err(_) => tracing::warn!("listen[flush]: timeout"),
            }
        }
    }

    /// Handle an audio buffer with silence detection for always-listening mode.
    async fn handle_audio_buffer(&mut self, buffer: AudioBuffer, bus: &Arc<EventBus>) {
        if let Some(ready_buffer) =
            self.always_listening_vad
                .feed(&buffer.samples, buffer.sample_rate, buffer.channels)
        {
            let request_id = self.next_request_id();
            self.execute_transcription(ready_buffer.samples, request_id, bus)
                .await;
        }
    }

    /// Convert a string backend name to `SttBackendKind`.
    fn parse_backend_name(backend_str: &str) -> Result<SttBackendKind, String> {
        match backend_str.to_lowercase().as_str() {
            "whisper" => Ok(SttBackendKind::Whisper),
            "sherpa" => Ok(SttBackendKind::Sherpa),
            "parakeet" => Ok(SttBackendKind::Parakeet),
            "mock" => Ok(SttBackendKind::Mock),
            _ => Err(format!(
                "unknown backend '{}', expected: whisper, sherpa, parakeet, mock",
                backend_str
            )),
        }
    }
}

#[async_trait]
impl Actor for SttActor {
    fn name(&self) -> &'static str {
        "stt"
    }

    async fn start(&mut self, bus: Arc<EventBus>) -> Result<(), ActorError> {
        self.bus = Some(bus.clone());
        self.bus_rx = Some(bus.subscribe_broadcast());

        tracing::info!(
            "stt: initializing backend (backend: {:?})",
            self.backend_kind
        );
        if let Err(e) = self.initialize_backend().await {
            tracing::error!("stt: backend init failed: {}", e);
            let _ = bus
                .broadcast(Event::Speech(SpeechEvent::SpeechFailed {
                    reason: format!("STT model load failed: {}", e),
                    request_id: 0,
                }))
                .await;
            return Err(ActorError::StartupFailed(e.to_string()));
        }
        tracing::info!("stt: backend initialized");

        tracing::info!(
            "stt: starting audio capture (always_listening={})",
            self.voice_always_listening
        );
        if let Err(e) = self.maybe_start_audio_capture() {
            tracing::error!("stt: audio capture failed: {}", e);
            let _ = bus
                .broadcast(Event::Speech(SpeechEvent::SpeechFailed {
                    reason: format!("audio device unavailable: {}", e),
                    request_id: 0,
                }))
                .await;
            return Err(ActorError::StartupFailed(e.to_string()));
        }

        bus.broadcast(Event::System(SystemEvent::ActorReady { actor_name: "stt" }))
            .await
            .map_err(|e| {
                ActorError::StartupFailed(format!("broadcast ActorReady failed: {}", e))
            })?;

        tracing::info!("stt: actor ready");
        Ok(())
    }

    async fn run(&mut self) -> Result<(), ActorError> {
        let mut bus_rx = self.bus_rx.take().ok_or_else(|| {
            ActorError::RuntimeError("bus_rx not initialized in start()".to_string())
        })?;

        let bus = self
            .bus
            .as_ref()
            .ok_or_else(|| ActorError::RuntimeError("bus not initialized in start()".to_string()))?
            .clone();

        loop {
            tokio::select! {
                bus_event = bus_rx.recv() => {
                    match bus_event {
                        Ok(Event::System(SystemEvent::LoopControlRequested { loop_name, enabled })) => {
                            if loop_name == "speech" {
                                self.speech_loop_enabled = enabled;
                                if !enabled {
                                    self.audio_rx = None;
                                    self.audio_stream = None;
                                } else if self.voice_always_listening {
                                    if let Err(e) = self.maybe_start_audio_capture() {
                                        tracing::warn!("speech: failed to restart audio capture: {}", e);
                                    }
                                }
                                let _ = bus.broadcast(Event::System(SystemEvent::LoopStatusChanged {
                                    loop_name: "speech".to_string(),
                                    enabled,
                                })).await;
                            }
                        }
                        Ok(Event::System(SystemEvent::ShutdownSignal)) => break,
                        Ok(Event::Speech(SpeechEvent::VoiceInputDetected { audio_bytes, duration_ms: _ })) => {
                            if let Ok(samples) = decode_audio_samples(&audio_bytes) {
                                let request_id = self.next_request_id();
                                self.execute_transcription(samples, request_id, &bus).await;
                            } else {
                                let request_id = self.next_request_id();
                                let _ = bus.broadcast(Event::Speech(SpeechEvent::TranscriptionFailed {
                                    reason: "invalid audio bytes".to_string(),
                                    request_id,
                                })).await;
                            }
                        }
                        Ok(Event::Speech(SpeechEvent::WakewordDetected { confidence: _ })) => {
                            if self.audio_stream.is_none() {
                                if let Err(e) = self.maybe_start_audio_capture() {
                                    let _ = bus.broadcast(Event::Speech(SpeechEvent::SpeechFailed {
                                        reason: format!("audio capture failed after wakeword: {}", e),
                                        request_id: 0,
                                    })).await;
                                }
                            }
                            self.always_listening_vad.reset();
                        }
                        Ok(Event::Speech(SpeechEvent::ListenModeRequested { session_id })) => {
                            if self.listen_session_id.is_some() {
                                tracing::warn!(
                                    "listen: new session {} requested while session {:?} active - stopping old session first",
                                    session_id, self.listen_session_id
                                );
                                // Flush backend to clean state from previous session.
                                if let Some(b) = &self.backend {
                                    let b = Arc::clone(b);
                                    let _ = tokio::task::spawn_blocking(move || {
                                        b.lock().map(|mut g| { let _ = g.flush(); }).ok()
                                    }).await;
                                }
                                self.listen_audio_rx = None;
                                self.listen_audio_stream = None;
                            }

                            // Derive buffer duration from the backend's preferred chunk size.
                            let (buffer_duration, backend_label) = match &self.backend {
                                Some(b) => {
                                    match b.lock() {
                                        Ok(guard) => {
                                            let chunk = guard.preferred_chunk_samples();
                                            let label = guard.backend_name();
                                            (chunk as f32 / 16_000.0, label)
                                        }
                                        Err(_) => (LISTEN_DEFAULT_BUFFER_DURATION_SECS, "unknown"),
                                    }
                                }
                                None => (LISTEN_DEFAULT_BUFFER_DURATION_SECS, "uninitialized"),
                            };

                            let config = AudioInputConfig {
                                sample_rate: 16_000,
                                buffer_duration_secs: buffer_duration,
                                energy_threshold: 0.0,
                                device_name: self.microphone_device.clone(),
                            };
                            match AudioInputStream::start(config) {
                                Ok((stream, rx)) => {
                                    self.listen_session_id = Some(session_id);
                                    self.listen_audio_stream = Some(stream);
                                    self.listen_audio_rx = Some(rx);
                                    self.listen_mode_active = true;
                                    self.always_listening_vad.reset();
                                    self.listen_mode_vad.reset();
                                    self.listen_last_partial_instant = None;
                                    tracing::info!(
                                        "listen: session {} started ({})",
                                        session_id, backend_label
                                    );
                                }
                                Err(e) => {
                                    tracing::error!("listen: failed to start audio capture: {}", e);
                                    let _ = bus
                                        .broadcast(Event::Speech(SpeechEvent::ListenModeStopped {
                                            session_id,
                                        }))
                                        .await;
                                }
                            }
                        }
                        Ok(Event::Speech(SpeechEvent::ListenModeStopRequested { session_id })) => {
                            if self.listen_session_id == Some(session_id) {
                                // Flush backend state so next session starts clean.
                                if let Some(b) = &self.backend {
                                    let b = Arc::clone(b);
                                    let _ = tokio::task::spawn_blocking(move || {
                                        b.lock().map(|mut g| { let _ = g.flush(); }).ok()
                                    }).await;
                                }
                                self.listen_session_id = None;
                                self.listen_audio_rx = None;
                                self.listen_audio_stream = None;
                                self.listen_mode_vad.reset();
                                self.listen_mode_active = false;
                                self.listen_last_partial_instant = None;
                                tracing::info!("listen: session {} stopped", session_id);
                                let _ = bus
                                    .broadcast(Event::Speech(SpeechEvent::ListenModeStopped {
                                        session_id,
                                    }))
                                    .await;
                            }
                        }
                        Ok(Event::Speech(SpeechEvent::WakewordSuppressed { .. })) => {
                            self.wakeword_suppressed = true;
                            tracing::debug!("stt: muted — wakeword suppressed");
                        }
                        Ok(Event::Speech(SpeechEvent::WakewordResumed)) => {
                            self.wakeword_suppressed = false;
                            tracing::debug!("stt: unmuted — wakeword resumed");
                        }
                        Ok(Event::Speech(SpeechEvent::SttBackendSwitchRequested { backend })) => {
                            tracing::info!("stt: backend switch requested to '{}'", backend);

                            let new_kind = match Self::parse_backend_name(&backend) {
                                Ok(b) => b,
                                Err(e) => {
                                    tracing::warn!("stt: invalid backend name: {}", e);
                                    let _ = bus.broadcast(Event::Speech(
                                        SpeechEvent::SttBackendSwitchFailed {
                                            backend: backend.clone(),
                                            reason: e,
                                        },
                                    )).await;
                                    continue;
                                }
                            };

                            self.backend_switching = true;

                            // Save old backend Arc for rollback; workers keep running.
                            let old_backend = self.backend.take();
                            let old_kind = self.backend_kind;

                            match build_stt_backend(
                                new_kind,
                                self.model_dir.as_deref(),
                                self.whisper_model_path.as_deref(),
                                self.stt_energy_threshold,
                            )
                            .await
                            {
                                Ok(new_b) => {
                                    self.backend = Some(Arc::new(Mutex::new(new_b)));
                                    self.backend_kind = new_kind;
                                    // old_backend dropped here → Drop sends Shutdown to old workers.
                                    tracing::info!("stt: backend switched to {:?}", new_kind);
                                    let _ = bus.broadcast(Event::Speech(
                                        SpeechEvent::SttBackendSwitchCompleted {
                                            backend: backend.clone(),
                                        },
                                    )).await;
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        "stt: backend switch failed ({}), rolling back to {:?}",
                                        e, old_kind
                                    );
                                    // Restore old backend (workers still alive).
                                    self.backend = old_backend;
                                    let _ = bus.broadcast(Event::Speech(
                                        SpeechEvent::SttBackendSwitchFailed {
                                            backend: backend.clone(),
                                            reason: e.to_string(),
                                        },
                                    )).await;
                                }
                            }

                            self.backend_switching = false;
                        }
                        Ok(_) => {}
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => {
                            return Err(ActorError::ChannelClosed("bus_rx closed".to_string()));
                        }
                    }
                }
                audio_buffer = async {
                    if let Some(rx) = self.audio_rx.as_mut() {
                        rx.recv().await
                    } else {
                        std::future::pending().await
                    }
                } => {
                    if let Some(buffer) = audio_buffer {
                        if self.backend_switching || self.wakeword_suppressed {
                            tracing::debug!("skipping audio during backend switch");
                        } else if !self.listen_mode_active && self.speech_loop_enabled {
                            self.handle_audio_buffer(buffer, &bus).await;
                        }
                    }
                }
                listen_buffer = async {
                    if let Some(rx) = self.listen_audio_rx.as_mut() {
                        rx.recv().await
                    } else {
                        std::future::pending::<Option<AudioBuffer>>().await
                    }
                } => {
                    if let (Some(buffer), Some(session_id)) =
                        (listen_buffer, self.listen_session_id)
                    {
                        if self.backend_switching || self.wakeword_suppressed {
                            tracing::debug!("skipping listen audio during backend switch");
                        } else {
                            self.handle_listen_audio_buffer(buffer, session_id, &bus).await;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), ActorError> {
        // Drop the backend — each backend's Drop impl sends Shutdown to its worker thread.
        self.backend = None;

        self.audio_rx = None;
        self.audio_stream = None;
        self.listen_audio_rx = None;
        self.listen_audio_stream = None;
        self.listen_session_id = None;

        Ok(())
    }
}

fn decode_audio_samples(bytes: &[u8]) -> Result<Vec<f32>, SpeechError> {
    if bytes.len().is_multiple_of(4) {
        return Ok(bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect());
    }

    if bytes.len().is_multiple_of(2) {
        return Ok(bytes
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / i16::MAX as f32)
            .collect());
    }

    Err(SpeechError::TranscriptionFailed(
        "unsupported audio byte payload".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stt_actor_boots_and_stops_cleanly() {
        let bus = Arc::new(EventBus::new());
        let mut actor = SttActor::new(SttBackendKind::Mock, false, 0.01, None);

        actor
            .start(Arc::clone(&bus))
            .await
            .expect("stt actor should start with mock backend");

        let actor_handle = tokio::spawn(async move { actor.run().await });

        bus.broadcast(Event::System(SystemEvent::ShutdownSignal))
            .await
            .expect("shutdown broadcast should succeed");

        actor_handle
            .await
            .expect("run task should join")
            .expect("actor should stop cleanly");
    }

    #[tokio::test]
    async fn mock_backend_emits_transcription_completed_with_expected_text() {
        let bus = Arc::new(EventBus::new());
        let mut actor = SttActor::new(SttBackendKind::Mock, false, 0.01, None);

        actor
            .start(Arc::clone(&bus))
            .await
            .expect("stt actor should start with mock backend");

        let mut event_rx = bus.subscribe_broadcast();
        let actor_handle = tokio::spawn(async move { actor.run().await });

        let samples: Vec<f32> = (0..48_000).map(|_| 0.1f32).collect();
        let audio_bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();

        bus.broadcast(Event::Speech(SpeechEvent::VoiceInputDetected {
            audio_bytes,
            duration_ms: 3_000,
        }))
        .await
        .expect("voice input event should broadcast");

        let mut found = false;
        for _ in 0..15 {
            match tokio::time::timeout(Duration::from_millis(100), event_rx.recv()).await {
                Ok(Ok(Event::Speech(SpeechEvent::TranscriptionCompleted {
                    text,
                    confidence,
                    ..
                }))) => {
                    assert_eq!(text, "mock transcription");
                    assert!(confidence >= 0.5);
                    found = true;
                    break;
                }
                Ok(Ok(_)) => continue,
                Ok(Err(_)) => break,
                Err(_) => break,
            }
        }

        assert!(found, "expected transcription completed event");

        bus.broadcast(Event::System(SystemEvent::ShutdownSignal))
            .await
            .expect("shutdown broadcast should succeed");

        actor_handle
            .await
            .expect("run task should join")
            .expect("actor should stop cleanly");
    }

    #[tokio::test]
    async fn low_energy_audio_emits_low_confidence_event() {
        let bus = Arc::new(EventBus::new());
        let mut actor = SttActor::new(SttBackendKind::Mock, false, 0.01, None);

        actor
            .start(Arc::clone(&bus))
            .await
            .expect("stt actor should start with mock backend");

        let mut event_rx = bus.subscribe_broadcast();
        let actor_handle = tokio::spawn(async move { actor.run().await });

        let samples = vec![0.0f32; 48_000];
        let audio_bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();

        bus.broadcast(Event::Speech(SpeechEvent::VoiceInputDetected {
            audio_bytes,
            duration_ms: 3_000,
        }))
        .await
        .expect("voice input event should broadcast");

        let mut found = false;
        for _ in 0..15 {
            match tokio::time::timeout(Duration::from_millis(100), event_rx.recv()).await {
                Ok(Ok(Event::Speech(SpeechEvent::LowConfidenceTranscription {
                    confidence,
                    ..
                }))) => {
                    assert!(confidence < 0.5, "confidence should be below threshold");
                    found = true;
                    break;
                }
                Ok(Ok(_)) => continue,
                Ok(Err(_)) => break,
                Err(_) => break,
            }
        }

        assert!(found, "expected low confidence transcription event");

        bus.broadcast(Event::System(SystemEvent::ShutdownSignal))
            .await
            .expect("shutdown broadcast should succeed");

        actor_handle
            .await
            .expect("run task should join")
            .expect("actor should stop cleanly");
    }

    #[test]
    fn decode_audio_samples_supports_f32_payload() {
        let samples = vec![0.25f32, -0.25f32];
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let decoded = decode_audio_samples(&bytes).expect("f32 payload should decode");
        assert_eq!(decoded.len(), 2);
        assert!((decoded[0] - 0.25).abs() < 1e-6);
    }
}
