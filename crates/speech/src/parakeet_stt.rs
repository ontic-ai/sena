//! NVIDIA Parakeet-EOU STT backend for streaming transcription.
//!
//! Wraps parakeet-rs::ParakeetEOU (ONNX-based streaming STT).
//! Called synchronously — callers must use spawn_blocking or a worker thread.

use crate::error::SpeechError;
use parakeet_rs::ParakeetEOU;
use std::path::Path;

/// Parakeet-EOU STT state, owns the loaded ONNX model.
pub struct ParakeetStt {
    model: ParakeetEOU,
}

impl ParakeetStt {
    /// Load the Parakeet-EOU model from an ONNX model directory.
    /// This is a blocking operation — call from spawn_blocking or a dedicated thread.
    ///
    /// Expected files in `model_dir`:
    ///   - `encoder.onnx`
    ///   - `decoder_joint.onnx`
    ///   - `tokenizer.json`
    pub fn load(model_dir: &Path) -> Result<Self, SpeechError> {
        tracing::debug!("loading parakeet-eou model from {}", model_dir.display());

        let model = ParakeetEOU::from_pretrained(model_dir, None)
            .map_err(|e| SpeechError::SttInitFailed(format!("parakeet model load: {}", e)))?;

        tracing::info!("parakeet-eou model loaded from {}", model_dir.display());
        Ok(Self { model })
    }

    /// Decode a chunk of i16 PCM audio samples at 16kHz mono and return transcribed text.
    /// This is a blocking operation.
    ///
    /// Audio is converted from i16 to f32 normalized to [-1.0, 1.0] before processing.
    pub fn decode_chunk(&mut self, audio: &[i16]) -> Result<String, SpeechError> {
        let audio_f32: Vec<f32> = audio.iter().map(|&s| s as f32 / 32768.0).collect();
        self.decode_chunk_f32(&audio_f32)
    }

    /// Decode a chunk of f32 audio samples at 16kHz mono and return transcribed text.
    ///
    /// Preferred over `decode_chunk` for streaming paths — avoids the lossy i16 round-trip.
    /// Feed exactly 2560 samples (160ms) per call for proper streaming behaviour.
    pub fn decode_chunk_f32(&mut self, audio: &[f32]) -> Result<String, SpeechError> {
        if audio.len() != 2560 {
            tracing::warn!(
                "parakeet: expected 2560 samples (160ms at 16kHz), got {} — model output may be degraded",
                audio.len()
            );
        }
        tracing::debug!("parakeet: decoding {} samples", audio.len());

        let text = self
            .model
            .transcribe(audio, false)
            .map_err(|e| SpeechError::TranscriptionFailed(format!("parakeet decode: {}", e)))?;

        tracing::debug!("parakeet: decoded token: {:?}", text.trim());
        Ok(text)
    }

    /// Returns true if all required Parakeet ONNX model files exist in the given directory.
    pub fn models_present(model_dir: &Path) -> bool {
        model_dir.join("encoder.onnx").exists()
            && model_dir.join("decoder_joint.onnx").exists()
            && model_dir.join("tokenizer.json").exists()
    }
}
