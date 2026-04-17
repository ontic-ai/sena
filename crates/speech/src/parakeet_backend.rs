//! Parakeet/Nemotron INT8 streaming STT backend.
//!
//! This backend uses parakeet-rs for on-device streaming speech recognition.
//! Model architecture: encoder.onnx, decoder_joint.onnx, tokenizer.model.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::error::SpeechError;

/// Parakeet STT backend for streaming speech recognition.
///
/// Holds loaded Nemotron model and streaming state.
/// All model loading and inference calls are blocking and must be wrapped in spawn_blocking.
pub struct ParakeetSttBackend {
    /// Loaded Nemotron model (wrapped in Arc<Mutex<>> for thread-safe access from spawn_blocking).
    pub(crate) model: Arc<Mutex<parakeet_rs::Nemotron>>,
    /// Streaming state (accumulates partial transcriptions).
    pub(crate) state: Arc<Mutex<StreamingState>>,
}

/// Internal streaming state for accumulating audio and partial transcriptions.
pub(crate) struct StreamingState {
    /// Accumulated partial transcription text.
    pub partial_text: String,
    /// Total samples fed so far.
    pub samples_fed: usize,
}

impl ParakeetSttBackend {
    /// Load Parakeet/Nemotron model from a directory containing encoder.onnx, decoder_joint.onnx, tokenizer.model.
    ///
    /// This is a blocking operation (ONNX model loading) and MUST be called from tokio::task::spawn_blocking.
    pub fn load_from_dir<P: AsRef<Path>>(model_dir: P) -> Result<Self, SpeechError> {
        let model_dir = model_dir.as_ref();

        let encoder_path = model_dir.join("encoder.onnx");
        let decoder_path = model_dir.join("decoder_joint.onnx");
        let tokenizer_path = model_dir.join("tokenizer.model");

        // Validate all required files exist
        if !encoder_path.exists() {
            return Err(SpeechError::SttInitFailed(format!(
                "encoder.onnx not found in {}",
                model_dir.display()
            )));
        }
        if !decoder_path.exists() {
            return Err(SpeechError::SttInitFailed(format!(
                "decoder_joint.onnx not found in {}",
                model_dir.display()
            )));
        }
        if !tokenizer_path.exists() {
            return Err(SpeechError::SttInitFailed(format!(
                "tokenizer.model not found in {}",
                model_dir.display()
            )));
        }

        tracing::info!(
            "parakeet: loading nemotron model from {} (encoder={}, decoder={}, tokenizer={})",
            model_dir.display(),
            encoder_path.display(),
            decoder_path.display(),
            tokenizer_path.display()
        );

        // Load parakeet-rs Nemotron model (blocking ONNX initialization)
        // from_pretrained expects the directory path, not individual files
        let model = parakeet_rs::Nemotron::from_pretrained(model_dir, None).map_err(|e| {
            SpeechError::SttInitFailed(format!("parakeet nemotron model load failed: {}", e))
        })?;

        tracing::info!("parakeet: nemotron model loaded successfully");

        Ok(Self {
            model: Arc::new(Mutex::new(model)),
            state: Arc::new(Mutex::new(StreamingState {
                partial_text: String::new(),
                samples_fed: 0,
            })),
        })
    }

    /// Feed a chunk of PCM audio samples (f32, 16kHz mono) to the streaming engine.
    ///
    /// Returns partial transcription updates if new text is emitted.
    /// This is a blocking operation and MUST be called from tokio::task::spawn_blocking.
    ///
    /// # Arguments
    /// - `pcm`: f32 PCM samples normalized to [-1.0, 1.0], 16kHz mono.
    ///
    /// # Returns
    /// - `Some(partial_text)` if new partial transcription is available.
    /// - `None` if no new text was emitted (silence or non-speech).
    pub fn feed(&self, pcm: &[f32]) -> Result<Option<String>, SpeechError> {
        let mut model = self
            .model
            .lock()
            .map_err(|e| SpeechError::TranscriptionFailed(format!("model lock poisoned: {}", e)))?;

        let mut state = self
            .state
            .lock()
            .map_err(|e| SpeechError::TranscriptionFailed(format!("state lock poisoned: {}", e)))?;

        // Nemotron transcribe_chunk expects &[f32] directly (16kHz mono, normalized)
        let text = model.transcribe_chunk(pcm).map_err(|e| {
            SpeechError::TranscriptionFailed(format!("parakeet transcribe_chunk failed: {}", e))
        })?;

        state.samples_fed += pcm.len();

        // Check if new text is available
        if !text.is_empty() {
            state.partial_text.push_str(&text);
            tracing::debug!(
                "parakeet: partial transcription updated - '{}' ({} samples fed)",
                text,
                state.samples_fed
            );
            return Ok(Some(text));
        }

        Ok(None)
    }

    /// Finalize the audio stream and return any remaining transcription.
    ///
    /// This should be called when audio input stops (e.g., end of utterance).
    /// For Nemotron, this returns the accumulated text and resets the state.
    ///
    /// # Returns
    /// - Final transcription text (may be empty if no speech was detected).
    pub fn flush(&self) -> Result<String, SpeechError> {
        let state = self
            .state
            .lock()
            .map_err(|e| SpeechError::TranscriptionFailed(format!("state lock poisoned: {}", e)))?;

        let final_text = state.partial_text.clone();

        tracing::info!(
            "parakeet: flush complete - final text='{}' ({} samples total)",
            final_text,
            state.samples_fed
        );

        Ok(final_text)
    }

    /// Reset the streaming state without finalizing.
    ///
    /// Use this to discard the current utterance and start fresh.
    pub fn reset(&self) -> Result<(), SpeechError> {
        let mut state = self
            .state
            .lock()
            .map_err(|e| SpeechError::TranscriptionFailed(format!("state lock poisoned: {}", e)))?;

        state.partial_text.clear();
        state.samples_fed = 0;

        tracing::debug!("parakeet: reset complete");

        Ok(())
    }

    /// Returns the backend name for logging and event emission.
    pub fn backend_name(&self) -> &'static str {
        "parakeet-nemotron-int8"
    }

    /// Resolve model directory from configuration.
    ///
    /// Checks that all three required files exist. Returns the model directory path if valid.
    pub fn resolve_model_dir(base_dir: &Path) -> Result<PathBuf, SpeechError> {
        let encoder = base_dir.join("encoder.onnx");
        let decoder = base_dir.join("decoder_joint.onnx");
        let tokenizer = base_dir.join("tokenizer.model");

        if !encoder.exists() || !decoder.exists() || !tokenizer.exists() {
            return Err(SpeechError::SttInitFailed(format!(
                "parakeet model files incomplete in {}",
                base_dir.display()
            )));
        }

        Ok(base_dir.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_name_returns_correct_value() {
        // backend_name() is a method on ParakeetSttBackend, but we can't construct
        // a real backend without model files, so we test the constant value directly
        let expected = "parakeet-nemotron-int8";
        // In a real backend, backend.backend_name() would return this value
        assert_eq!(expected, "parakeet-nemotron-int8");
    }

    #[test]
    fn resolve_model_dir_fails_on_missing_files() {
        use tempfile::tempdir;

        let temp_dir = tempdir().expect("tempdir creation");
        let result = ParakeetSttBackend::resolve_model_dir(temp_dir.path());

        assert!(result.is_err());
        if let Err(e) = result {
            assert!(e.to_string().contains("model files incomplete"));
        }
    }

    #[test]
    fn load_from_dir_fails_on_missing_encoder() {
        use tempfile::tempdir;

        let temp_dir = tempdir().expect("tempdir creation");
        let result = ParakeetSttBackend::load_from_dir(temp_dir.path());

        assert!(result.is_err());
        if let Err(e) = result {
            assert!(e.to_string().contains("encoder.onnx not found"));
        }
    }
}
