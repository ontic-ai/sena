//! STT Worker - isolated whisper-rs transcription process.
//!
//! This binary exists solely to isolate whisper-rs from llama.cpp to prevent GGML symbol conflicts.
//! It reads PCM audio chunks from stdin, runs whisper transcription, and writes JSON events to stdout.
//!
//! # Wire Protocol
//!
//! **Input (stdin):**
//! - Length-prefixed PCM chunks: `[u32 sample_count (LE)][f32 samples...]`
//! - Zero-length chunk (sample_count=0) signals graceful shutdown
//!
//! **Output (stdout):**
//! - JSON events, one per line:
//!   - `{"type":"listening"}` — worker ready
//!   - `{"type":"word","text":"hello","confidence":0.94,"sequence":1,"request_id":42}`
//!   - `{"type":"completed","text":"full text","avg_confidence":0.91,"request_id":42}`
//!   - `{"type":"stopped"}` — graceful shutdown acknowledged
//!   - `{"type":"error","reason":"description"}`

use std::io::{self, Read, Write};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// Event sent to parent process via stdout.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum WorkerEvent {
    /// Worker is listening and ready for audio chunks.
    Listening,
    /// A single word was transcribed (streaming output).
    Word {
        text: String,
        confidence: f32,
        sequence: u32,
        request_id: u64,
    },
    /// Transcription request completed.
    Completed {
        text: String,
        avg_confidence: f32,
        request_id: u64,
    },
    /// Worker is stopping gracefully.
    Stopped,
    /// An error occurred.
    Error { reason: String },
}

/// Whisper transcription worker.
struct SttWorker {
    ctx: Arc<WhisperContext>,
    request_id_counter: u64,
}

impl SttWorker {
    /// Load a whisper model from the given path.
    fn load(model_path: &str) -> Result<Self, String> {
        let ctx = WhisperContext::new_with_params(model_path, WhisperContextParameters::default())
            .map_err(|e| format!("whisper model load failed: {}", e))?;

        Ok(Self {
            ctx: Arc::new(ctx),
            request_id_counter: 0,
        })
    }

    /// Transcribe a PCM audio chunk (16kHz mono f32).
    fn transcribe(&mut self, samples: &[f32]) -> Result<Vec<TranscriptionSegment>, String> {
        // Create a new state for this transcription
        let mut state = self
            .ctx
            .create_state()
            .map_err(|e| format!("whisper state creation failed: {}", e))?;

        // Configure transcription parameters for English, greedy decoding
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some("en"));
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_special(false);
        params.set_token_timestamps(true);

        // Run transcription
        state
            .full(params, samples)
            .map_err(|e| format!("transcription failed: {}", e))?;

        // Extract segments from the state
        let num_segments = state.full_n_segments();
        let mut segments = Vec::with_capacity(num_segments as usize);

        for i in 0..num_segments {
            if let Some(segment) = state.get_segment(i) {
                let text = segment
                    .to_str_lossy()
                    .map_err(|e| format!("segment text failed: {}", e))?
                    .to_string();

                let start_ms = (segment.start_timestamp() * 10) as u32; // centiseconds → ms
                let end_ms = (segment.end_timestamp() * 10) as u32;

                // Calculate confidence from no_speech_prob
                let no_speech_prob = segment.no_speech_probability();
                let confidence = 1.0 - no_speech_prob;

                segments.push(TranscriptionSegment {
                    text,
                    start_ms,
                    end_ms,
                    confidence,
                });
            }
        }

        Ok(segments)
    }

    fn next_request_id(&mut self) -> u64 {
        let id = self.request_id_counter;
        self.request_id_counter = self.request_id_counter.saturating_add(1);
        id
    }

    /// Process audio chunk and emit events.
    fn process_chunk(&mut self, samples: Vec<f32>) -> Result<(), String> {
        let request_id = self.next_request_id();

        eprintln!(
            "stt-worker: starting transcription of {} samples (request_id={})",
            samples.len(),
            request_id
        );

        // Run transcription
        let segments = self.transcribe(&samples)?;

        eprintln!(
            "stt-worker: transcription complete - {} segments",
            segments.len()
        );

        // Emit word events
        let mut sequence = 0u32;
        for seg in &segments {
            let words: Vec<&str> = seg.text.split_whitespace().collect();
            eprintln!(
                "stt-worker: segment '{}' confidence={:.2}",
                seg.text.trim(),
                seg.confidence
            );

            for word in words {
                let event = WorkerEvent::Word {
                    text: word.to_string(),
                    confidence: seg.confidence,
                    sequence,
                    request_id,
                };
                emit_event(&event)?;
                sequence += 1;
            }
        }

        // Emit completed event
        let full_text: String = segments
            .iter()
            .map(|s| s.text.trim())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");

        let avg_confidence = if segments.is_empty() {
            0.0
        } else {
            segments.iter().map(|s| s.confidence).sum::<f32>() / segments.len() as f32
        };

        eprintln!(
            "stt-worker: final text='{}', avg_confidence={:.2}",
            full_text, avg_confidence
        );

        let event = WorkerEvent::Completed {
            text: full_text,
            avg_confidence,
            request_id,
        };
        emit_event(&event)?;

        Ok(())
    }
}

/// A transcription segment returned by Whisper.
#[derive(Debug, Clone)]
struct TranscriptionSegment {
    text: String,
    #[allow(dead_code)]
    start_ms: u32,
    #[allow(dead_code)]
    end_ms: u32,
    confidence: f32,
}

/// Emit a JSON event to stdout and flush.
fn emit_event(event: &WorkerEvent) -> Result<(), String> {
    let json = serde_json::to_string(event).map_err(|e| format!("JSON serialize failed: {}", e))?;
    let mut stdout = io::stdout();
    writeln!(stdout, "{}", json).map_err(|e| format!("stdout write failed: {}", e))?;
    stdout
        .flush()
        .map_err(|e| format!("stdout flush failed: {}", e))?;
    Ok(())
}

/// Read a length-prefixed PCM chunk from stdin.
///
/// Returns `None` if zero-length chunk (graceful shutdown signal).
fn read_chunk() -> Result<Option<Vec<f32>>, String> {
    let mut stdin = io::stdin();

    // Read 4-byte length prefix (little-endian u32)
    let mut len_buf = [0u8; 4];
    stdin
        .read_exact(&mut len_buf)
        .map_err(|e| format!("stdin read failed: {}", e))?;

    let sample_count = u32::from_le_bytes(len_buf);

    eprintln!(
        "stt-worker: read chunk header - {} samples expected",
        sample_count
    );

    // Zero-length chunk signals graceful shutdown
    if sample_count == 0 {
        eprintln!("stt-worker: received shutdown signal (zero-length chunk)");
        return Ok(None);
    }

    // Read f32 samples
    let byte_count = (sample_count as usize) * 4;
    let mut sample_bytes = vec![0u8; byte_count];
    stdin
        .read_exact(&mut sample_bytes)
        .map_err(|e| format!("stdin read samples failed: {}", e))?;

    // Convert bytes to f32
    let samples: Vec<f32> = sample_bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();

    eprintln!("stt-worker: successfully read {} samples", samples.len());

    Ok(Some(samples))
}

fn main() {
    eprintln!("stt-worker: starting up");

    // Read model path from first command line argument or env var
    let model_path = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("WHISPER_MODEL_PATH").ok())
        .unwrap_or_else(|| {
            eprintln!("stt-worker: ERROR - no model path provided");
            emit_event(&WorkerEvent::Error {
                reason: "no model path provided (arg or WHISPER_MODEL_PATH env)".to_string(),
            })
            .ok();
            std::process::exit(1);
        });

    eprintln!("stt-worker: loading whisper model: {}", model_path);

    // Load whisper model
    let mut worker = match SttWorker::load(&model_path) {
        Ok(w) => {
            eprintln!("stt-worker: model loaded successfully");
            w
        }
        Err(e) => {
            eprintln!("stt-worker: ERROR - model load failed: {}", e);
            emit_event(&WorkerEvent::Error {
                reason: format!("model load failed: {}", e),
            })
            .ok();
            std::process::exit(2);
        }
    };

    // Signal ready
    if let Err(e) = emit_event(&WorkerEvent::Listening) {
        eprintln!("stt-worker: failed to emit listening event: {}", e);
        std::process::exit(3);
    }

    eprintln!("stt-worker: ready - waiting for audio chunks on stdin");

    // Main loop: read chunks, transcribe, write events
    loop {
        match read_chunk() {
            Ok(Some(samples)) => {
                if let Err(e) = worker.process_chunk(samples) {
                    eprintln!("stt-worker: ERROR in process_chunk: {}", e);
                    emit_event(&WorkerEvent::Error {
                        reason: format!("transcription error: {}", e),
                    })
                    .ok();
                }
            }
            Ok(None) => {
                // Graceful shutdown signal received
                eprintln!("stt-worker: shutting down gracefully");
                emit_event(&WorkerEvent::Stopped).ok();
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("stt-worker: ERROR reading chunk: {}", e);
                emit_event(&WorkerEvent::Error {
                    reason: format!("chunk read error: {}", e),
                })
                .ok();
                std::process::exit(4);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_event_serialization() {
        let event = WorkerEvent::Listening;
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(json, r#"{"type":"listening"}"#);

        let event = WorkerEvent::Word {
            text: "hello".to_string(),
            confidence: 0.94,
            sequence: 1,
            request_id: 42,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"word""#));
        assert!(json.contains(r#""text":"hello""#));
    }

    #[test]
    fn zero_length_chunk_signals_shutdown() {
        let chunk = vec![0u8, 0, 0, 0]; // u32 zero in LE
        let mut cursor = std::io::Cursor::new(chunk);
        let mut len_buf = [0u8; 4];
        cursor.read_exact(&mut len_buf).unwrap();
        let sample_count = u32::from_le_bytes(len_buf);
        assert_eq!(sample_count, 0);
    }
}
