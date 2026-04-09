//! Sherpa-onnx Zipformer STT backend for listen mode.
//!
//! Wraps sherpa_rs::zipformer::ZipFormer (offline transducer model).
//! Called synchronously — callers must use spawn_blocking or a worker thread.

use crate::error::SpeechError;
use sherpa_rs::zipformer::{ZipFormer, ZipFormerConfig};
use std::path::Path;

/// sherpa-onnx Zipformer STT state, owns the loaded model.
pub(crate) struct SherpaZipformerStt {
    model: ZipFormer,
}

impl SherpaZipformerStt {
    /// Load the Zipformer model from ONNX files.
    /// This is a blocking operation — call from spawn_blocking or a dedicated thread.
    pub(crate) fn load(
        encoder: &str,
        decoder: &str,
        joiner: &str,
        tokens: &str,
    ) -> Result<Self, SpeechError> {
        let config = ZipFormerConfig {
            encoder: encoder.to_string(),
            decoder: decoder.to_string(),
            joiner: joiner.to_string(),
            tokens: tokens.to_string(),
            num_threads: Some(2),
            provider: None,
            debug: false,
        };
        let model =
            ZipFormer::new(config).map_err(|e| SpeechError::SttInitFailed(e.to_string()))?;
        Ok(Self { model })
    }

    /// Decode a batch of f32 samples at 16kHz mono and return the transcribed text.
    /// This is a blocking operation.
    pub(crate) fn decode_chunk(&mut self, samples: Vec<f32>) -> String {
        self.model.decode(16000, samples)
    }

    /// Returns true if all four model files exist in the given directory.
    pub(crate) fn models_present(model_dir: &Path) -> bool {
        model_dir.join("sherpa_encoder.onnx").exists()
            && model_dir.join("sherpa_decoder.onnx").exists()
            && model_dir.join("sherpa_joiner.onnx").exists()
            && model_dir.join("sherpa_tokens.txt").exists()
    }
}
