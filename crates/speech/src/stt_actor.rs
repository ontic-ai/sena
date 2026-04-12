//! STT Actor - speech-to-text processing.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{broadcast, mpsc};

use bus::{Actor, ActorError, Event, EventBus, SpeechEvent, SystemEvent};

use crate::audio_input::{AudioInputConfig, AudioInputStream};
use crate::worker::{SttWorker, WorkerEvent};
use crate::{AudioBuffer, SpeechError, SttBackend};

const TRANSCRIPTION_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_BUFFER_DURATION_SECS: f32 = 3.0;

/// STT Actor - handles speech-to-text transcription.
///
/// Pipeline:
/// 1. On start, initialize STT backend (whisper model load for whisper backend)
/// 2. If always-listening is enabled, capture mic audio and buffer speech chunks
/// 3. Also listen for SpeechEvent::VoiceInputDetected on the bus (on-demand mode)
/// 4. Transcribe in a blocking worker and emit completion/failure events
/// 5. Silence detection: accumulate audio during speech, transcribe after silence threshold
pub struct SttActor {
    backend: SttBackend,
    bus: Option<Arc<EventBus>>,
    bus_rx: Option<broadcast::Receiver<Event>>,
    audio_rx: Option<mpsc::UnboundedReceiver<AudioBuffer>>,
    audio_stream: Option<AudioInputStream>,
    backend_handle: Option<SttBackendHandle>,
    request_id_counter: u64,
    voice_always_listening: bool,
    stt_energy_threshold: f32,
    /// Minimum confidence score for transcription output to be accepted.
    /// Range [0.0, 1.0]. Default: 0.5.
    confidence_threshold: f32,
    /// Path to Whisper model file. Will be used by whisper-rs.
    #[allow(dead_code)]
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
}

impl SttActor {
    /// Create a new STT actor with backend and runtime config values.
    pub fn new(
        backend: SttBackend,
        voice_always_listening: bool,
        stt_energy_threshold: f32,
        whisper_model_path: Option<String>,
    ) -> Self {
        Self {
            backend,
            bus: None,
            bus_rx: None,
            audio_rx: None,
            audio_stream: None,
            backend_handle: None,
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
                1.5,
            ),
            listen_session_id: None,
            listen_audio_rx: None,
            listen_audio_stream: None,
            listen_mode_active: false,
            microphone_device: None,
            speech_loop_enabled: true,
        }
    }

    /// Set the model directory path (where downloaded models are stored).
    pub fn with_model_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.model_dir = dir;
        self
    }

    /// Set the silence duration threshold in seconds.
    /// When silence lasts longer than this after speech, transcription is triggered.
    pub fn with_silence_duration(mut self, secs: f32) -> Self {
        self.silence_duration_secs = secs;
        self.always_listening_vad =
            crate::silence_detector::SilenceDetector::new(self.stt_energy_threshold, secs);
        self.listen_mode_vad =
            crate::silence_detector::SilenceDetector::new(self.stt_energy_threshold, secs);
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

    async fn initialize_backend(&mut self) -> Result<(), SpeechError> {
        let handle = match self.backend {
            SttBackend::Mock => SttBackendHandle::Mock,
            SttBackend::Whisper => {
                // Resolve model path: use whisper_model_path if set, else model_dir/ggml-base.en.bin
                let model_path = if let Some(ref path) = self.whisper_model_path {
                    path.clone()
                } else if let Some(ref dir) = self.model_dir {
                    dir.join("ggml-base.en.bin").to_string_lossy().to_string()
                } else {
                    return Err(SpeechError::SttInitFailed(
                        "no whisper model path configured".to_string(),
                    ));
                };

                // Spawn stt-worker child process
                let mut worker = SttWorker::spawn(&model_path).await?;

                // Wait for "listening" event to confirm worker is ready
                match worker.read_event().await? {
                    Some(WorkerEvent::Listening) => {
                        tracing::info!("stt: worker ready (model: {})", model_path);
                    }
                    Some(WorkerEvent::Error { reason }) => {
                        return Err(SpeechError::SttInitFailed(format!(
                            "worker failed to initialize: {}",
                            reason
                        )));
                    }
                    _ => {
                        return Err(SpeechError::SttInitFailed(
                            "unexpected worker response during init".to_string(),
                        ));
                    }
                }

                // Emit SttModelLoaded event
                if let Some(ref bus) = self.bus {
                    let _ = bus
                        .broadcast(Event::Speech(SpeechEvent::SttModelLoaded {
                            model_name: model_path.clone(),
                            backend: "stt-worker".to_string(),
                        }))
                        .await;
                }

                SttBackendHandle::Whisper(Box::new(worker))
            }
        };

        self.backend_handle = Some(handle);
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

    async fn transcribe(
        &mut self,
        buffer: AudioBuffer,
    ) -> Result<TranscriptionResult, SpeechError> {
        match self.backend_handle.as_mut() {
            Some(SttBackendHandle::Mock) => {
                let rms = crate::silence_detector::calculate_rms(&buffer.samples);
                if rms < 0.001 {
                    Ok(TranscriptionResult {
                        text: String::new(),
                        confidence: 0.1,
                        words: vec![],
                    })
                } else {
                    Ok(TranscriptionResult {
                        text: "mock transcription".to_string(),
                        confidence: 0.85,
                        words: vec![],
                    })
                }
            }
            Some(SttBackendHandle::Whisper(worker)) => {
                // Write audio chunk to worker stdin
                worker.write_chunk(&buffer.samples).await?;

                // Read worker events until we get a Completed event
                let full_text;
                let avg_confidence;
                let mut words = Vec::new();

                loop {
                    match worker.read_event().await? {
                        Some(WorkerEvent::Word {
                            text, confidence, ..
                        }) => {
                            // Accumulate words (optional — we wait for Completed for full text)
                            words.push(bus::events::speech::TranscribedWord {
                                text: text.clone(),
                                confidence,
                                start_ms: 0, // Worker doesn't provide timestamps yet
                                end_ms: 0,
                            });
                        }
                        Some(WorkerEvent::Completed {
                            text,
                            avg_confidence: conf,
                            ..
                        }) => {
                            full_text = text;
                            avg_confidence = conf;
                            break;
                        }
                        Some(WorkerEvent::Error { reason }) => {
                            return Err(SpeechError::TranscriptionFailed(reason));
                        }
                        Some(WorkerEvent::Stopped) => {
                            return Err(SpeechError::WorkerCrashed(0));
                        }
                        Some(WorkerEvent::Listening) => {
                            // Unexpected — ignore
                        }
                        None => {
                            return Err(SpeechError::WorkerPipeError(
                                "worker stdout closed unexpectedly".to_string(),
                            ));
                        }
                    }
                }

                Ok(TranscriptionResult {
                    text: full_text,
                    confidence: avg_confidence,
                    words,
                })
            }
            None => Err(SpeechError::TranscriptionFailed(
                "STT backend not initialized".to_string(),
            )),
        }
    }

    async fn transcribe_with_timeout(
        &mut self,
        buffer: AudioBuffer,
        request_id: u64,
        bus: &Arc<EventBus>,
    ) {
        match tokio::time::timeout(TRANSCRIPTION_TIMEOUT, self.transcribe(buffer)).await {
            Ok(Ok(result)) => {
                if result.confidence >= self.confidence_threshold {
                    // Emit completion with word-level data
                    let avg = if result.words.is_empty() {
                        result.confidence
                    } else {
                        result.words.iter().map(|w| w.confidence).sum::<f32>()
                            / result.words.len() as f32
                    };
                    let _ = bus
                        .broadcast(Event::Speech(SpeechEvent::TranscriptionCompleted {
                            text: result.text,
                            confidence: result.confidence,
                            request_id,
                            words: result.words,
                            average_confidence: avg,
                        }))
                        .await;
                } else {
                    // Low confidence: inform user that speech was detected but unclear
                    let _ = bus
                        .broadcast(Event::Speech(SpeechEvent::LowConfidenceTranscription {
                            confidence: result.confidence,
                            request_id,
                        }))
                        .await;
                }
            }
            Ok(Err(e)) => {
                let _ = bus
                    .broadcast(Event::Speech(SpeechEvent::TranscriptionFailed {
                        reason: e.to_string(),
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
    /// Emits `ListenModeTranscription` with `is_final=false` for low-energy buffers and
    /// `is_final=true` after silence-threshold silence within the session.
    async fn handle_listen_audio_buffer(
        &mut self,
        buffer: AudioBuffer,
        session_id: u64,
        bus: &Arc<EventBus>,
    ) {
        if let Some(ready_buffer) =
            self.listen_mode_vad
                .feed(&buffer.samples, buffer.sample_rate, buffer.channels)
        {
            match tokio::time::timeout(TRANSCRIPTION_TIMEOUT, self.transcribe(ready_buffer)).await {
                Ok(Ok(result)) => {
                    if !result.text.trim().is_empty() {
                        let _ = bus
                            .broadcast(Event::Speech(SpeechEvent::ListenModeTranscription {
                                text: result.text,
                                is_final: true,
                                confidence: result.confidence,
                                session_id,
                            }))
                            .await;
                    }
                }
                Ok(Err(e)) => tracing::warn!("listen: transcription error: {}", e),
                Err(_) => tracing::warn!("listen: transcription timeout"),
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
            self.transcribe_with_timeout(ready_buffer, request_id, bus)
                .await;
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

        tracing::info!("stt: initializing backend (backend: {:?})", self.backend);
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
                                // When disabling, stop audio capture
                                if !enabled {
                                    self.audio_rx = None;
                                    self.audio_stream = None;
                                } else {
                                    // Re-enable: restart audio capture if always-listening is configured
                                    if self.voice_always_listening {
                                        if let Err(e) = self.maybe_start_audio_capture() {
                                            tracing::warn!("speech: failed to restart audio capture: {}", e);
                                        }
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
                                let buffer = AudioBuffer { samples, sample_rate: 16_000, channels: 1 };
                                self.transcribe_with_timeout(buffer, request_id, &bus).await;
                            } else {
                                let request_id = self.next_request_id();
                                let _ = bus.broadcast(Event::Speech(SpeechEvent::TranscriptionFailed {
                                    reason: "invalid audio bytes".to_string(),
                                    request_id,
                                })).await;
                            }
                        }
                        Ok(Event::Speech(SpeechEvent::WakewordDetected { confidence: _ })) => {
                            // Wakeword detected - ensure audio capture is active
                            if self.audio_stream.is_none() {
                                if let Err(e) = self.maybe_start_audio_capture() {
                                    let _ = bus.broadcast(Event::Speech(SpeechEvent::SpeechFailed {
                                        reason: format!("audio capture failed after wakeword: {}", e),
                                        request_id: 0,
                                    })).await;
                                }
                            }
                            // Reset only always-listening VAD, not listen mode VAD
                            self.always_listening_vad.reset();
                        }
                        Ok(Event::Speech(SpeechEvent::ListenModeRequested { session_id })) => {
                            if self.listen_session_id.is_some() {
                                tracing::warn!("listen: new session {} requested while session {:?} active - stopping old session first", session_id, self.listen_session_id);
                                self.listen_audio_rx = None;
                                self.listen_audio_stream = None;
                            }
                            let config = AudioInputConfig {
                                sample_rate: 16_000,
                                buffer_duration_secs: DEFAULT_BUFFER_DURATION_SECS,
                                // Pass all audio to SilenceDetector - it handles
                                // voice/silence classification internally.
                                energy_threshold: 0.0,
                                device_name: self.microphone_device.clone(),
                            };
                            match AudioInputStream::start(config) {
                                Ok((stream, rx)) => {
                                    self.listen_session_id = Some(session_id);
                                    self.listen_audio_stream = Some(stream);
                                    self.listen_audio_rx = Some(rx);
                                    // Suppress always-on STT while listen mode is active to
                                    // prevent conflicting TranscriptionCompleted events and
                                    // unintended inference calls.
                                    self.listen_mode_active = true;
                                    self.always_listening_vad.reset();
                                    // Reset listen mode VAD for a clean session.
                                    self.listen_mode_vad.reset();
                                    tracing::info!("listen: session {} started", session_id);
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
                                self.listen_session_id = None;
                                self.listen_audio_rx = None;
                                self.listen_audio_stream = None;
                                self.listen_mode_vad.reset();
                                self.listen_mode_active = false;
                                tracing::info!("listen: session {} stopped", session_id);
                                let _ = bus
                                    .broadcast(Event::Speech(SpeechEvent::ListenModeStopped {
                                        session_id,
                                    }))
                                    .await;
                            }
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
                    // Skip always-on processing while listen mode is active or speech loop is disabled.
                    // Listen mode dispatches its own audio via listen_audio_rx.
                    if let Some(buffer) = audio_buffer {
                        if !self.listen_mode_active && self.speech_loop_enabled {
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
                    // Listen mode is independent of speech_loop_enabled (explicit user request)
                    if let (Some(buffer), Some(session_id)) = (listen_buffer, self.listen_session_id) {
                        self.handle_listen_audio_buffer(buffer, session_id, &bus).await;
                    }
                }
            }
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), ActorError> {
        // Shutdown worker gracefully
        if let Some(SttBackendHandle::Whisper(worker)) = self.backend_handle.as_mut() {
            if let Err(e) = worker.shutdown().await {
                tracing::warn!("stt: worker shutdown signal failed: {}", e);
            }
            if let Err(e) = tokio::time::timeout(Duration::from_secs(5), worker.wait()).await {
                tracing::warn!("stt: worker did not exit within 5s: {}", e);
                let _ = worker.kill().await;
            }
        }

        self.audio_rx = None;
        self.audio_stream = None;
        self.listen_audio_rx = None;
        self.listen_audio_stream = None;
        self.listen_session_id = None;
        self.backend_handle = None;

        Ok(())
    }
}

enum SttBackendHandle {
    Mock,
    Whisper(Box<SttWorker>),
}

struct TranscriptionResult {
    text: String,
    confidence: f32,
    words: Vec<bus::events::speech::TranscribedWord>,
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
        let mut actor = SttActor::new(SttBackend::Mock, false, 0.01, None);

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
        let mut actor = SttActor::new(SttBackend::Mock, false, 0.01, None);

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
        let mut actor = SttActor::new(SttBackend::Mock, false, 0.01, None);

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
