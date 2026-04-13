//! Sherpa-onnx Zipformer STT backend for streaming transcription.
//!
//! Wraps sherpa-onnx OnlineRecognizer (ONNX Zipformer Transducer model).
//! Called synchronously — callers must use spawn_blocking or a worker thread.

use crate::error::SpeechError;
use std::path::Path;

/// Sherpa-onnx Zipformer STT state, owns the loaded model and stream.
pub struct SherpaZipformerStt {
    recognizer: sherpa_onnx::OnlineRecognizer,
    stream: sherpa_onnx::OnlineStream,
}

impl SherpaZipformerStt {
    /// Load the Zipformer model from ONNX files.
    /// This is a blocking operation — call from spawn_blocking or a dedicated thread.
    ///
    /// Expected files:
    ///   - encoder.onnx (or encoder.int8.onnx for quantized)
    ///   - decoder.onnx
    ///   - joiner.onnx (or joiner.int8.onnx for quantized)
    ///   - tokens.txt
    pub fn load(
        encoder: &str,
        decoder: &str,
        joiner: &str,
        tokens: &str,
    ) -> Result<Self, SpeechError> {
        tracing::debug!(
            "loading sherpa-onnx zipformer model: encoder={}, decoder={}, joiner={}, tokens={}",
            encoder,
            decoder,
            joiner,
            tokens
        );

        let mut config = sherpa_onnx::OnlineRecognizerConfig::default();
        config.model_config.transducer.encoder = Some(encoder.to_string());
        config.model_config.transducer.decoder = Some(decoder.to_string());
        config.model_config.transducer.joiner = Some(joiner.to_string());
        config.model_config.tokens = Some(tokens.to_string());
        config.enable_endpoint = true;
        config.decoding_method = Some("greedy_search".to_string());

        let recognizer = sherpa_onnx::OnlineRecognizer::create(&config).ok_or_else(|| {
            SpeechError::SttInitFailed("sherpa-onnx OnlineRecognizer creation failed".to_string())
        })?;

        let stream = recognizer.create_stream();

        tracing::info!("sherpa-onnx zipformer model loaded successfully");
        Ok(Self { recognizer, stream })
    }

    /// Decode a batch of f32 samples at 16kHz mono and return the transcribed text.
    /// This is a blocking operation.
    ///
    /// Audio is expected to be f32 samples normalized to [-1.0, 1.0].
    pub fn decode_chunk(&mut self, samples: Vec<f32>) -> String {
        tracing::debug!("sherpa-onnx: decoding {} samples", samples.len());

        // Feed audio to stream (16kHz assumed)
        self.stream.accept_waveform(16000, &samples);

        // Decode
        while self.recognizer.is_ready(&self.stream) {
            self.recognizer.decode(&self.stream);
        }

        // Get result
        let text = self
            .recognizer
            .get_result(&self.stream)
            .map(|result| result.text)
            .unwrap_or_default();

        if !text.trim().is_empty() {
            tracing::debug!("sherpa-onnx: transcribed \"{}\"", text.trim());
        }

        text
    }

    /// Returns true if all required Sherpa ONNX model files exist in the given directory.
    pub fn models_present(model_dir: &Path) -> bool {
        model_dir.join("encoder-epoch-99-avg-1.int8.onnx").exists()
            && model_dir.join("decoder-epoch-99-avg-1.int8.onnx").exists()
            && model_dir.join("joiner-epoch-99-avg-1.int8.onnx").exists()
            && model_dir.join("tokens.txt").exists()
    }
}
