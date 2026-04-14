//! NVIDIA Parakeet-EOU streaming STT backend.
//!
//! Wraps a `ParakeetStt` worker thread. Audio is drained in 2560-sample
//! (160 ms) chunks. SentencePiece tokens (`_hello`, `_world`) are accumulated
//! and decoded into display text. `flush()` pads any remaining sub-chunk audio
//! with silence, emits the final decoded text, and resets internal state.

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use crate::SpeechError;
use super::backend_trait::{SttBackend, SttEvent};

/// Exact chunk size required by ParakeetEOU — 160 ms at 16 kHz.
/// Feeding larger chunks causes the model to silently discard all but the last 160 ms.
const PARAKEET_CHUNK_SAMPLES: usize = 2_560;

/// Synchronous worker commands for the Parakeet thread.
enum ParakeetCmd {
    /// 160 ms f32 chunk for streaming decode.
    Chunk {
        samples: Vec<f32>,
        reply: mpsc::Sender<String>,
    },
    Shutdown,
}

/// Parakeet-EOU STT backend backed by a ONNX worker thread.
///
/// `feed()` accumulates audio into 2560-sample chunks and decodes each one,
/// accumulating SentencePiece tokens. `flush()` pads remaining samples with
/// silence, finalizes the token stream, and resets state.
pub struct ParakeetSttBackend {
    worker_tx: mpsc::Sender<ParakeetCmd>,
    /// Sub-2560-sample accumulator. Drained in exact 2560-sample chunks.
    chunk_accumulator: Vec<f32>,
    /// Accumulated SentencePiece tokens for the current utterance.
    session_text: String,
}

impl ParakeetSttBackend {
    /// Load the Parakeet model synchronously and spawn the inference worker thread.
    ///
    /// Must be called from within `tokio::task::spawn_blocking` to avoid blocking
    /// the async executor during model load.
    pub fn new(model_dir: &Path) -> Result<Self, SpeechError> {
        let parakeet_model_dir = model_dir.join("parakeet");

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

        let model = crate::parakeet_stt::ParakeetStt::load(&parakeet_model_dir)?;

        let (cmd_tx, cmd_rx) = mpsc::channel::<ParakeetCmd>();
        std::thread::spawn(move || {
            parakeet_worker_loop(model, cmd_rx);
        });

        tracing::info!("Parakeet backend initialized successfully");

        Ok(Self {
            worker_tx: cmd_tx,
            chunk_accumulator: Vec::new(),
            session_text: String::new(),
        })
    }

    /// Decode a single 2560-sample f32 chunk via the worker thread.
    fn decode_chunk_inner(&self, samples: Vec<f32>) -> Result<String, SpeechError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.worker_tx
            .send(ParakeetCmd::Chunk {
                samples,
                reply: reply_tx,
            })
            .map_err(|_| SpeechError::ChannelClosed("parakeet worker closed".to_string()))?;

        reply_rx.recv_timeout(Duration::from_millis(500)).map_err(|e| {
            SpeechError::TranscriptionFailed(format!("parakeet chunk timeout: {}", e))
        })
    }
}

impl Drop for ParakeetSttBackend {
    fn drop(&mut self) {
        let _ = self.worker_tx.send(ParakeetCmd::Shutdown);
    }
}

impl SttBackend for ParakeetSttBackend {
    fn preferred_chunk_samples(&self) -> usize {
        PARAKEET_CHUNK_SAMPLES // 160 ms at 16 kHz — required chunk size
    }

    fn feed(&mut self, pcm: &[f32]) -> Result<Vec<SttEvent>, SpeechError> {
        self.chunk_accumulator.extend_from_slice(pcm);
        let mut events = vec![];

        // Drain exact 2560-sample chunks and decode each one.
        while self.chunk_accumulator.len() >= PARAKEET_CHUNK_SAMPLES {
            let chunk: Vec<f32> = self
                .chunk_accumulator
                .drain(..PARAKEET_CHUNK_SAMPLES)
                .collect();

            let token = self.decode_chunk_inner(chunk)?;

            if !token.trim().is_empty() {
                self.session_text.push_str(&token);
            }

            if !self.session_text.is_empty() {
                let display = decode_parakeet_tokens(&self.session_text);
                if !display.is_empty() {
                    events.push(SttEvent::Partial {
                        text: display,
                        confidence: 0.85,
                    });
                }
            }
        }

        Ok(events)
    }

    fn flush(&mut self) -> Result<Vec<SttEvent>, SpeechError> {
        // Pad any remaining sub-chunk samples to exactly PARAKEET_CHUNK_SAMPLES with silence.
        if !self.chunk_accumulator.is_empty() {
            let mut remaining = std::mem::take(&mut self.chunk_accumulator);
            remaining.resize(PARAKEET_CHUNK_SAMPLES, 0.0_f32);

            let token = self.decode_chunk_inner(remaining)?;
            if !token.trim().is_empty() {
                self.session_text.push_str(&token);
            }
        }

        let final_text = decode_parakeet_tokens(&self.session_text);
        self.session_text.clear();
        self.chunk_accumulator.clear();

        if final_text.is_empty() {
            return Ok(vec![]);
        }

        Ok(vec![SttEvent::Completed {
            text: final_text,
            confidence: 0.85,
        }])
    }

    fn backend_name(&self) -> &'static str {
        "parakeet"
    }

    fn vram_mb(&self) -> u64 {
        480
    }
}

/// Post-process raw Parakeet-EOU SentencePiece tokens into readable text.
///
/// Parakeet emits tokens like `_hello`, `_world` where a leading `_` means
/// "space before this word" (the SentencePiece convention). This function
/// converts that token stream into a properly spaced string.
///
/// Example: `"_my_name_is"` → `"my name is"`.
pub(crate) fn decode_parakeet_tokens(tokens: &str) -> String {
    tokens.replace('_', " ").trim().to_string()
}

fn parakeet_worker_loop(
    mut model: crate::parakeet_stt::ParakeetStt,
    rx: mpsc::Receiver<ParakeetCmd>,
) {
    while let Ok(cmd) = rx.recv() {
        match cmd {
            ParakeetCmd::Shutdown => break,
            ParakeetCmd::Chunk { samples, reply } => {
                let result = model.decode_chunk_f32(&samples).unwrap_or_default();
                let _ = reply.send(result);
            }
        }
    }
}
