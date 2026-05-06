//! Piper TTS backend using direct ONNX Runtime.

use crate::backend::TtsBackend;
use crate::error::TtsError;
use crate::types::AudioStream;
use espeak_rs::text_to_phonemes;
use ndarray::{Array1, Array2};
use ort::session::Session;
use ort::value::Value;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;
use std::time::Instant;

/// Piper model configuration (from .onnx.json file).
#[derive(Debug, Deserialize)]
struct PiperConfig {
    audio: AudioConfig,
    espeak: ESpeakConfig,
    inference: InferenceConfig,
    num_speakers: u32,
    #[serde(default)]
    phoneme_id_map: HashMap<char, Vec<i64>>,
}

#[derive(Debug, Deserialize)]
struct AudioConfig {
    sample_rate: u32,
}

#[derive(Debug, Deserialize)]
struct ESpeakConfig {
    voice: String,
}

#[derive(Debug, Deserialize)]
struct InferenceConfig {
    noise_scale: f32,
    length_scale: f32,
    noise_w: f32,
}

const BOS: char = '^';
const EOS: char = '$';
const PAD: char = '_';

/// Piper TTS backend for high-quality local speech synthesis.
///
/// Uses ONNX Runtime directly with espeak-rs for phoneme conversion.
pub struct PiperTtsBackend {
    /// ONNX Runtime session.
    session: Session,
    /// Model configuration.
    config: PiperConfig,
    /// Cached phoneme ID mappings.
    pad_id: i64,
    bos_id: i64,
    eos_id: i64,
    speaking_rate: f32,
    pitch_scale: f32,
    last_interrupt: Option<Instant>,
}

impl PiperTtsBackend {
    /// Create a new Piper TTS backend with validated asset paths.
    ///
    /// # Arguments
    /// * `model_path` - Path to the ONNX model file (e.g., en_US-lessac-high.onnx)
    /// * `config_path` - Path to the model config JSON (e.g., en_US-lessac-high.onnx.json)
    ///
    /// # Errors
    /// Returns `TtsError::InitializationFailed` if any asset path is invalid or model cannot be loaded.
    pub fn new(model_path: PathBuf, config_path: PathBuf) -> Result<Self, TtsError> {
        // Validate that required model files exist
        if !model_path.exists() {
            return Err(TtsError::InitializationFailed(format!(
                "model file not found: {}",
                model_path.display()
            )));
        }
        if !config_path.exists() {
            return Err(TtsError::InitializationFailed(format!(
                "config file not found: {}",
                config_path.display()
            )));
        }

        // Load and parse the config file
        let config_file = File::open(&config_path).map_err(|e| {
            TtsError::InitializationFailed(format!("failed to open config file: {}", e))
        })?;
        let config: PiperConfig = serde_json::from_reader(config_file).map_err(|e| {
            TtsError::InitializationFailed(format!("failed to parse config JSON: {}", e))
        })?;

        // Extract meta-token IDs
        let pad_id = *config
            .phoneme_id_map
            .get(&PAD)
            .and_then(|v| v.first())
            .ok_or_else(|| {
                TtsError::InitializationFailed("PAD token not found in phoneme_id_map".to_string())
            })?;
        let bos_id = *config
            .phoneme_id_map
            .get(&BOS)
            .and_then(|v| v.first())
            .ok_or_else(|| {
                TtsError::InitializationFailed("BOS token not found in phoneme_id_map".to_string())
            })?;
        let eos_id = *config
            .phoneme_id_map
            .get(&EOS)
            .and_then(|v| v.first())
            .ok_or_else(|| {
                TtsError::InitializationFailed("EOS token not found in phoneme_id_map".to_string())
            })?;

        // Create ONNX Runtime session
        let session = Session::builder()
            .map_err(|e| {
                TtsError::InitializationFailed(format!("failed to create session builder: {}", e))
            })?
            .commit_from_file(&model_path)
            .map_err(|e| {
                TtsError::InitializationFailed(format!("failed to load ONNX model: {}", e))
            })?;

        Ok(Self {
            session,
            config,
            pad_id,
            bos_id,
            eos_id,
            speaking_rate: 1.0,
            pitch_scale: 1.0,
            last_interrupt: None,
        })
    }

    /// Convert text to phoneme IDs using espeak.
    fn text_to_phoneme_ids(&self, text: &str) -> Result<Vec<i64>, TtsError> {
        // Use espeak to phonemize the text
        let phonemes = text_to_phonemes(text, &self.config.espeak.voice, None, true, false)
            .map_err(|e| {
                TtsError::SynthesisFailed(format!("espeak phonemization failed: {}", e))
            })?;

        // Convert phonemes to IDs
        // espeak returns Vec<String>, iterate over words and then chars
        let mut phoneme_ids = Vec::new();
        phoneme_ids.push(self.bos_id);
        for word in &phonemes {
            for phoneme in word.chars() {
                if let Some(id_list) = self.config.phoneme_id_map.get(&phoneme)
                    && let Some(&id) = id_list.first()
                {
                    phoneme_ids.push(id);
                    phoneme_ids.push(self.pad_id);
                }
            }
        }
        phoneme_ids.push(self.eos_id);

        Ok(phoneme_ids)
    }

    fn apply_pitch_scale(samples: &[f32], pitch_scale: f32) -> Vec<f32> {
        if samples.is_empty() || (pitch_scale - 1.0).abs() < f32::EPSILON {
            return samples.to_vec();
        }

        let scale = pitch_scale.max(0.1);
        let target_len = ((samples.len() as f32) / scale).round().max(1.0) as usize;
        let mut output = Vec::with_capacity(target_len);

        for index in 0..target_len {
            let source_position = (index as f32) * scale;
            let lower_index = source_position.floor() as usize;
            let upper_index = (lower_index + 1).min(samples.len().saturating_sub(1));
            let fraction = source_position - lower_index as f32;

            let lower = samples[lower_index.min(samples.len().saturating_sub(1))];
            let upper = samples[upper_index];
            output.push(lower + (upper - lower) * fraction);
        }

        output
    }
}

impl TtsBackend for PiperTtsBackend {
    fn synthesize(&mut self, text: &str) -> Result<AudioStream, TtsError> {
        let synth_started_at = Instant::now();

        // Validate input
        if text.is_empty() {
            return Err(TtsError::InvalidInput(
                "cannot synthesize empty text".to_string(),
            ));
        }

        // Convert text to phoneme IDs
        let phoneme_ids = self.text_to_phoneme_ids(text)?;
        let input_len = phoneme_ids.len();

        // Prepare input tensors
        let phoneme_inputs =
            Array2::<i64>::from_shape_vec((1, input_len), phoneme_ids).map_err(|e| {
                TtsError::SynthesisFailed(format!("failed to create phoneme tensor: {}", e))
            })?;
        let input_lengths = Array1::<i64>::from_elem(1, input_len as i64);
        let scales = Array1::<f32>::from_vec(vec![
            self.config.inference.noise_scale,
            self.config.inference.length_scale / self.speaking_rate.max(0.1),
            self.config.inference.noise_w,
        ]);

        // Create ONNX input values - convert to SessionInputValue
        let mut input_values = vec![
            Value::from_array(phoneme_inputs)
                .map_err(|e| {
                    TtsError::SynthesisFailed(format!("failed to create input value: {}", e))
                })?
                .into(),
            Value::from_array(input_lengths)
                .map_err(|e| {
                    TtsError::SynthesisFailed(format!("failed to create length value: {}", e))
                })?
                .into(),
            Value::from_array(scales)
                .map_err(|e| {
                    TtsError::SynthesisFailed(format!("failed to create scales value: {}", e))
                })?
                .into(),
        ];

        // Add speaker ID if multi-speaker model
        if self.config.num_speakers > 1 {
            let speaker_id = Array1::<i64>::from_elem(1, 0i64);
            input_values.push(
                Value::from_array(speaker_id)
                    .map_err(|e| {
                        TtsError::SynthesisFailed(format!("failed to create speaker value: {}", e))
                    })?
                    .into(),
            );
        }

        // Run inference - use input_values slice
        let pitch_scale = self.pitch_scale;
        let outputs = self
            .session
            .run(&input_values[..])
            .map_err(|e| TtsError::SynthesisFailed(format!("ONNX inference failed: {}", e)))?;

        if self
            .last_interrupt
            .is_some_and(|interrupted_at| interrupted_at >= synth_started_at)
        {
            return Err(TtsError::BackendError(
                "synthesis cancelled before audio could be delivered".to_string(),
            ));
        }

        // Extract audio samples from output tensor
        let audio_tensor = outputs[0].try_extract_tensor::<f32>().map_err(|e| {
            TtsError::SynthesisFailed(format!("failed to extract audio tensor: {}", e))
        })?;
        let samples = Self::apply_pitch_scale(audio_tensor.1, pitch_scale);

        Ok(AudioStream::new(samples, self.config.audio.sample_rate))
    }

    fn set_prosody(&mut self, speaking_rate: f32, pitch_scale: f32) {
        self.speaking_rate = speaking_rate;
        self.pitch_scale = pitch_scale;
    }

    fn cancel(&mut self) {
        self.last_interrupt = Some(Instant::now());
    }

    fn flush_buffer(&mut self) {
        self.last_interrupt = Some(Instant::now());
    }

    fn backend_name(&self) -> &'static str {
        "piper-onnx"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn piper_backend_initialization_fails_on_missing_model() {
        let (_, config) = create_stub_model_files();
        let result = PiperTtsBackend::new(
            PathBuf::from("/nonexistent/model.onnx"),
            config.path().to_path_buf(),
        );

        assert!(result.is_err());
        assert!(matches!(result, Err(TtsError::InitializationFailed(_))));
    }

    #[test]
    fn piper_backend_initialization_fails_on_missing_config() {
        let (model, _) = create_stub_model_files();
        let result = PiperTtsBackend::new(
            model.path().to_path_buf(),
            PathBuf::from("/nonexistent/config.json"),
        );

        assert!(result.is_err());
    }

    #[test]
    fn piper_config_parsing_fails_on_invalid_json() {
        let mut model = NamedTempFile::new().expect("failed to create temp file");
        let mut config = NamedTempFile::new().expect("failed to create temp file");

        model.write_all(b"stub model").expect("write failed");
        config.write_all(b"{invalid json").expect("write failed");

        let result = PiperTtsBackend::new(model.path().to_path_buf(), config.path().to_path_buf());

        assert!(result.is_err());
        if let Err(e) = result {
            let err_msg = e.to_string();
            assert!(err_msg.contains("failed to parse config JSON"));
        }
    }

    // Helper to create temporary stub model files for testing
    fn create_stub_model_files() -> (NamedTempFile, NamedTempFile) {
        let mut model = NamedTempFile::new().expect("failed to create temp file");
        let mut config = NamedTempFile::new().expect("failed to create temp file");

        // Write minimal ONNX content
        model.write_all(b"stub model").expect("write failed");

        // Write a valid minimal Piper config with required fields
        let config_json = r#"{
            "audio": {"sample_rate": 22050},
            "espeak": {"voice": "en-us"},
            "inference": {"noise_scale": 0.667, "length_scale": 1.0, "noise_w": 0.8},
            "num_speakers": 1,
            "phoneme_id_map": {
                "^": [0],
                "$": [1],
                "_": [2],
                "a": [3],
                "b": [4]
            }
        }"#;
        config
            .write_all(config_json.as_bytes())
            .expect("write failed");

        (model, config)
    }
}
