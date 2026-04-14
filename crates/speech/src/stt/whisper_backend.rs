//! Whisper (candle) STT backend.
//!
//! Wraps a `CandleWhisperModel` worker thread. Accumulates a rolling buffer for
//! listen-mode interim transcriptions and delegates blocking inference to the
//! worker via a `std::sync::mpsc` channel with a synchronous reply sender.

use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::silence_detector::calculate_rms;
use crate::SpeechError;
use super::backend_trait::{SttBackend, SttEvent};

/// How often to emit interim (non-final) transcriptions during Whisper listen mode.
const LISTEN_INTERIM_INTERVAL: Duration = Duration::from_millis(1500);

/// Minimum audio accumulated before first interim transcription attempt (2 s at 16 kHz).
const LISTEN_INTERIM_MIN_SAMPLES: usize = 16_000 * 2;

/// Maximum rolling audio retained for listen-mode interim transcriptions (3 s at 16 kHz).
const LISTEN_ROLLING_MAX_SAMPLES: usize = 16_000 * 3;

/// Synchronous worker command for the candle inference thread.
enum WhisperCmd {
    Transcribe {
        samples: Vec<f32>,
        reply: mpsc::Sender<Result<WhisperResult, SpeechError>>,
    },
    Shutdown,
}

struct WhisperResult {
    text: String,
    confidence: f32,
}

/// Whisper STT backend backed by a candle inference worker thread.
///
/// Audio is accumulated in a rolling buffer. Partial (interim) events are
/// emitted every `LISTEN_INTERIM_INTERVAL` when enough speech energy is
/// present. `flush()` transcribes the accumulated buffer and resets state.
pub struct WhisperSttBackend {
    worker_tx: mpsc::Sender<WhisperCmd>,
    energy_threshold: f32,
    rolling_samples: Vec<f32>,
    last_interim: Option<Instant>,
}

impl WhisperSttBackend {
    /// Load the Whisper model synchronously and spawn the inference worker thread.
    ///
    /// Must be called from within `tokio::task::spawn_blocking` to avoid blocking
    /// the async executor during model load.
    pub fn new(
        model_dir: &Path,
        model_path: Option<&str>,
        energy_threshold: f32,
    ) -> Result<Self, SpeechError> {
        let model = crate::candle_whisper::CandleWhisperModel::load(model_dir, model_path)?;
        let (tx, rx) = mpsc::channel::<WhisperCmd>();

        std::thread::spawn(move || {
            whisper_worker_loop(model, rx);
        });

        Ok(Self {
            worker_tx: tx,
            energy_threshold,
            rolling_samples: Vec::new(),
            last_interim: None,
        })
    }

    /// Send samples to the worker thread and wait synchronously for the result.
    fn transcribe_samples(&self, samples: Vec<f32>) -> Result<Option<WhisperResult>, SpeechError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.worker_tx
            .send(WhisperCmd::Transcribe {
                samples,
                reply: reply_tx,
            })
            .map_err(|_| SpeechError::ChannelClosed("whisper worker closed".to_string()))?;

        reply_rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|e| {
                SpeechError::TranscriptionFailed(format!("whisper reply timeout: {}", e))
            })?
            .map(Some)
    }
}

impl Drop for WhisperSttBackend {
    fn drop(&mut self) {
        let _ = self.worker_tx.send(WhisperCmd::Shutdown);
    }
}

impl SttBackend for WhisperSttBackend {
    fn preferred_chunk_samples(&self) -> usize {
        16_000 // 1 s at 16 kHz — fake-streaming window
    }

    fn feed(&mut self, pcm: &[f32]) -> Result<Vec<SttEvent>, SpeechError> {
        // Accumulate in rolling buffer (cap at 3 s).
        self.rolling_samples.extend_from_slice(pcm);
        if self.rolling_samples.len() > LISTEN_ROLLING_MAX_SAMPLES {
            let excess = self.rolling_samples.len() - LISTEN_ROLLING_MAX_SAMPLES;
            self.rolling_samples.drain(..excess);
        }

        let enough_audio = self.rolling_samples.len() >= LISTEN_INTERIM_MIN_SAMPLES;
        let interval_elapsed = self
            .last_interim
            .map(|t| t.elapsed() >= LISTEN_INTERIM_INTERVAL)
            .unwrap_or(true);
        let has_speech =
            calculate_rms(&self.rolling_samples) > self.energy_threshold;

        if enough_audio && interval_elapsed && has_speech {
            self.last_interim = Some(Instant::now());
            if let Some(result) = self.transcribe_samples(self.rolling_samples.clone())? {
                if !result.text.trim().is_empty() {
                    return Ok(vec![SttEvent::Partial {
                        text: result.text.trim().to_string(),
                        confidence: result.confidence,
                    }]);
                }
            }
        }

        Ok(vec![])
    }

    fn flush(&mut self) -> Result<Vec<SttEvent>, SpeechError> {
        if self.rolling_samples.is_empty() {
            self.last_interim = None;
            return Ok(vec![]);
        }

        let samples = std::mem::take(&mut self.rolling_samples);
        self.last_interim = None;

        if let Some(result) = self.transcribe_samples(samples)? {
            if !result.text.trim().is_empty() {
                return Ok(vec![SttEvent::Completed {
                    text: result.text.trim().to_string(),
                    confidence: result.confidence,
                }]);
            }
        }

        Ok(vec![])
    }

    fn backend_name(&self) -> &'static str {
        "whisper"
    }

    fn vram_mb(&self) -> u64 {
        142
    }
}

fn whisper_worker_loop(
    mut model: crate::candle_whisper::CandleWhisperModel,
    rx: mpsc::Receiver<WhisperCmd>,
) {
    while let Ok(cmd) = rx.recv() {
        match cmd {
            WhisperCmd::Shutdown => break,
            WhisperCmd::Transcribe { samples, reply } => {
                let result = model.transcribe(&samples).map(|text| {
                    let confidence = if text.trim().is_empty() {
                        0.0
                    } else {
                        (calculate_rms(&samples) * 10.0).clamp(0.55, 0.99)
                    };
                    WhisperResult { text, confidence }
                });
                let _ = reply.send(result);
            }
        }
    }
}
