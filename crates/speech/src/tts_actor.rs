//! TTS Actor - text-to-speech generation and playback.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use tokio::sync::{broadcast, mpsc};

use bus::events::inference::InferenceEvent;
use bus::{Actor, ActorError, Event, EventBus, SoulEvent, SpeechEvent, SystemEvent};

use crate::audio_output::AudioOutput;
use crate::error::SpeechError;
use crate::TtsBackend;

const MAX_QUEUE_SIZE: usize = 10;

/// TTS Actor - handles text-to-speech generation and playback.
///
/// Pipeline: SpeakRequested -> queued FIFO -> backend synthesis -> playback -> SpeechOutputCompleted
pub struct TtsActor {
    backend_preference: TtsBackend,
    bus: Option<Arc<EventBus>>,
    bus_rx: Option<broadcast::Receiver<Event>>,
    request_tx: Option<mpsc::Sender<SpeakRequest>>,
    request_rx: Option<mpsc::Receiver<SpeakRequest>>,
    active_backend: Option<ActiveTtsBackend>,
    tts_voice: Option<String>,
    /// User-configured base speaking rate.
    base_tts_rate: f32,
    /// Current personality state as (warmth, assertiveness, brevity).
    current_personality: Option<(u8, u8, u8)>,
    model_dir: Option<PathBuf>,
    interrupt: Arc<AtomicBool>,
    /// Synthesized audio indexed by sentence_index, awaiting ordered playback.
    streaming_pending: BTreeMap<u64, SynthResult>,
    /// Next sentence_index to play in the streaming queue.
    streaming_next_play: u64,
    /// Request ID of the active streaming session (None when idle).
    streaming_request_id: Option<u64>,
    /// Channel for synthesis tasks to send completed audio back to the main loop.
    synth_tx: mpsc::Sender<SynthResult>,
    synth_rx: Option<mpsc::Receiver<SynthResult>>,
    /// Maximum streaming queue depth.
    tts_queue_depth: usize,
}

#[derive(Debug)]
struct SpeakRequest {
    text: String,
    request_id: u64,
}

/// Result of a background synthesis task for a streaming sentence.
struct SynthResult {
    request_id: u64,
    sentence_index: u64,
    /// Synthesized PCM audio. None for SystemPlatform (uses direct TTS speak).
    audio: Option<Vec<i16>>,
    /// Original sentence text (for SystemPlatform fallback).
    sentence: String,
}

#[derive(Debug, Clone)]
enum ActiveTtsBackend {
    Piper { model: Option<PathBuf> },
    SystemPlatform,
    Mock,
}

impl TtsActor {
    /// Create a new TTS actor with the specified backend preference.
    pub fn new(backend: TtsBackend) -> Self {
        let (request_tx, request_rx) = mpsc::channel(MAX_QUEUE_SIZE);
        let (synth_tx, synth_rx) = mpsc::channel(32);

        Self {
            backend_preference: backend,
            bus: None,
            bus_rx: None,
            request_tx: Some(request_tx),
            request_rx: Some(request_rx),
            active_backend: None,
            tts_voice: None,
            base_tts_rate: 1.0,
            current_personality: None,
            model_dir: None,
            interrupt: Arc::new(AtomicBool::new(false)),
            streaming_pending: BTreeMap::new(),
            streaming_next_play: 0,
            streaming_request_id: None,
            synth_tx,
            synth_rx: Some(synth_rx),
            tts_queue_depth: 5,
        }
    }

    /// Set TTS voice (backend-dependent; Piper uses this as model path/name).
    pub fn with_voice(mut self, voice: Option<String>) -> Self {
        self.tts_voice = voice;
        self
    }

    /// Set TTS rate (0.5-2.0 speed multiplier).
    pub fn with_rate(mut self, rate: f32) -> Self {
        self.base_tts_rate = rate.clamp(0.5, 2.0);
        self
    }

    fn effective_tts_rate(&self) -> f32 {
        (self.base_tts_rate * self.personality_rate_modifier()).clamp(0.5, 2.0)
    }

    /// Compute a subtle rate multiplier from personality values.
    ///
    /// - assertiveness higher => slightly faster
    /// - brevity higher => slightly faster
    /// - warmth higher => slightly calmer/slower
    fn personality_rate_modifier(&self) -> f32 {
        let Some((warmth, assertiveness, brevity)) = self.current_personality else {
            return 1.0;
        };

        let warmth_delta = (warmth as f32 - 50.0) / 50.0;
        let assertiveness_delta = (assertiveness as f32 - 50.0) / 50.0;
        let brevity_delta = (brevity as f32 - 50.0) / 50.0;

        let modifier = 1.0
            + (assertiveness_delta * 0.08)
            + (brevity_delta * 0.06)
            - (warmth_delta * 0.06);

        modifier.clamp(0.8, 1.2)
    }

    /// Set model directory for resolving voice model paths.
    pub fn with_model_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.model_dir = dir;
        self
    }

    /// Configure the maximum streaming synthesis queue depth.
    pub fn with_queue_depth(mut self, depth: usize) -> Self {
        self.tts_queue_depth = depth.max(1);
        self
    }

    async fn initialize_backend(&mut self) -> Result<(), SpeechError> {
        let chosen = match self.backend_preference {
            TtsBackend::Piper => {
                if is_piper_available() {
                    // Validate that audio output can be opened before accepting requests.
                    let _ = tokio::task::spawn_blocking(AudioOutput::new)
                        .await
                        .map_err(|e| {
                            SpeechError::AudioPlaybackFailed(format!("task join failed: {e}"))
                        })??;

                    // Resolve model path against model_dir if needed.
                    let model = self.tts_voice.as_ref().map(|voice| {
                        let path = PathBuf::from(voice);
                        if path.is_absolute() || path.exists() {
                            path
                        } else if let Some(ref dir) = self.model_dir {
                            dir.join(voice)
                        } else {
                            path
                        }
                    });

                    ActiveTtsBackend::Piper { model }
                } else if is_system_tts_available() {
                    ActiveTtsBackend::SystemPlatform
                } else {
                    return Err(SpeechError::TtsInitFailed(
                        "Piper unavailable and system TTS unavailable".to_string(),
                    ));
                }
            }
            TtsBackend::SystemPlatform => {
                if is_system_tts_available() {
                    ActiveTtsBackend::SystemPlatform
                } else {
                    return Err(SpeechError::TtsInitFailed(
                        "system platform TTS unavailable".to_string(),
                    ));
                }
            }
            TtsBackend::Mock => ActiveTtsBackend::Mock,
        };

        self.active_backend = Some(chosen);
        Ok(())
    }

    async fn process_request(&self, request: SpeakRequest) {
        let Some(bus) = self.bus.as_ref() else {
            return;
        };

        let event = match self.generate_and_play(&request.text).await {
            Ok(()) => Event::Speech(SpeechEvent::SpeechOutputCompleted {
                request_id: request.request_id,
            }),
            Err(e) => Event::Speech(SpeechEvent::SpeechFailed {
                reason: e.to_string(),
                request_id: request.request_id,
            }),
        };

        let _ = bus.broadcast(event).await;
    }

    async fn generate_and_play(&self, text: &str) -> Result<(), SpeechError> {
        let backend = self
            .active_backend
            .clone()
            .ok_or_else(|| SpeechError::TtsInitFailed("backend not initialized".to_string()))?;

        let text = text.to_string();
        let rate = self.effective_tts_rate();

        tracing::debug!(
            "tts: generate_and_play — backend={:?} text_len={} rate={}",
            backend,
            text.len(),
            rate
        );

        match backend {
            ActiveTtsBackend::Mock => {
                // Deterministic mock synthesis for tests (no hardware dependency).
                tokio::time::sleep(Duration::from_millis(30)).await;
                tracing::debug!("tts: mock playback complete");
                Ok(())
            }
            ActiveTtsBackend::SystemPlatform => {
                tracing::debug!("tts: using system platform TTS");
                let result = tokio::task::spawn_blocking(move || {
                    tracing::debug!("tts: creating default Tts instance");
                    let mut tts = tts::Tts::default().map_err(|e| {
                        SpeechError::SpeechGenerationFailed(format!("Tts::default failed: {e}"))
                    })?;

                    tracing::debug!("tts: setting rate to {}", rate);
                    tts.set_rate(rate).map_err(|e| {
                        SpeechError::SpeechGenerationFailed(format!("set_rate failed: {e}"))
                    })?;

                    tracing::debug!("tts: calling speak() with text_len={}", text.len());
                    tts.speak(&text, false).map_err(|e| {
                        SpeechError::SpeechGenerationFailed(format!("speak failed: {e}"))
                    })?;

                    tracing::debug!("tts: system TTS speak() returned successfully");
                    Ok::<(), SpeechError>(())
                })
                .await
                .map_err(|e| {
                    SpeechError::SpeechGenerationFailed(format!("task join failed: {e}"))
                })?;

                tracing::info!("tts: system platform playback complete");
                result
            }
            ActiveTtsBackend::Piper { model } => {
                tracing::debug!("tts: using Piper backend with model={:?}", model);
                tokio::task::spawn_blocking(move || {
                    let samples = synthesize_with_piper(&text, model.as_ref(), rate)?;
                    tracing::debug!("tts: piper synthesis complete — {} samples", samples.len());
                    let output = AudioOutput::new()?;
                    tracing::debug!("tts: AudioOutput created, starting playback");
                    output.play_pcm16_mono_22050(&samples)?;
                    tracing::info!("tts: piper playback complete");
                    Ok::<(), SpeechError>(())
                })
                .await
                .map_err(|e| {
                    SpeechError::SpeechGenerationFailed(format!("task join failed: {e}"))
                })?
            }
        }
    }

    /// Handle high-priority interrupt requests (request_id == 0).
    fn handle_interrupt(&mut self) {
        // Set interrupt flag.
        self.interrupt.store(true, Ordering::SeqCst);

        // Clear the queue by draining all pending requests.
        if let Some(rx) = &mut self.request_rx {
            while rx.try_recv().is_ok() {}
        }

        // Reset interrupt flag for next request.
        self.interrupt.store(false, Ordering::SeqCst);
    }

    /// Play a synthesized result (PCM for Piper/Mock, or direct TTS for SystemPlatform).
    /// Runs in spawn_blocking. Returns error string on failure.
    async fn play_synth_result(&self, result: &SynthResult) -> Result<(), SpeechError> {
        let backend = self.active_backend.clone();
        let sentence = result.sentence.clone();
        let rate = self.effective_tts_rate();
        let audio = result.audio.clone();

        tokio::task::spawn_blocking(move || {
            match (&backend, &audio) {
                (_, Some(samples)) if !samples.is_empty() => {
                    // Piper PCM
                    let output = AudioOutput::new()?;
                    output.play_pcm16_mono_22050(samples)
                }
                (_, Some(_)) => {
                    // Mock backend — empty audio, no-op
                    Ok(())
                }
                (Some(ActiveTtsBackend::SystemPlatform), None) => {
                    // System TTS: speak sentence directly
                    let mut tts = tts::Tts::default().map_err(|e| {
                        SpeechError::SpeechGenerationFailed(format!("Tts::default: {e}"))
                    })?;
                    tts.set_rate(rate).map_err(|e| {
                        SpeechError::SpeechGenerationFailed(format!("set_rate: {e}"))
                    })?;
                    tts.speak(&sentence, false)
                        .map_err(|e| SpeechError::SpeechGenerationFailed(format!("speak: {e}")))?;
                    Ok(())
                }
                _ => Ok(()), // backend not set or no audio and not system TTS
            }
        })
        .await
        .map_err(|e| SpeechError::SpeechGenerationFailed(format!("task join: {e}")))?
    }

    /// Spawn a background synthesis task for a streaming sentence.
    /// The result is sent to `synth_tx` when complete.
    fn spawn_sentence_synthesis(&self, sentence: String, request_id: u64, sentence_index: u64) {
        let backend = self.active_backend.clone();
        let rate = self.effective_tts_rate();
        let synth_tx = self.synth_tx.clone();

        tokio::task::spawn_blocking(move || {
            let audio = match &backend {
                Some(ActiveTtsBackend::Piper { model }) => {
                    match synthesize_with_piper(&sentence, model.as_ref(), rate) {
                        Ok(samples) => Some(samples),
                        Err(e) => {
                            tracing::warn!(
                                "tts: synthesis failed for sentence_index={}: {}",
                                sentence_index,
                                e
                            );
                            Some(vec![]) // empty = no-op on playback
                        }
                    }
                }
                Some(ActiveTtsBackend::Mock) => {
                    // Mock: deterministic short delay, no real audio
                    std::thread::sleep(Duration::from_millis(20));
                    Some(vec![]) // empty PCM for mock
                }
                Some(ActiveTtsBackend::SystemPlatform) => {
                    None // SystemPlatform speaks in the playback step
                }
                None => Some(vec![]), // no backend
            };

            let result = SynthResult {
                request_id,
                sentence_index,
                audio,
                sentence,
            };

            if let Err(e) = synth_tx.blocking_send(result) {
                tracing::warn!("tts: failed to send synth result: {}", e);
            }
        });
    }
}

#[async_trait]
impl Actor for TtsActor {
    fn name(&self) -> &'static str {
        "tts"
    }

    async fn start(&mut self, bus: Arc<EventBus>) -> Result<(), ActorError> {
        tracing::info!(
            "tts: initializing backend (preference: {:?})",
            self.backend_preference
        );
        if let Err(e) = self.initialize_backend().await {
            tracing::error!("tts: backend init failed: {}", e);
            let _ = bus
                .broadcast(Event::Speech(SpeechEvent::SpeechFailed {
                    reason: format!("TTS init failed: {e}"),
                    request_id: 0,
                }))
                .await;
            return Err(ActorError::StartupFailed(e.to_string()));
        }
        tracing::info!("tts: backend initialized — {:?}", self.active_backend);

        self.bus_rx = Some(bus.subscribe_broadcast());
        self.bus = Some(Arc::clone(&bus));

        bus.broadcast(Event::System(SystemEvent::ActorReady { actor_name: "tts" }))
            .await
            .map_err(|e| ActorError::StartupFailed(format!("broadcast ActorReady failed: {e}")))?;

        Ok(())
    }

    async fn run(&mut self) -> Result<(), ActorError> {
        let mut bus_rx = self.bus_rx.take().ok_or_else(|| {
            ActorError::RuntimeError("bus_rx not initialized in start()".to_string())
        })?;

        let mut request_rx = self
            .request_rx
            .take()
            .ok_or_else(|| ActorError::RuntimeError("request_rx not initialized".to_string()))?;

        let mut synth_rx = self
            .synth_rx
            .take()
            .ok_or_else(|| ActorError::RuntimeError("synth_rx not initialized".to_string()))?;

        let request_tx = self
            .request_tx
            .as_ref()
            .ok_or_else(|| ActorError::RuntimeError("request_tx not initialized".to_string()))?
            .clone();

        loop {
            tokio::select! {
                biased;
                bus_event = bus_rx.recv() => {
                    match bus_event {
                        Ok(Event::System(SystemEvent::ShutdownSignal)) => {
                            break;
                        }
                        Ok(Event::Soul(SoulEvent::PersonalityUpdated(update))) => {
                            // Soul currently emits `rate`, `warmth`, and `verbosity`.
                            // Until assertiveness/brevity are added upstream, derive:
                            // assertiveness from normalized rate, brevity as inverse verbosity.
                            let assertiveness = (((update.rate.clamp(0.5, 2.0) - 0.5) / 1.5)
                                * 100.0)
                                .round() as u8;
                            let brevity = 100u8.saturating_sub(update.verbosity.min(100));

                            self.current_personality = Some((
                                update.warmth.min(100),
                                assertiveness.min(100),
                                brevity,
                            ));

                            tracing::info!(
                                "tts: personality updated — rate={:.2}, warmth={}, verbosity={}, modifier={:.3}, effective_rate={:.3}",
                                update.rate,
                                update.warmth,
                                update.verbosity,
                                self.personality_rate_modifier(),
                                self.effective_tts_rate(),
                            );
                        }
                        Ok(Event::Inference(InferenceEvent::InferenceSentenceReady {
                            sentence,
                            request_id,
                            sentence_index,
                        })) => {
                            // Ignore if backend not initialized
                            if self.active_backend.is_none() {
                                continue;
                            }

                            // Check if this is a new stream (reset queue if different request)
                            if self.streaming_request_id.is_some()
                                && self.streaming_request_id != Some(request_id)
                            {
                                // Different stream: clear old queue
                                self.streaming_pending.clear();
                                self.streaming_next_play = 0;
                            }
                            self.streaming_request_id = Some(request_id);

                            // Enforce queue depth: drop oldest unstartable entry if at cap
                            if self.streaming_pending.len() >= self.tts_queue_depth {
                                tracing::warn!(
                                    "tts: streaming queue full (depth={}), dropping oldest pending sentence",
                                    self.tts_queue_depth
                                );
                                if let Some(oldest_key) = self.streaming_pending.keys().next().copied() {
                                    self.streaming_pending.remove(&oldest_key);
                                }
                            }

                            // Spawn synthesis in background
                            self.spawn_sentence_synthesis(sentence, request_id, sentence_index);
                        }
                        Ok(Event::Inference(InferenceEvent::InferenceStreamCompleted {
                            request_id,
                            total_sentence_count,
                            ..
                        })) => {
                            if self.streaming_request_id != Some(request_id) {
                                continue;
                            }

                            tracing::info!(
                                "tts: stream completed for request_id={}, expected {} sentences",
                                request_id,
                                total_sentence_count,
                            );

                            // Drain synth results for a bounded time, then play all ready entries in order.
                            // Wait up to 30s total for pending synthesis tasks.
                            let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
                            let expected_count = total_sentence_count;

                            // Drain remaining synth results until we have all sentences or timeout
                            while self.streaming_pending.len() < expected_count as usize {
                                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                                if remaining.is_zero() {
                                    tracing::warn!("tts: timeout waiting for synthesis tasks to complete");
                                    break;
                                }

                                match tokio::time::timeout(remaining, synth_rx.recv()).await {
                                    Ok(Some(result)) if result.request_id == request_id => {
                                        self.streaming_pending.insert(result.sentence_index, result);
                                    }
                                    Ok(Some(_)) => {} // stale result from previous stream
                                    Ok(None) | Err(_) => break, // channel closed or timeout
                                }
                            }

                            // Play all ready entries in order
                            while let Some(result) = self.streaming_pending.remove(&self.streaming_next_play) {
                                if let Err(e) = self.play_synth_result(&result).await {
                                    tracing::warn!("tts: playback error for sentence_index={}: {}", self.streaming_next_play, e);
                                }
                                self.streaming_next_play += 1;
                            }

                            // Reset streaming state
                            self.streaming_pending.clear();
                            self.streaming_next_play = 0;
                            self.streaming_request_id = None;
                        }
                        Ok(Event::Speech(SpeechEvent::TranscriptionCompleted { .. })) => {
                            if self.streaming_request_id.is_some() {
                                tracing::info!("tts: new transcription received - clearing streaming queue");
                                self.streaming_pending.clear();
                                self.streaming_next_play = 0;
                                self.streaming_request_id = None;
                                // Drain synth_rx to discard stale in-flight results
                                while synth_rx.try_recv().is_ok() {}
                            }
                        }
                        Ok(Event::Speech(SpeechEvent::SpeakRequested { text, request_id })) => {
                            tracing::info!("tts: SpeakRequested request_id={} text_len={}", request_id, text.len());
                            // High-priority interrupt: request_id == 0 clears queue.
                            if request_id == 0 {
                                self.handle_interrupt();
                            }

                            let request = SpeakRequest { text, request_id };
                            if request_tx.try_send(request).is_err() {
                                tracing::warn!("tts: queue full, dropping request_id={}", request_id);
                                if let Some(bus) = &self.bus {
                                    let _ = bus.broadcast(Event::Speech(SpeechEvent::SpeechFailed {
                                        reason: format!("queue full (max {MAX_QUEUE_SIZE} requests)"),
                                        request_id,
                                    })).await;
                                }
                            }
                        }
                        Ok(_) => {}
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => {
                            return Err(ActorError::ChannelClosed("bus_rx closed".to_string()));
                        }
                    }
                }
                Some(request) = request_rx.recv() => {
                    self.process_request(request).await;
                }
                Some(synth_result) = synth_rx.recv() => {
                    // Ignore stale results from previous streams
                    if Some(synth_result.request_id) != self.streaming_request_id {
                        continue;
                    }

                    let sentence_index = synth_result.sentence_index;
                    self.streaming_pending.insert(sentence_index, synth_result);

                    // Play consecutive ready entries starting from next_play
                    while let Some(result) = self.streaming_pending.remove(&self.streaming_next_play) {
                        if let Err(e) = self.play_synth_result(&result).await {
                            tracing::warn!(
                                "tts: playback error sentence_index={}: {}",
                                self.streaming_next_play,
                                e
                            );
                        }
                        self.streaming_next_play += 1;
                    }
                }
            }
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), ActorError> {
        self.active_backend = None;
        Ok(())
    }
}

fn is_piper_available() -> bool {
    Command::new("piper")
        .arg("--help")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn is_system_tts_available() -> bool {
    tts::Tts::default().is_ok()
}

fn synthesize_with_piper(
    text: &str,
    model: Option<&PathBuf>,
    rate: f32,
) -> Result<Vec<i16>, SpeechError> {
    let temp_path = temp_wav_path();

    let mut cmd = Command::new("piper");
    if let Some(model_path) = model {
        cmd.arg("--model").arg(model_path);
    }
    cmd.arg("--output_file")
        .arg(&temp_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| SpeechError::SpeechGenerationFailed(format!("failed to start piper: {e}")))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes()).map_err(|e| {
            SpeechError::SpeechGenerationFailed(format!("failed to write text to piper: {e}"))
        })?;
    }

    let output = child.wait_with_output().map_err(|e| {
        SpeechError::SpeechGenerationFailed(format!("failed waiting for piper process: {e}"))
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(SpeechError::SpeechGenerationFailed(format!(
            "piper synthesis failed: {}",
            stderr.trim()
        )));
    }

    let wav_bytes = std::fs::read(&temp_path).map_err(|e| {
        SpeechError::SpeechGenerationFailed(format!("failed to read piper output wav: {e}"))
    })?;
    let _ = std::fs::remove_file(&temp_path);

    let (mut samples, sample_rate) = parse_pcm16_wav_mono(&wav_bytes)?;

    if (rate - 1.0).abs() > f32::EPSILON {
        samples = time_scale_pcm_i16(&samples, rate);
    }

    if sample_rate != 22_050 {
        samples = resample_pcm_i16(&samples, sample_rate, 22_050);
    }

    Ok(samples)
}

fn temp_wav_path() -> PathBuf {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_nanos();
    std::env::temp_dir().join(format!("sena_tts_{ts}_{}.wav", std::process::id()))
}

fn parse_pcm16_wav_mono(bytes: &[u8]) -> Result<(Vec<i16>, u32), SpeechError> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(SpeechError::SpeechGenerationFailed(
            "invalid WAV header from Piper".to_string(),
        ));
    }

    let channels = u16::from_le_bytes([bytes[22], bytes[23]]);
    let sample_rate = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]);
    let bits_per_sample = u16::from_le_bytes([bytes[34], bytes[35]]);

    if channels != 1 || bits_per_sample != 16 {
        return Err(SpeechError::SpeechGenerationFailed(format!(
            "unsupported WAV format from Piper: channels={channels}, bits={bits_per_sample}"
        )));
    }

    // Find the first data chunk.
    let mut idx = 12usize;
    let mut data_offset = None;
    let mut data_len = 0usize;

    while idx + 8 <= bytes.len() {
        let chunk_id = &bytes[idx..idx + 4];
        let chunk_size = u32::from_le_bytes([
            bytes[idx + 4],
            bytes[idx + 5],
            bytes[idx + 6],
            bytes[idx + 7],
        ]) as usize;
        idx += 8;

        if chunk_id == b"data" {
            data_offset = Some(idx);
            data_len = chunk_size;
            break;
        }

        idx = idx.saturating_add(chunk_size);
    }

    let Some(offset) = data_offset else {
        return Err(SpeechError::SpeechGenerationFailed(
            "WAV data chunk missing".to_string(),
        ));
    };

    if offset + data_len > bytes.len() || !data_len.is_multiple_of(2) {
        return Err(SpeechError::SpeechGenerationFailed(
            "invalid WAV data chunk size".to_string(),
        ));
    }

    let mut samples = Vec::with_capacity(data_len / 2);
    for chunk in bytes[offset..offset + data_len].chunks_exact(2) {
        samples.push(i16::from_le_bytes([chunk[0], chunk[1]]));
    }

    Ok((samples, sample_rate))
}

fn time_scale_pcm_i16(samples: &[i16], rate: f32) -> Vec<i16> {
    if samples.is_empty() {
        return Vec::new();
    }

    let clamped = rate.clamp(0.5, 2.0);
    let out_len = ((samples.len() as f32) / clamped).max(1.0) as usize;
    let mut out = Vec::with_capacity(out_len);

    for i in 0..out_len {
        let src_idx = ((i as f32) * clamped) as usize;
        out.push(samples[src_idx.min(samples.len() - 1)]);
    }

    out
}

fn resample_pcm_i16(samples: &[i16], src_rate: u32, dst_rate: u32) -> Vec<i16> {
    if samples.is_empty() || src_rate == dst_rate {
        return samples.to_vec();
    }

    let ratio = dst_rate as f32 / src_rate as f32;
    let out_len = (samples.len() as f32 * ratio).max(1.0) as usize;
    let mut out = Vec::with_capacity(out_len);

    for i in 0..out_len {
        let src_pos = (i as f32) / ratio;
        let src_idx = src_pos as usize;
        out.push(samples[src_idx.min(samples.len() - 1)]);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn personality_rate_modifier_reflects_personality_and_is_bounded() {
        let mut actor = TtsActor::new(TtsBackend::Mock).with_rate(1.0);

        assert!((actor.personality_rate_modifier() - 1.0).abs() < f32::EPSILON);

        actor.current_personality = Some((20, 90, 90));
        let faster = actor.personality_rate_modifier();
        assert!(faster > 1.0);

        actor.current_personality = Some((100, 0, 0));
        let calmer = actor.personality_rate_modifier();
        assert!(calmer < 1.0);

        actor.current_personality = Some((0, 100, 100));
        let max_modifier = actor.personality_rate_modifier();
        assert!(max_modifier <= 1.2);

        actor.current_personality = Some((100, 0, 0));
        let min_modifier = actor.personality_rate_modifier();
        assert!(min_modifier >= 0.8);
    }

    #[tokio::test]
    async fn tts_actor_boots_and_stops_cleanly() {
        let bus = Arc::new(EventBus::new());
        let mut actor = TtsActor::new(TtsBackend::Mock);

        actor
            .start(Arc::clone(&bus))
            .await
            .expect("mock TTS starts");

        let run_handle = tokio::spawn(async move { actor.run().await });

        bus.broadcast(Event::System(SystemEvent::ShutdownSignal))
            .await
            .expect("shutdown broadcast succeeds");

        let result = run_handle.await.expect("run task joins");
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn mock_backend_emits_speech_output_completed() {
        let bus = Arc::new(EventBus::new());
        let mut actor = TtsActor::new(TtsBackend::Mock);

        actor
            .start(Arc::clone(&bus))
            .await
            .expect("mock TTS starts");

        let mut rx = bus.subscribe_broadcast();
        let run_handle = tokio::spawn(async move { actor.run().await });

        bus.broadcast(Event::Speech(SpeechEvent::SpeakRequested {
            text: "hello world".to_string(),
            request_id: 1,
        }))
        .await
        .expect("speak request broadcast succeeds");

        let mut completed = false;
        for _ in 0..20 {
            if let Ok(Event::Speech(SpeechEvent::SpeechOutputCompleted { request_id: 1 })) =
                rx.try_recv()
            {
                completed = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(completed, "SpeechOutputCompleted not received");

        bus.broadcast(Event::System(SystemEvent::ShutdownSignal))
            .await
            .expect("shutdown broadcast succeeds");

        run_handle
            .await
            .expect("run task joins")
            .expect("actor run succeeds");
    }

    #[tokio::test]
    async fn queueing_works_fifo_order() {
        let bus = Arc::new(EventBus::new());
        let mut actor = TtsActor::new(TtsBackend::Mock);

        actor
            .start(Arc::clone(&bus))
            .await
            .expect("mock TTS starts");

        let mut rx = bus.subscribe_broadcast();
        let run_handle = tokio::spawn(async move { actor.run().await });

        for i in 1..=3 {
            bus.broadcast(Event::Speech(SpeechEvent::SpeakRequested {
                text: format!("message {i}"),
                request_id: i,
            }))
            .await
            .expect("speak request broadcast succeeds");
        }

        let mut completed_ids = Vec::new();
        for _ in 0..80 {
            if let Ok(Event::Speech(SpeechEvent::SpeechOutputCompleted { request_id })) =
                rx.try_recv()
            {
                completed_ids.push(request_id);
                if completed_ids.len() == 3 {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        assert_eq!(completed_ids, vec![1, 2, 3]);

        bus.broadcast(Event::System(SystemEvent::ShutdownSignal))
            .await
            .expect("shutdown broadcast succeeds");

        run_handle
            .await
            .expect("run task joins")
            .expect("actor run succeeds");
    }

    #[tokio::test]
    async fn queue_full_rejects_request() {
        let bus = Arc::new(EventBus::new());
        let mut actor = TtsActor::new(TtsBackend::Mock);

        actor
            .start(Arc::clone(&bus))
            .await
            .expect("mock TTS starts");

        let mut rx = bus.subscribe_broadcast();
        let run_handle = tokio::spawn(async move { actor.run().await });

        for i in 1..=25 {
            bus.broadcast(Event::Speech(SpeechEvent::SpeakRequested {
                text: format!("message {i}"),
                request_id: i,
            }))
            .await
            .expect("speak request broadcast succeeds");
        }

        let mut found_queue_full = false;
        for _ in 0..120 {
            if let Ok(Event::Speech(SpeechEvent::SpeechFailed { reason, .. })) = rx.try_recv() {
                if reason.contains("queue full") {
                    found_queue_full = true;
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        assert!(found_queue_full, "expected queue full rejection event");

        bus.broadcast(Event::System(SystemEvent::ShutdownSignal))
            .await
            .expect("shutdown broadcast succeeds");

        run_handle
            .await
            .expect("run task joins")
            .expect("actor run succeeds");
    }
}
