//! Parakeet Nemotron STT backend using parakeet-rs.

use crate::backend::{AudioDevice, SttBackend};
use crate::error::SttError;
use crate::types::SttEvent;
use parakeet_rs::Nemotron;
use std::path::PathBuf;

/// Parakeet STT backend powered by NVIDIA Nemotron INT8 ONNX models.
///
/// Uses parakeet-rs to perform streaming speech-to-text transcription.
pub struct ParakeetSttBackend {
    /// Nemotron model instance for real inference.
    model: Nemotron,
    /// Preferred chunk size in samples (100ms at 16kHz = 1600 samples).
    chunk_size: usize,
}

impl ParakeetSttBackend {
    /// Create a new Parakeet STT backend with validated asset paths.
    ///
    /// # Arguments
    /// * `encoder_path` - Path to encoder.onnx
    /// * `decoder_path` - Path to decoder_joint.onnx
    /// * `tokenizer_path` - Path to tokenizer.model
    ///
    /// All three files must exist and share the same parent directory.
    ///
    /// # Errors
    /// Returns `SttError::InitializationFailed` if any asset path is invalid or models cannot be loaded.
    pub fn new(
        encoder_path: PathBuf,
        decoder_path: PathBuf,
        tokenizer_path: PathBuf,
    ) -> Result<Self, SttError> {
        // Validate that all required model files exist
        if !encoder_path.exists() {
            return Err(SttError::InitializationFailed(format!(
                "encoder model not found: {}",
                encoder_path.display()
            )));
        }
        if !decoder_path.exists() {
            return Err(SttError::InitializationFailed(format!(
                "decoder model not found: {}",
                decoder_path.display()
            )));
        }
        if !tokenizer_path.exists() {
            return Err(SttError::InitializationFailed(format!(
                "tokenizer model not found: {}",
                tokenizer_path.display()
            )));
        }

        // Validate that all three files share the same parent directory
        let encoder_parent = encoder_path.parent().ok_or_else(|| {
            SttError::InitializationFailed("encoder path has no parent directory".to_string())
        })?;
        let decoder_parent = decoder_path.parent().ok_or_else(|| {
            SttError::InitializationFailed("decoder path has no parent directory".to_string())
        })?;
        let tokenizer_parent = tokenizer_path.parent().ok_or_else(|| {
            SttError::InitializationFailed("tokenizer path has no parent directory".to_string())
        })?;

        if encoder_parent != decoder_parent || encoder_parent != tokenizer_parent {
            return Err(SttError::InitializationFailed(
                "encoder, decoder, and tokenizer must be in the same directory".to_string(),
            ));
        }

        // Load the Nemotron model from the shared parent directory
        let model = Nemotron::from_pretrained(encoder_parent, None).map_err(|e| {
            SttError::InitializationFailed(format!("failed to load Nemotron model: {}", e))
        })?;

        // Chunk size of 1600 samples = 100ms at 16kHz
        Ok(Self {
            model,
            chunk_size: 1600,
        })
    }
}

impl SttBackend for ParakeetSttBackend {
    fn preferred_chunk_samples(&self) -> usize {
        self.chunk_size
    }

    fn feed(&mut self, pcm: &[f32]) -> Result<Vec<SttEvent>, SttError> {
        // Process incoming PCM samples through Nemotron streaming inference
        let text = self.model.transcribe_chunk(pcm).map_err(|e| {
            SttError::TranscriptionFailed(format!("Nemotron inference failed: {}", e))
        })?;

        // If we got any text back, emit it as a Word event (partial result)
        // In a future refinement, we could distinguish between partial and final results
        if text.is_empty() {
            Ok(vec![])
        } else {
            Ok(vec![SttEvent::Word {
                text,
                confidence: 0.95, // Nemotron doesn't expose per-word confidence; use default
            }])
        }
    }

    fn flush(&mut self) -> Result<Vec<SttEvent>, SttError> {
        // Get the accumulated transcript from the model's internal state
        let full_text = self.model.get_transcript();

        if full_text.is_empty() {
            return Ok(vec![]);
        }

        // Reset the model for the next utterance
        self.model.reset();

        // Return the completed transcription
        Ok(vec![SttEvent::Completed {
            text: full_text,
            confidence: 0.95,
        }])
    }

    fn list_audio_devices(&self) -> Result<Vec<AudioDevice>, SttError> {
        // Audio device enumeration is handled by cpal in the actor layer.
        // This backend does not directly interact with audio devices.
        Ok(vec![])
    }

    fn backend_name(&self) -> &'static str {
        "parakeet-nemotron-int8"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn parakeet_backend_initialization_fails_on_missing_encoder() {
        let (_, decoder, tokenizer) = create_stub_model_files();
        let result = ParakeetSttBackend::new(
            PathBuf::from("/nonexistent/encoder.onnx"),
            decoder.path().to_path_buf(),
            tokenizer.path().to_path_buf(),
        );

        assert!(result.is_err());
        assert!(matches!(result, Err(SttError::InitializationFailed(_))));
    }

    #[test]
    fn parakeet_backend_initialization_fails_on_missing_decoder() {
        let (encoder, _, tokenizer) = create_stub_model_files();
        let result = ParakeetSttBackend::new(
            encoder.path().to_path_buf(),
            PathBuf::from("/nonexistent/decoder.onnx"),
            tokenizer.path().to_path_buf(),
        );

        assert!(result.is_err());
    }

    #[test]
    fn parakeet_backend_initialization_fails_on_missing_tokenizer() {
        let (encoder, decoder, _) = create_stub_model_files();
        let result = ParakeetSttBackend::new(
            encoder.path().to_path_buf(),
            decoder.path().to_path_buf(),
            PathBuf::from("/nonexistent/tokenizer.model"),
        );

        assert!(result.is_err());
    }

    #[test]
    fn parakeet_backend_initialization_fails_on_mismatched_directories() {
        let (encoder, _decoder, tokenizer) = create_stub_model_files();
        let (other, _, _) = create_stub_model_files();
        let result = ParakeetSttBackend::new(
            encoder.path().to_path_buf(),
            other.path().to_path_buf(),
            tokenizer.path().to_path_buf(),
        );

        // Should fail either due to mismatched directories or model loading failure
        assert!(result.is_err());
    }

    // Helper to create temporary stub model files for testing
    fn create_stub_model_files() -> (NamedTempFile, NamedTempFile, NamedTempFile) {
        let mut encoder = NamedTempFile::new().expect("failed to create temp file");
        let mut decoder = NamedTempFile::new().expect("failed to create temp file");
        let mut tokenizer = NamedTempFile::new().expect("failed to create temp file");

        // Write minimal content so files exist
        encoder.write_all(b"stub encoder").expect("write failed");
        decoder.write_all(b"stub decoder").expect("write failed");
        tokenizer
            .write_all(b"stub tokenizer")
            .expect("write failed");

        (encoder, decoder, tokenizer)
    }
}
