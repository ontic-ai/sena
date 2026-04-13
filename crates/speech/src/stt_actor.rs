//! STT Actor - speech-to-text processing.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{broadcast, mpsc, oneshot};

use bus::{Actor, ActorError, Event, EventBus, SpeechEvent, SystemEvent};

use crate::audio_input::{AudioInputConfig, AudioInputStream};
use crate::{AudioBuffer, SpeechError, SttBackend};

const TRANSCRIPTION_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_BUFFER_DURATION_SECS: f32 = 3.0;

/// Audio chunk duration for Whisper fake-streaming /listen sessions (1s for responsiveness).
const LISTEN_WHISPER_BUFFER_DURATION_SECS: f32 = 1.0;

/// Audio chunk duration for sherpa-onnx Zipformer listen sessions (200ms for responsiveness).
const LISTEN_SHERPA_BUFFER_DURATION_SECS: f32 = 0.2;

/// How often to run sherpa decode on the growing listen buffer.
const LISTEN_SHERPA_INTERVAL: Duration = Duration::from_millis(200);

/// Maximum audio retained for sherpa growing-window decode (8s at 16kHz).
const LISTEN_SHERPA_MAX_SAMPLES: usize = 16_000 * 8;

/// Audio chunk duration for Parakeet-EOU streaming sessions (160ms — required chunk size).
const LISTEN_PARAKEET_BUFFER_DURATION_SECS: f32 = 0.16;

/// Exact chunk size required by ParakeetEOU.transcribe() — 160ms at 16kHz.
/// Feeding larger chunks causes the model to silently discard all but the last 160ms.
const PARAKEET_CHUNK_SAMPLES: usize = 2_560;

/// Maximum rolling audio retained for listen-mode interim transcriptions (6 seconds at 16kHz).
const LISTEN_ROLLING_MAX_SAMPLES: usize = 16_000 * 3;

/// Minimum audio accumulated before first interim transcription attempt (2s at 16kHz).
const LISTEN_INTERIM_MIN_SAMPLES: usize = 16_000 * 2;

/// How often to emit interim (non-final) transcriptions during Whisper listen mode.
const LISTEN_INTERIM_INTERVAL: Duration = Duration::from_millis(1500);

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
    /// Rolling audio accumulator for Whisper fake-streaming interim transcriptions.
    listen_rolling_samples: Vec<f32>,
    /// Time of last interim Whisper transcription in listen mode.
    listen_last_interim: Option<std::time::Instant>,
    /// Sender to the sherpa worker thread. Some when sherpa is active for listen mode.
    sherpa_worker_tx: Option<std::sync::mpsc::Sender<SherpaCmd>>,
    /// True while sherpa is the active STT for the current listen session.
    sherpa_listen_active: bool,
    /// Time of last sherpa decode in the current listen session.
    listen_last_sherpa: Option<std::time::Instant>,
    /// Sender to the parakeet worker thread. Some when parakeet is active for listen mode.
    parakeet_worker_tx: Option<std::sync::mpsc::Sender<ParakeetCommand>>,
    /// True while parakeet is the active STT for the current listen session.
    parakeet_listen_active: bool,
    /// Accumulator for sub-chunk parakeet audio. Drained in 2560-sample (160ms) chunks.
    parakeet_chunk_accumulator: Vec<f32>,
    /// True while a backend switch is in progress. Suppresses audio processing.
    backend_switching: bool,
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
            listen_rolling_samples: Vec::new(),
            listen_last_interim: None,
            sherpa_worker_tx: None,
            sherpa_listen_active: false,
            listen_last_sherpa: None,
            parakeet_worker_tx: None,
            parakeet_listen_active: false,
            parakeet_chunk_accumulator: Vec::new(),
            backend_switching: false,
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
            SttBackend::Whisper => self.initialize_candle_backend().await?,
            SttBackend::Sherpa => self.initialize_sherpa_backend().await?,
            SttBackend::Parakeet => self.initialize_parakeet_backend().await?,
        };

        self.backend_handle = Some(handle);
        Ok(())
    }

    async fn initialize_candle_backend(&self) -> Result<SttBackendHandle, SpeechError> {
        let model_dir = self.model_dir.clone();
        let model_path = self.whisper_model_path.clone();

        // Load model in spawn_blocking to avoid blocking async
        let model = tokio::task::spawn_blocking(move || {
            let dir = model_dir
                .as_deref()
                .unwrap_or_else(|| std::path::Path::new(""));
            crate::candle_whisper::CandleWhisperModel::load(dir, model_path.as_deref())
        })
        .await
        .map_err(|e| SpeechError::SttInitFailed(format!("spawn_blocking panicked: {}", e)))??;

        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<WorkerCommand>();
        std::thread::spawn(move || {
            candle_worker_loop(model, cmd_rx);
        });

        Ok(SttBackendHandle::CandleWhisper { tx: cmd_tx })
    }

    async fn initialize_parakeet_backend(&self) -> Result<SttBackendHandle, SpeechError> {
        let model_dir = self
            .model_dir
            .clone()
            .ok_or_else(|| SpeechError::SttInitFailed("model_dir not configured".to_string()))?;

        let parakeet_model_dir = model_dir.join("parakeet");

        // Guard: check model files exist before calling into C library
        if !crate::parakeet_stt::ParakeetStt::models_present(&parakeet_model_dir) {
            return Err(SpeechError::SttInitFailed(format!(
                "Parakeet models not found in {}; run /models download to fetch them",
                parakeet_model_dir.display()
            )));
        }

        tracing::info!(
            "initializing Parakeet backend from {}",
            parakeet_model_dir.display()
        );

        // Load model in spawn_blocking to avoid blocking async
        let model = tokio::task::spawn_blocking(move || {
            crate::parakeet_stt::ParakeetStt::load(&parakeet_model_dir)
        })
        .await
        .map_err(|e| SpeechError::SttInitFailed(format!("spawn_blocking panicked: {}", e)))??;

        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<ParakeetCommand>();
        std::thread::spawn(move || {
            parakeet_worker_loop(model, cmd_rx);
        });

        tracing::info!("Parakeet backend initialized successfully");

        Ok(SttBackendHandle::Parakeet { tx: cmd_tx })
    }

    async fn initialize_sherpa_backend(&self) -> Result<SttBackendHandle, SpeechError> {
        let model_dir = self
            .model_dir
            .clone()
            .ok_or_else(|| SpeechError::SttInitFailed("model_dir not configured".to_string()))?;

        let sherpa_model_dir = model_dir.join("sherpa");

        // Guard: check model files exist before calling into C library.
        // The sherpa-onnx C++ library will abort() the process on certain errors
        // (e.g. wrong ONNX format), so we must validate up front.
        if !crate::sherpa_stt::SherpaZipformerStt::models_present(&sherpa_model_dir) {
            return Err(SpeechError::SttInitFailed(format!(
                "Sherpa models not found in {}; run /models download to fetch them",
                sherpa_model_dir.display()
            )));
        }

        tracing::info!(
            "initializing Sherpa backend from {}",
            sherpa_model_dir.display()
        );

        // Build model file paths
        let encoder = sherpa_model_dir
            .join("encoder-epoch-99-avg-1.int8.onnx")
            .to_str()
            .ok_or_else(|| SpeechError::SttInitFailed("non-UTF-8 path for encoder".to_string()))?
            .to_string();

        let decoder = sherpa_model_dir
            .join("decoder-epoch-99-avg-1.int8.onnx")
            .to_str()
            .ok_or_else(|| SpeechError::SttInitFailed("non-UTF-8 path for decoder".to_string()))?
            .to_string();

        let joiner = sherpa_model_dir
            .join("joiner-epoch-99-avg-1.int8.onnx")
            .to_str()
            .ok_or_else(|| SpeechError::SttInitFailed("non-UTF-8 path for joiner".to_string()))?
            .to_string();

        let tokens = sherpa_model_dir
            .join("tokens.txt")
            .to_str()
            .ok_or_else(|| SpeechError::SttInitFailed("non-UTF-8 path for tokens".to_string()))?
            .to_string();

        // Create channel for worker thread communication
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<SherpaCmd>();
        let (init_tx, init_rx) = std::sync::mpsc::channel::<Result<(), SpeechError>>();

        // Spawn worker thread that creates and owns the model
        std::thread::spawn(move || {
            // Load model in this thread (sherpa-onnx types are not Send)
            match crate::sherpa_stt::SherpaZipformerStt::load(&encoder, &decoder, &joiner, &tokens)
            {
                Ok(model) => {
                    // Signal successful initialization
                    let _ = init_tx.send(Ok(()));
                    // Run worker loop
                    sherpa_worker_loop(model, cmd_rx);
                }
                Err(e) => {
                    // Signal initialization failure
                    let _ = init_tx.send(Err(e));
                }
            }
        });

        // Wait for initialization to complete
        init_rx.recv().map_err(|e| {
            SpeechError::SttInitFailed(format!("sherpa worker init channel failed: {}", e))
        })??;

        tracing::info!("Sherpa backend initialized successfully");

        Ok(SttBackendHandle::Sherpa { tx: cmd_tx })
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

    async fn transcribe(&self, buffer: AudioBuffer) -> Result<TranscriptionResult, SpeechError> {
        match self.backend_handle.as_ref() {
            Some(SttBackendHandle::Mock) => {
                let rms = crate::silence_detector::calculate_rms(&buffer.samples);
                if rms < 0.001 {
                    Ok(TranscriptionResult {
                        text: String::new(),
                        confidence: 0.1,
                    })
                } else {
                    Ok(TranscriptionResult {
                        text: "mock transcription".to_string(),
                        confidence: 0.85,
                    })
                }
            }
            Some(SttBackendHandle::CandleWhisper { tx }) => {
                let (reply_tx, reply_rx) = oneshot::channel();
                tx.send(WorkerCommand::Transcribe {
                    samples: buffer.samples,
                    reply: reply_tx,
                })
                .map_err(|e| {
                    SpeechError::TranscriptionFailed(format!(
                        "candle worker channel send failed: {}",
                        e
                    ))
                })?;

                reply_rx.await.map_err(|e| {
                    SpeechError::TranscriptionFailed(format!("candle worker reply failed: {}", e))
                })?
            }
            Some(SttBackendHandle::Sherpa { tx }) => {
                // Calculate confidence from audio energy before moving samples
                let rms = crate::silence_detector::calculate_rms(&buffer.samples);

                let (reply_tx, reply_rx) = oneshot::channel();
                tx.send(SherpaCmd::Decode {
                    samples: buffer.samples,
                    reply: reply_tx,
                })
                .map_err(|e| {
                    SpeechError::TranscriptionFailed(format!(
                        "sherpa worker channel send failed: {}",
                        e
                    ))
                })?;

                let text = reply_rx.await.map_err(|e| {
                    SpeechError::TranscriptionFailed(format!("sherpa worker reply failed: {}", e))
                })?;

                // Calculate confidence from audio energy
                let confidence = if text.trim().is_empty() {
                    0.0
                } else {
                    (rms * 10.0).clamp(0.55, 0.99)
                };

                Ok(TranscriptionResult { text, confidence })
            }
            Some(SttBackendHandle::Parakeet { tx }) => {
                // Convert f32 samples to i16 for parakeet
                let samples_i16: Vec<i16> = buffer
                    .samples
                    .iter()
                    .map(|&s| (s * 32768.0).clamp(-32768.0, 32767.0) as i16)
                    .collect();

                let (reply_tx, reply_rx) = oneshot::channel();
                tx.send(ParakeetCommand::Transcribe {
                    samples: samples_i16,
                    reply: reply_tx,
                })
                .map_err(|e| {
                    SpeechError::TranscriptionFailed(format!(
                        "parakeet worker channel send failed: {}",
                        e
                    ))
                })?;

                reply_rx.await.map_err(|e| {
                    SpeechError::TranscriptionFailed(format!("parakeet worker reply failed: {}", e))
                })?
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
        // Calculate chunk duration from audio buffer
        let chunk_duration_ms =
            (buffer.samples.len() as f64 / buffer.sample_rate as f64 * 1000.0) as u64;

        // Track latency (time from transcribe start to result)
        let start = std::time::Instant::now();

        match tokio::time::timeout(TRANSCRIPTION_TIMEOUT, self.transcribe(buffer)).await {
            Ok(Ok(result)) => {
                let latency_ms = start.elapsed().as_millis() as u64;

                // Determine backend name and VRAM
                let (backend_name, vram_mb) = match self.backend {
                    SttBackend::Whisper => ("whisper", 142),
                    SttBackend::Sherpa => ("sherpa", 100),
                    SttBackend::Parakeet => ("parakeet", 480),
                    SttBackend::Mock => ("mock", 0),
                };

                // Log telemetry (non-fatal failure — log error but continue)
                if let Err(e) = crate::telemetry::log_stt_telemetry(
                    backend_name,
                    chunk_duration_ms,
                    latency_ms,
                    result.confidence as f64,
                    vram_mb,
                )
                .await
                {
                    tracing::warn!("telemetry write failed: {}", e);
                }

                // Broadcast telemetry event
                let _ = bus
                    .broadcast(Event::Speech(SpeechEvent::SttTelemetryUpdate {
                        backend: backend_name.to_string(),
                        vram_mb: Some(vram_mb as f64),
                        latency_ms: latency_ms as f64,
                        avg_confidence: result.confidence as f64,
                        request_id,
                    }))
                    .await;

                if result.confidence >= self.confidence_threshold {
                    let _ = bus
                        .broadcast(Event::Speech(SpeechEvent::TranscriptionCompleted {
                            text: result.text,
                            confidence: result.confidence,
                            request_id,
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
    /// Sherpa primary path: growing-window decode every 200ms — returns text every ~200ms.
    /// Whisper fallback: rolling buffer + interim every 1500ms (original fake-streaming).
    async fn handle_listen_audio_buffer(
        &mut self,
        buffer: AudioBuffer,
        session_id: u64,
        bus: &Arc<EventBus>,
    ) {
        if self.sherpa_listen_active {
            // ── Sherpa path ──────────────────────────────────────────────────────────
            // Grow the rolling buffer (cap at 8s).
            self.listen_rolling_samples
                .extend_from_slice(&buffer.samples);
            if self.listen_rolling_samples.len() > LISTEN_SHERPA_MAX_SAMPLES {
                let excess = self.listen_rolling_samples.len() - LISTEN_SHERPA_MAX_SAMPLES;
                self.listen_rolling_samples.drain(..excess);
            }

            let interval_elapsed = self
                .listen_last_sherpa
                .map(|t| t.elapsed() >= LISTEN_SHERPA_INTERVAL)
                .unwrap_or(true);

            if interval_elapsed && !self.listen_rolling_samples.is_empty() {
                if let Some(tx) = &self.sherpa_worker_tx {
                    let (reply_tx, reply_rx) = oneshot::channel();
                    let samples = self.listen_rolling_samples.clone();
                    let _ = tx.send(SherpaCmd::Decode {
                        samples,
                        reply: reply_tx,
                    });
                    self.listen_last_sherpa = Some(std::time::Instant::now());

                    match tokio::time::timeout(Duration::from_millis(500), reply_rx).await {
                        Ok(Ok(text)) if !text.trim().is_empty() => {
                            let _ = bus
                                .broadcast(Event::Speech(SpeechEvent::ListenModeTranscription {
                                    text: text.trim().to_string(),
                                    is_final: false,
                                    confidence: 0.9,
                                    session_id,
                                }))
                                .await;
                        }
                        Ok(Ok(_)) => {}
                        Ok(Err(_)) => tracing::warn!("listen[sherpa]: worker channel closed"),
                        Err(_) => tracing::warn!("listen[sherpa]: decode timeout"),
                    }
                }
            }

            // VAD for final speech-end detection.
            if let Some(_ready_buffer) =
                self.listen_mode_vad
                    .feed(&buffer.samples, buffer.sample_rate, buffer.channels)
            {
                // Emit final transcription of the accumulated rolling buffer.
                if let Some(tx) = &self.sherpa_worker_tx {
                    let (reply_tx, reply_rx) = oneshot::channel();
                    let samples = self.listen_rolling_samples.clone();
                    let _ = tx.send(SherpaCmd::Decode {
                        samples,
                        reply: reply_tx,
                    });

                    match tokio::time::timeout(Duration::from_millis(1000), reply_rx).await {
                        Ok(Ok(text)) if !text.trim().is_empty() => {
                            let _ = bus
                                .broadcast(Event::Speech(SpeechEvent::ListenModeTranscription {
                                    text: text.trim().to_string(),
                                    is_final: true,
                                    confidence: 0.9,
                                    session_id,
                                }))
                                .await;
                        }
                        Ok(Ok(_)) => {}
                        Ok(Err(_)) => tracing::warn!("listen[sherpa]: worker closed on final"),
                        Err(_) => tracing::warn!("listen[sherpa]: final decode timeout"),
                    }
                }
                // Reset rolling buffer for next utterance.
                self.listen_rolling_samples.clear();
                self.listen_last_sherpa = None;
            }
        } else if self.parakeet_listen_active {
            // ── Parakeet streaming path ───────────────────────────────────────────────
            // Accumulate incoming audio into the chunk buffer.
            self.parakeet_chunk_accumulator
                .extend_from_slice(&buffer.samples);

            // Drain 2560-sample (160ms) chunks and decode each one.
            while self.parakeet_chunk_accumulator.len() >= PARAKEET_CHUNK_SAMPLES {
                let chunk: Vec<f32> = self
                    .parakeet_chunk_accumulator
                    .drain(..PARAKEET_CHUNK_SAMPLES)
                    .collect();

                if let Some(tx) = &self.parakeet_worker_tx {
                    let (reply_tx, reply_rx) = oneshot::channel();
                    let _ = tx.send(ParakeetCommand::Chunk {
                        samples: chunk,
                        reply: reply_tx,
                    });
                    match tokio::time::timeout(Duration::from_millis(500), reply_rx).await {
                        Ok(Ok(text)) if !text.trim().is_empty() => {
                            let _ = bus
                                .broadcast(Event::Speech(SpeechEvent::ListenModeTranscription {
                                    text: text.trim().to_string(),
                                    is_final: false,
                                    confidence: 0.85,
                                    session_id,
                                }))
                                .await;
                        }
                        Ok(Ok(_)) => {}
                        Ok(Err(_)) => tracing::warn!("listen[parakeet]: worker channel closed"),
                        Err(_) => tracing::warn!("listen[parakeet]: chunk decode timeout"),
                    }
                }
            }

            // VAD for final speech-end detection.
            if let Some(_ready_buffer) =
                self.listen_mode_vad
                    .feed(&buffer.samples, buffer.sample_rate, buffer.channels)
            {
                // Flush any remaining sub-chunk samples (pad to 2560 with silence).
                if !self.parakeet_chunk_accumulator.is_empty() {
                    let mut remaining = self.parakeet_chunk_accumulator.clone();
                    remaining.resize(PARAKEET_CHUNK_SAMPLES, 0.0_f32);

                    if let Some(tx) = &self.parakeet_worker_tx {
                        let (reply_tx, reply_rx) = oneshot::channel();
                        let _ = tx.send(ParakeetCommand::Chunk {
                            samples: remaining,
                            reply: reply_tx,
                        });
                        match tokio::time::timeout(Duration::from_millis(800), reply_rx).await {
                            Ok(Ok(text)) if !text.trim().is_empty() => {
                                let _ = bus
                                    .broadcast(Event::Speech(
                                        SpeechEvent::ListenModeTranscription {
                                            text: text.trim().to_string(),
                                            is_final: true,
                                            confidence: 0.85,
                                            session_id,
                                        },
                                    ))
                                    .await;
                            }
                            Ok(Ok(_)) => {}
                            Ok(Err(_)) => {
                                tracing::warn!("listen[parakeet]: worker closed on flush")
                            }
                            Err(_) => tracing::warn!("listen[parakeet]: flush timeout"),
                        }
                    }
                    self.parakeet_chunk_accumulator.clear();
                }
            }
        } else {
            // ── Whisper fallback path ─────────────────────────────────────────────────
            // Accumulate samples in rolling buffer (max 3s at 16kHz).
            self.listen_rolling_samples
                .extend_from_slice(&buffer.samples);
            if self.listen_rolling_samples.len() > LISTEN_ROLLING_MAX_SAMPLES {
                let excess = self.listen_rolling_samples.len() - LISTEN_ROLLING_MAX_SAMPLES;
                self.listen_rolling_samples.drain(..excess);
            }

            // Emit interim transcription every LISTEN_INTERIM_INTERVAL once we have enough audio.
            let enough_audio = self.listen_rolling_samples.len() >= LISTEN_INTERIM_MIN_SAMPLES;
            let interval_elapsed = self
                .listen_last_interim
                .map(|t| t.elapsed() >= LISTEN_INTERIM_INTERVAL)
                .unwrap_or(true);
            let rolling_rms = crate::silence_detector::calculate_rms(&self.listen_rolling_samples);
            let has_speech = rolling_rms > self.stt_energy_threshold;

            if enough_audio && interval_elapsed && has_speech {
                let interim_buf = AudioBuffer {
                    samples: self.listen_rolling_samples.clone(),
                    sample_rate: buffer.sample_rate,
                    channels: buffer.channels,
                };
                self.listen_last_interim = Some(std::time::Instant::now());
                match tokio::time::timeout(TRANSCRIPTION_TIMEOUT, self.transcribe(interim_buf))
                    .await
                {
                    Ok(Ok(result)) if !result.text.trim().is_empty() => {
                        let _ = bus
                            .broadcast(Event::Speech(SpeechEvent::ListenModeTranscription {
                                text: result.text,
                                is_final: false,
                                confidence: result.confidence,
                                session_id,
                            }))
                            .await;
                    }
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => tracing::warn!("listen[whisper/interim]: error: {}", e),
                    Err(_) => tracing::warn!("listen[whisper/interim]: timeout"),
                }
            }

            // VAD for final speech-end detection.
            if let Some(ready_buffer) =
                self.listen_mode_vad
                    .feed(&buffer.samples, buffer.sample_rate, buffer.channels)
            {
                match tokio::time::timeout(TRANSCRIPTION_TIMEOUT, self.transcribe(ready_buffer))
                    .await
                {
                    Ok(Ok(result)) if !result.text.trim().is_empty() => {
                        let _ = bus
                            .broadcast(Event::Speech(SpeechEvent::ListenModeTranscription {
                                text: result.text,
                                is_final: true,
                                confidence: result.confidence,
                                session_id,
                            }))
                            .await;
                        self.listen_rolling_samples.clear();
                        self.listen_last_interim = None;
                    }
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => tracing::warn!("listen[whisper]: transcription error: {}", e),
                    Err(_) => tracing::warn!("listen[whisper]: transcription timeout"),
                }
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

    /// Convert a string backend name to SttBackend enum.
    fn parse_backend_name(backend_str: &str) -> Result<SttBackend, String> {
        match backend_str.to_lowercase().as_str() {
            "whisper" => Ok(SttBackend::Whisper),
            "sherpa" => Ok(SttBackend::Sherpa),
            "parakeet" => Ok(SttBackend::Parakeet),
            "mock" => Ok(SttBackend::Mock),
            _ => Err(format!(
                "unknown backend '{}', expected: whisper, sherpa, parakeet, mock",
                backend_str
            )),
        }
    }

    /// Shut down a backend handle gracefully.
    async fn shutdown_backend(handle: SttBackendHandle) -> Result<(), SpeechError> {
        match handle {
            SttBackendHandle::CandleWhisper { tx } => {
                tx.send(WorkerCommand::Shutdown).map_err(|_| {
                    SpeechError::ChannelClosed("whisper worker tx closed".to_string())
                })?;
            }
            SttBackendHandle::Sherpa { tx } => {
                // Ignore send errors, channel may already be closed.
                let _ = tx.send(SherpaCmd::Shutdown);
            }
            SttBackendHandle::Parakeet { tx } => {
                // Ignore send errors, channel may already be closed.
                let _ = tx.send(ParakeetCommand::Shutdown);
            }
            SttBackendHandle::Mock => {}
        }
        Ok(())
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
                                tracing::warn!(
                                    "listen: new session {} requested while session {:?} active - stopping old session first",
                                    session_id,
                                    self.listen_session_id
                                );
                                // Shut down the previous sherpa worker if any.
                                if let Some(old_tx) = self.sherpa_worker_tx.take() {
                                    let _ = old_tx.send(SherpaCmd::Shutdown);
                                }
                                self.sherpa_listen_active = false;
                                // Shut down the previous parakeet worker if any.
                                if let Some(old_tx) = self.parakeet_worker_tx.take() {
                                    let _ = old_tx.send(ParakeetCommand::Shutdown);
                                }
                                self.parakeet_listen_active = false;
                                self.parakeet_chunk_accumulator.clear();
                                self.listen_audio_rx = None;
                                self.listen_audio_stream = None;
                            }

                            // Use Sherpa for listen-mode streaming only when Sherpa is the
                            // active, already-initialized backend. Cloning the sender is safe —
                            // mpsc::Sender is Clone and the worker loop handles commands serially.
                            let sherpa_worker = match (&self.backend, &self.backend_handle) {
                                (SttBackend::Sherpa, Some(SttBackendHandle::Sherpa { tx })) => {
                                    Some(tx.clone())
                                }
                                _ => None,
                            };

                            // Use Parakeet for listen-mode streaming when Parakeet is active.
                            let parakeet_worker = match (&self.backend, &self.backend_handle) {
                                (SttBackend::Parakeet, Some(SttBackendHandle::Parakeet { tx })) => {
                                    Some(tx.clone())
                                }
                                _ => None,
                            };

                            let (buffer_duration, backend_label) = if sherpa_worker.is_some() {
                                (LISTEN_SHERPA_BUFFER_DURATION_SECS, "sherpa-onnx")
                            } else if parakeet_worker.is_some() {
                                (LISTEN_PARAKEET_BUFFER_DURATION_SECS, "parakeet streaming")
                            } else {
                                (LISTEN_WHISPER_BUFFER_DURATION_SECS, "whisper fake-streaming")
                            };

                            self.sherpa_worker_tx = sherpa_worker;
                            self.sherpa_listen_active = self.sherpa_worker_tx.is_some();
                            self.listen_last_sherpa = None;
                            self.parakeet_worker_tx = parakeet_worker;
                            self.parakeet_listen_active = self.parakeet_worker_tx.is_some();
                            self.parakeet_chunk_accumulator.clear();

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
                                    self.listen_rolling_samples.clear();
                                    self.listen_last_interim = None;
                                    tracing::info!("listen: session {} started ({})", session_id, backend_label);
                                }
                                Err(e) => {
                                    tracing::error!("listen: failed to start audio capture: {}", e);
                                    // Clean up sherpa worker if audio failed.
                                    if let Some(tx) = self.sherpa_worker_tx.take() {
                                        let _ = tx.send(SherpaCmd::Shutdown);
                                    }
                                    self.sherpa_listen_active = false;
                                    // Clean up parakeet worker if audio failed.
                                    if let Some(tx) = self.parakeet_worker_tx.take() {
                                        let _ = tx.send(ParakeetCommand::Shutdown);
                                    }
                                    self.parakeet_listen_active = false;
                                    self.parakeet_chunk_accumulator.clear();
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
                                // Shut down sherpa worker if active.
                                if let Some(tx) = self.sherpa_worker_tx.take() {
                                    let _ = tx.send(SherpaCmd::Shutdown);
                                }
                                self.sherpa_listen_active = false;
                                self.listen_last_sherpa = None;
                                // Shut down parakeet worker if active.
                                if let Some(tx) = self.parakeet_worker_tx.take() {
                                    let _ = tx.send(ParakeetCommand::Shutdown);
                                }
                                self.parakeet_listen_active = false;
                                self.parakeet_chunk_accumulator.clear();
                                self.listen_session_id = None;
                                self.listen_audio_rx = None;
                                self.listen_audio_stream = None;
                                self.listen_mode_vad.reset();
                                self.listen_mode_active = false;
                                self.listen_rolling_samples.clear();
                                self.listen_last_interim = None;
                                tracing::info!("listen: session {} stopped", session_id);
                                let _ = bus
                                    .broadcast(Event::Speech(SpeechEvent::ListenModeStopped {
                                        session_id,
                                    }))
                                    .await;
                            }
                        }
                        Ok(Event::Speech(SpeechEvent::SttBackendSwitchRequested { backend })) => {
                            tracing::info!("stt: backend switch requested to '{}'", backend);

                            // Parse backend name
                            let new_backend = match Self::parse_backend_name(&backend) {
                                Ok(b) => b,
                                Err(e) => {
                                    tracing::warn!("stt: invalid backend name: {}", e);
                                    let _ = bus.broadcast(Event::Speech(SpeechEvent::SttBackendSwitchFailed {
                                        backend: backend.clone(),
                                        reason: e,
                                    })).await;
                                    continue;
                                }
                            };

                            // Set switching flag to pause audio processing
                            self.backend_switching = true;

                            // Preserve old backend for rollback
                            let old_backend = self.backend;
                            let old_backend_handle = self.backend_handle.take();

                            // Shut down current backend
                            if let Some(handle) = old_backend_handle {
                                tracing::debug!("stt: shutting down current backend");
                                if let Err(e) = Self::shutdown_backend(handle).await {
                                    tracing::warn!("stt: error shutting down old backend: {}", e);
                                }
                            }

                            // Update backend enum
                            self.backend = new_backend;

                            // Try to initialize new backend
                            match self.initialize_backend().await {
                                Ok(()) => {
                                    tracing::info!("stt: backend switched to {:?}", new_backend);
                                    let _ = bus.broadcast(Event::Speech(SpeechEvent::SttBackendSwitchCompleted {
                                        backend: backend.clone(),
                                    })).await;
                                }
                                Err(e) => {
                                    tracing::warn!("stt: backend switch failed ({}), rolling back to {:?}", e, old_backend);

                                    // Rollback to old backend
                                    self.backend = old_backend;
                                    match self.initialize_backend().await {
                                        Ok(()) => {
                                            tracing::info!("stt: rollback to {:?} successful", old_backend);
                                        }
                                        Err(rollback_err) => {
                                            tracing::error!("stt: rollback failed: {}", rollback_err);
                                        }
                                    }

                                    let _ = bus.broadcast(Event::Speech(SpeechEvent::SttBackendSwitchFailed {
                                        backend: backend.clone(),
                                        reason: e.to_string(),
                                    })).await;
                                }
                            }

                            // Clear switching flag to resume audio processing
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
                    // Skip always-on processing while listen mode is active, speech loop is disabled,
                    // or backend switch is in progress.
                    if let Some(buffer) = audio_buffer {
                        if self.backend_switching {
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
                    // Listen mode is independent of speech_loop_enabled (explicit user request)
                    // but is paused during backend switch.
                    if let (Some(buffer), Some(session_id)) = (listen_buffer, self.listen_session_id) {
                        if self.backend_switching {
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
        // Shut down sherpa worker if running.
        if let Some(tx) = self.sherpa_worker_tx.take() {
            let _ = tx.send(SherpaCmd::Shutdown);
        }
        // Shut down parakeet worker if running.
        if let Some(tx) = self.parakeet_worker_tx.take() {
            let _ = tx.send(ParakeetCommand::Shutdown);
        }
        self.parakeet_listen_active = false;
        self.parakeet_chunk_accumulator.clear();
        self.audio_rx = None;
        self.audio_stream = None;
        self.listen_audio_rx = None;
        self.listen_audio_stream = None;
        self.listen_session_id = None;

        match self.backend_handle.take() {
            Some(SttBackendHandle::CandleWhisper { tx }) => {
                let _ = tx.send(WorkerCommand::Shutdown);
            }
            Some(SttBackendHandle::Sherpa { tx }) => {
                let _ = tx.send(SherpaCmd::Shutdown);
            }
            Some(SttBackendHandle::Parakeet { tx }) => {
                let _ = tx.send(ParakeetCommand::Shutdown);
            }
            _ => {}
        }

        Ok(())
    }
}

enum SttBackendHandle {
    Mock,
    CandleWhisper {
        tx: std::sync::mpsc::Sender<WorkerCommand>,
    },
    /// Sherpa backend — sherpa-onnx Zipformer ONNX Transducer streaming STT.
    Sherpa {
        tx: std::sync::mpsc::Sender<SherpaCmd>,
    },
    /// Parakeet backend — NVIDIA Parakeet-EOU ONNX streaming STT.
    Parakeet {
        tx: std::sync::mpsc::Sender<ParakeetCommand>,
    },
}

enum WorkerCommand {
    Transcribe {
        samples: Vec<f32>,
        reply: oneshot::Sender<Result<TranscriptionResult, SpeechError>>,
    },
    Shutdown,
}

fn candle_worker_loop(
    mut model: crate::candle_whisper::CandleWhisperModel,
    rx: std::sync::mpsc::Receiver<WorkerCommand>,
) {
    while let Ok(command) = rx.recv() {
        match command {
            WorkerCommand::Shutdown => break,
            WorkerCommand::Transcribe { samples, reply } => {
                let result = model.transcribe(&samples).map(|text| {
                    let confidence = if text.trim().is_empty() {
                        0.0
                    } else {
                        (crate::silence_detector::calculate_rms(&samples) * 10.0).clamp(0.55, 0.99)
                    };
                    TranscriptionResult { text, confidence }
                });
                let _ = reply.send(result);
            }
        }
    }
}

/// Commands for the sherpa-onnx worker thread.
enum SherpaCmd {
    Decode {
        samples: Vec<f32>,
        reply: oneshot::Sender<String>,
    },
    Shutdown,
}

/// Worker loop for blocking sherpa-onnx decode calls.
fn sherpa_worker_loop(
    mut model: crate::sherpa_stt::SherpaZipformerStt,
    rx: std::sync::mpsc::Receiver<SherpaCmd>,
) {
    while let Ok(cmd) = rx.recv() {
        match cmd {
            SherpaCmd::Shutdown => break,
            SherpaCmd::Decode { samples, reply } => {
                let text = model.decode_chunk(samples);
                let _ = reply.send(text);
            }
        }
    }
}

/// Commands for the Parakeet worker thread.
enum ParakeetCommand {
    Transcribe {
        samples: Vec<i16>,
        reply: oneshot::Sender<Result<TranscriptionResult, SpeechError>>,
    },
    /// Streaming 160ms chunk for /listen mode.
    /// Uses f32 directly — no lossy i16 round-trip.
    Chunk {
        samples: Vec<f32>,
        reply: oneshot::Sender<String>,
    },
    Shutdown,
}

/// Worker loop for blocking Parakeet decode calls.
fn parakeet_worker_loop(
    mut model: crate::parakeet_stt::ParakeetStt,
    rx: std::sync::mpsc::Receiver<ParakeetCommand>,
) {
    while let Ok(command) = rx.recv() {
        match command {
            ParakeetCommand::Shutdown => break,
            ParakeetCommand::Chunk { samples, reply } => {
                let text = model
                    .decode_chunk_f32(&samples)
                    .unwrap_or_default();
                let _ = reply.send(text);
            }
            ParakeetCommand::Transcribe { samples, reply } => {
                let result = model.decode_chunk(&samples).map(|text| {
                    let confidence = if text.trim().is_empty() {
                        0.0
                    } else {
                        // Calculate confidence from audio energy (similar to Whisper)
                        let samples_f32: Vec<f32> =
                            samples.iter().map(|&s| s as f32 / 32768.0).collect();
                        (crate::silence_detector::calculate_rms(&samples_f32) * 10.0)
                            .clamp(0.55, 0.99)
                    };
                    TranscriptionResult { text, confidence }
                });
                let _ = reply.send(result);
            }
        }
    }
}

struct TranscriptionResult {
    text: String,
    confidence: f32,
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
