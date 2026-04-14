//! Sherpa-onnx Zipformer streaming STT backend.
//!
//! Wraps a `SherpaZipformerStt` worker thread. Accumulates a growing-window
//! rolling buffer and decodes every 200 ms via the worker. `flush()` emits
//! the final transcription and sends `ResetStream` so the model is clean for
//! the next session.

use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::SpeechError;
use super::backend_trait::{SttBackend, SttEvent};

/// How often to run a growing-window decode (200 ms).
const SHERPA_DECODE_INTERVAL: Duration = Duration::from_millis(200);

/// Maximum rolling audio retained for Sherpa growing-window decode (8 s at 16 kHz).
const SHERPA_MAX_SAMPLES: usize = 16_000 * 8;

/// Synchronous worker commands for the Sherpa thread.
enum SherpaCmd {
    Decode {
        samples: Vec<f32>,
        reply: mpsc::Sender<String>,
    },
    ResetStream,
    Shutdown,
}

/// Sherpa-onnx STT backend backed by a Zipformer ONNX worker thread.
///
/// `feed()` appends audio to a growing rolling buffer and decodes every
/// `SHERPA_DECODE_INTERVAL`, returning `Partial` events.
/// `flush()` decodes the remaining buffer, emits a final `Completed` event,
/// and resets the Sherpa stream for the next session.
pub struct SherpaSttBackend {
    worker_tx: mpsc::Sender<SherpaCmd>,
    rolling_samples: Vec<f32>,
    last_decode: Option<Instant>,
}

impl SherpaSttBackend {
    /// Load the Sherpa-onnx model synchronously and spawn the worker thread.
    ///
    /// Must be called from within `tokio::task::spawn_blocking` to avoid blocking
    /// the async executor during the model-present check and worker-init handshake.
    pub fn new(model_dir: &Path) -> Result<Self, SpeechError> {
        let sherpa_model_dir = model_dir.join("sherpa-streaming");

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

        let encoder = sherpa_model_dir
            .join("encoder-epoch-99-avg-1.int8.onnx")
            .to_str()
            .ok_or_else(|| {
                SpeechError::SttInitFailed("non-UTF-8 path for encoder".to_string())
            })?
            .to_string();

        let decoder = sherpa_model_dir
            .join("decoder-epoch-99-avg-1.int8.onnx")
            .to_str()
            .ok_or_else(|| {
                SpeechError::SttInitFailed("non-UTF-8 path for decoder".to_string())
            })?
            .to_string();

        let joiner = sherpa_model_dir
            .join("joiner-epoch-99-avg-1.int8.onnx")
            .to_str()
            .ok_or_else(|| {
                SpeechError::SttInitFailed("non-UTF-8 path for joiner".to_string())
            })?
            .to_string();

        let tokens = sherpa_model_dir
            .join("tokens.txt")
            .to_str()
            .ok_or_else(|| {
                SpeechError::SttInitFailed("non-UTF-8 path for tokens".to_string())
            })?
            .to_string();

        let (cmd_tx, cmd_rx) = mpsc::channel::<SherpaCmd>();
        let (init_tx, init_rx) = mpsc::channel::<Result<(), SpeechError>>();

        std::thread::spawn(move || {
            match crate::sherpa_stt::SherpaZipformerStt::load(
                &encoder, &decoder, &joiner, &tokens,
            ) {
                Ok(model) => {
                    let _ = init_tx.send(Ok(()));
                    sherpa_worker_loop(model, cmd_rx);
                }
                Err(e) => {
                    let _ = init_tx.send(Err(e));
                }
            }
        });

        init_rx
            .recv()
            .map_err(|e| {
                SpeechError::SttInitFailed(format!(
                    "sherpa worker init channel failed: {}",
                    e
                ))
            })??;

        tracing::info!("Sherpa backend initialized successfully");

        Ok(Self {
            worker_tx: cmd_tx,
            rolling_samples: Vec::new(),
            last_decode: None,
        })
    }

    fn decode_rolling(&self, samples: Vec<f32>) -> Result<String, SpeechError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.worker_tx
            .send(SherpaCmd::Decode {
                samples,
                reply: reply_tx,
            })
            .map_err(|_| SpeechError::ChannelClosed("sherpa worker closed".to_string()))?;

        reply_rx.recv_timeout(Duration::from_millis(500)).map_err(|e| {
            SpeechError::TranscriptionFailed(format!("sherpa decode timeout: {}", e))
        })
    }
}

impl Drop for SherpaSttBackend {
    fn drop(&mut self) {
        let _ = self.worker_tx.send(SherpaCmd::Shutdown);
    }
}

impl SttBackend for SherpaSttBackend {
    fn preferred_chunk_samples(&self) -> usize {
        3_200 // 200 ms at 16 kHz — Sherpa decode interval
    }

    fn feed(&mut self, pcm: &[f32]) -> Result<Vec<SttEvent>, SpeechError> {
        // Grow the rolling buffer (cap at 8 s).
        self.rolling_samples.extend_from_slice(pcm);
        if self.rolling_samples.len() > SHERPA_MAX_SAMPLES {
            let excess = self.rolling_samples.len() - SHERPA_MAX_SAMPLES;
            self.rolling_samples.drain(..excess);
        }

        let interval_elapsed = self
            .last_decode
            .map(|t| t.elapsed() >= SHERPA_DECODE_INTERVAL)
            .unwrap_or(true);

        if interval_elapsed && !self.rolling_samples.is_empty() {
            self.last_decode = Some(Instant::now());
            let text = self.decode_rolling(self.rolling_samples.clone())?;
            if !text.trim().is_empty() {
                return Ok(vec![SttEvent::Partial {
                    text: text.trim().to_string(),
                    confidence: 0.9,
                }]);
            }
        }

        Ok(vec![])
    }

    fn flush(&mut self) -> Result<Vec<SttEvent>, SpeechError> {
        let mut events = vec![];

        if !self.rolling_samples.is_empty() {
            let samples = std::mem::take(&mut self.rolling_samples);
            let text = self.decode_rolling(samples)?;
            if !text.trim().is_empty() {
                events.push(SttEvent::Completed {
                    text: text.trim().to_string(),
                    confidence: 0.9,
                });
            }
        }

        // Reset the Sherpa stream so the next session starts clean.
        let _ = self.worker_tx.send(SherpaCmd::ResetStream);
        self.last_decode = None;

        Ok(events)
    }

    fn backend_name(&self) -> &'static str {
        "sherpa"
    }

    fn vram_mb(&self) -> u64 {
        100
    }
}

fn sherpa_worker_loop(
    mut model: crate::sherpa_stt::SherpaZipformerStt,
    rx: mpsc::Receiver<SherpaCmd>,
) {
    while let Ok(cmd) = rx.recv() {
        match cmd {
            SherpaCmd::Shutdown => break,
            SherpaCmd::ResetStream => {
                model.reset_stream();
            }
            SherpaCmd::Decode { samples, reply } => {
                let text = model.decode_chunk(samples);
                let _ = reply.send(text);
            }
        }
    }
}
