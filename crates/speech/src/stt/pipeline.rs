//! Whisper-based speech-to-text pipeline.
//!
//! This module provides the core transcription engine using whisper-rs,
//! which wraps whisper.cpp for local STT processing.

use std::sync::Arc;

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// Whisper transcription pipeline.
///
/// Wraps a whisper-rs context in a thread-safe container.
/// Both model loading and transcription are blocking operations and
/// must be called from `tokio::task::spawn_blocking`.
#[derive(Debug)]
pub struct WhisperPipeline {
    ctx: Arc<WhisperContext>,
    model_path: String,
}

impl WhisperPipeline {
    /// Load a GGML Whisper model from the given path.
    ///
    /// This is a **blocking operation** — call from `spawn_blocking`.
    ///
    /// # Arguments
    /// - `model_path`: Path to the GGML model file (e.g., "ggml-base.en.bin")
    ///
    /// # Returns
    /// `Ok(WhisperPipeline)` on success, or `Err(String)` with error details.
    pub fn load(model_path: &str) -> Result<Self, String> {
        let ctx = WhisperContext::new_with_params(model_path, WhisperContextParameters::default())
            .map_err(|e| format!("whisper model load failed: {}", e))?;

        Ok(Self {
            ctx: Arc::new(ctx),
            model_path: model_path.to_string(),
        })
    }

    /// Transcribe audio samples (16kHz mono f32).
    ///
    /// This is a **blocking operation** — call from `spawn_blocking`.
    ///
    /// # Arguments
    /// - `samples`: PCM audio samples as f32 in range [-1.0, 1.0], 16kHz, mono
    ///
    /// # Returns
    /// `Ok(Vec<TranscriptionSegment>)` with transcription results, or `Err(String)`.
    pub fn transcribe(&self, samples: &[f32]) -> Result<Vec<TranscriptionSegment>, String> {
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

    /// Get the model path for this pipeline.
    pub fn model_path(&self) -> &str {
        &self.model_path
    }
}

/// A transcription segment returned by Whisper.
#[derive(Debug, Clone)]
pub struct TranscriptionSegment {
    /// Transcribed text for this segment.
    pub text: String,
    /// Start timestamp in milliseconds.
    pub start_ms: u32,
    /// End timestamp in milliseconds.
    pub end_ms: u32,
    /// Confidence score [0.0, 1.0].
    pub confidence: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whisper_pipeline_load_fails_with_invalid_path() {
        let result = WhisperPipeline::load("/nonexistent/model.bin");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("whisper model load failed"));
    }

    #[test]
    fn transcription_segment_has_expected_fields() {
        let seg = TranscriptionSegment {
            text: "hello world".to_string(),
            start_ms: 0,
            end_ms: 1500,
            confidence: 0.85,
        };
        assert_eq!(seg.text, "hello world");
        assert_eq!(seg.start_ms, 0);
        assert_eq!(seg.end_ms, 1500);
        assert_eq!(seg.confidence, 0.85);
    }
}
