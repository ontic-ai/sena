//! STT Actor - speech-to-text processing.

#[cfg(feature = "whisper")]
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
#[cfg(feature = "whisper")]
use tokio::sync::oneshot;
use tokio::sync::{broadcast, mpsc};

use bus::{Actor, ActorError, Event, EventBus, SpeechEvent, SystemEvent};

use crate::audio_input::{AudioInputConfig, AudioInputStream};
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
    whisper_model_path: Option<String>,
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
            whisper_model_path,
        }
    }

    fn next_request_id(&mut self) -> u64 {
        let id = self.request_id_counter;
        self.request_id_counter = self.request_id_counter.saturating_add(1);
        id
    }

    async fn initialize_backend(&mut self) -> Result<(), SpeechError> {
        // Keep config field actively referenced in non-whisper builds.
        let _ = self.whisper_model_path.as_deref();

        let handle = match self.backend {
            SttBackend::Mock => SttBackendHandle::Mock,
            SttBackend::WhisperCpp => self.initialize_whisper_backend().await?,
        };

        self.backend_handle = Some(handle);
        Ok(())
    }

    async fn initialize_whisper_backend(&self) -> Result<SttBackendHandle, SpeechError> {
        #[cfg(feature = "whisper")]
        {
            let model_path = resolve_model_path(self.whisper_model_path.as_deref())?;
            if !Path::new(&model_path).exists() {
                return Err(SpeechError::SttInitFailed(format!(
                    "whisper model not found at {}",
                    model_path
                )));
            }

            let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<WorkerCommand>();
            tokio::task::spawn_blocking(move || {
                whisper_worker_loop(&model_path, cmd_rx);
            });

            Ok(SttBackendHandle::WhisperWorker { tx: cmd_tx })
        }

        #[cfg(not(feature = "whisper"))]
        {
            Err(SpeechError::SttInitFailed(
                "whisper feature not enabled for speech crate".to_string(),
            ))
        }
    }

    fn maybe_start_audio_capture(&mut self) -> Result<(), SpeechError> {
        if !self.voice_always_listening {
            return Ok(());
        }

        let config = AudioInputConfig {
            sample_rate: 16_000,
            buffer_duration_secs: DEFAULT_BUFFER_DURATION_SECS,
            energy_threshold: self.stt_energy_threshold,
        };

        let (stream, rx) = AudioInputStream::start(config)?;
        self.audio_stream = Some(stream);
        self.audio_rx = Some(rx);
        Ok(())
    }

    async fn transcribe(&self, buffer: AudioBuffer) -> Result<TranscriptionResult, SpeechError> {
        match self.backend_handle.as_ref() {
            Some(SttBackendHandle::Mock) => {
                let rms = calculate_rms(&buffer.samples);
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
            #[cfg(feature = "whisper")]
            Some(SttBackendHandle::WhisperWorker { tx }) => {
                let (reply_tx, reply_rx) = oneshot::channel();
                tx.send(WorkerCommand::Transcribe {
                    samples: buffer.samples,
                    reply: reply_tx,
                })
                .map_err(|e| {
                    SpeechError::TranscriptionFailed(format!(
                        "whisper worker channel send failed: {}",
                        e
                    ))
                })?;

                reply_rx.await.map_err(|e| {
                    SpeechError::TranscriptionFailed(format!("whisper worker reply failed: {}", e))
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
        match tokio::time::timeout(TRANSCRIPTION_TIMEOUT, self.transcribe(buffer)).await {
            Ok(Ok(result)) => {
                if result.confidence >= 0.5 {
                    let _ = bus
                        .broadcast(Event::Speech(SpeechEvent::TranscriptionCompleted {
                            text: result.text,
                            confidence: result.confidence,
                            request_id,
                        }))
                        .await;
                } else {
                    let _ = bus
                        .broadcast(Event::Speech(SpeechEvent::TranscriptionFailed {
                            reason: "low confidence".to_string(),
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
}

#[async_trait]
impl Actor for SttActor {
    fn name(&self) -> &'static str {
        "stt"
    }

    async fn start(&mut self, bus: Arc<EventBus>) -> Result<(), ActorError> {
        self.bus = Some(bus.clone());
        self.bus_rx = Some(bus.subscribe_broadcast());

        if let Err(e) = self.initialize_backend().await {
            let _ = bus
                .broadcast(Event::Speech(SpeechEvent::SpeechFailed {
                    reason: format!("STT model load failed: {}", e),
                    request_id: 0,
                }))
                .await;
            return Err(ActorError::StartupFailed(e.to_string()));
        }

        if let Err(e) = self.maybe_start_audio_capture() {
            let _ = bus
                .broadcast(Event::Speech(SpeechEvent::SpeechFailed {
                    reason: format!("audio device unavailable: {}", e),
                    request_id: 0,
                }))
                .await;
            return Err(ActorError::StartupFailed(e.to_string()));
        }

        bus.broadcast(Event::System(SystemEvent::ActorReady { actor_name: "STT" }))
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

        let bus = self
            .bus
            .as_ref()
            .ok_or_else(|| ActorError::RuntimeError("bus not initialized in start()".to_string()))?
            .clone();

        loop {
            tokio::select! {
                bus_event = bus_rx.recv() => {
                    match bus_event {
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
                        let request_id = self.next_request_id();
                        self.transcribe_with_timeout(buffer, request_id, &bus).await;
                    }
                }
            }
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), ActorError> {
        self.audio_rx = None;
        self.audio_stream = None;

        #[cfg(feature = "whisper")]
        if let Some(SttBackendHandle::WhisperWorker { tx }) = self.backend_handle.take() {
            let _ = tx.send(WorkerCommand::Shutdown);
        }

        #[cfg(not(feature = "whisper"))]
        {
            self.backend_handle = None;
        }

        Ok(())
    }
}

enum SttBackendHandle {
    Mock,
    #[cfg(feature = "whisper")]
    WhisperWorker {
        tx: std::sync::mpsc::Sender<WorkerCommand>,
    },
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

fn calculate_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }

    let sum_squares: f32 = samples.iter().map(|s| s * s).sum();
    (sum_squares / samples.len() as f32).sqrt()
}

#[cfg(feature = "whisper")]
fn resolve_model_path(configured: Option<&str>) -> Result<String, SpeechError> {
    if let Some(path) = configured {
        return Ok(path.to_string());
    }

    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map_err(|_| SpeechError::SttInitFailed("cannot determine home directory".to_string()))?;

    Ok(format!("{}/.sena/models/whisper/ggml-small.bin", home))
}

#[cfg(feature = "whisper")]
enum WorkerCommand {
    Transcribe {
        samples: Vec<f32>,
        reply: oneshot::Sender<Result<TranscriptionResult, SpeechError>>,
    },
    Shutdown,
}

#[cfg(feature = "whisper")]
fn whisper_worker_loop(model_path: &str, rx: std::sync::mpsc::Receiver<WorkerCommand>) {
    use whisper_rs::WhisperContext;

    let mut context = match WhisperContext::new(model_path) {
        Ok(ctx) => ctx,
        Err(_) => return,
    };

    while let Ok(command) = rx.recv() {
        match command {
            WorkerCommand::Shutdown => break,
            WorkerCommand::Transcribe { samples, reply } => {
                let result = transcribe_with_whisper(&mut context, &samples);
                let _ = reply.send(result);
            }
        }
    }
}

#[cfg(feature = "whisper")]
fn transcribe_with_whisper(
    context: &mut whisper_rs::WhisperContext,
    samples: &[f32],
) -> Result<TranscriptionResult, SpeechError> {
    use whisper_rs::{FullParams, SamplingStrategy};

    let mut state = context.create_state().map_err(|e| {
        SpeechError::TranscriptionFailed(format!("create whisper state failed: {}", e))
    })?;

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_n_threads(4);
    params.set_translate(false);
    params.set_language(Some("en"));

    state
        .full(params, samples)
        .map_err(|e| SpeechError::TranscriptionFailed(format!("whisper full failed: {}", e)))?;

    let segment_count = state
        .full_n_segments()
        .map_err(|e| SpeechError::TranscriptionFailed(format!("segment count failed: {}", e)))?;

    let mut text = String::new();
    for i in 0..segment_count {
        let seg = state
            .full_get_segment_text(i)
            .map_err(|e| SpeechError::TranscriptionFailed(format!("segment read failed: {}", e)))?;
        text.push_str(seg);
    }

    let normalized = text.trim().to_string();
    let confidence = if normalized.is_empty() {
        0.0
    } else {
        (calculate_rms(samples) * 10.0).clamp(0.55, 0.99)
    };

    Ok(TranscriptionResult {
        text: normalized,
        confidence,
    })
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
    async fn low_energy_audio_emits_transcription_failed() {
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
                Ok(Ok(Event::Speech(SpeechEvent::TranscriptionFailed { reason, .. }))) => {
                    assert_eq!(reason, "low confidence");
                    found = true;
                    break;
                }
                Ok(Ok(_)) => continue,
                Ok(Err(_)) => break,
                Err(_) => break,
            }
        }

        assert!(found, "expected transcription failed event");

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
