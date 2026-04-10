//! Speech model metadata and path resolution.
//!
//! This module defines speech model metadata (URLs, checksums, filenames) and
//! provides path resolution and existence checking — NO download functionality.
//! Downloads are handled by the runtime's DownloadManager.

use std::path::{Path, PathBuf};

/// Speech model type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelType {
    /// Whisper GGUF model for STT.
    WhisperStt,
    /// Piper voice model for TTS.
    PiperTts,
    /// OpenWakeWord model for wakeword detection.
    OpenWakeWord,
}

/// Speech model information.
#[derive(Debug, Clone)]
pub struct ModelInfo {
    /// Model name (human-readable).
    pub name: String,
    /// Filename on disk.
    pub filename: String,
    /// HuggingFace URL.
    pub url: String,
    /// Expected SHA-256 checksum.
    pub sha256: String,
    /// File size in bytes.
    pub size_bytes: u64,
    /// Model type.
    pub model_type: ModelType,
}

/// Known speech models with their HuggingFace metadata.
pub struct ModelManifest;

impl ModelManifest {
    /// Returns the Whisper base (English-only) model for STT.
    ///
    /// Candle-compatible models require three files:
    /// - model.safetensors (or model.gguf for quantized)
    /// - config.json
    /// - tokenizer.json
    ///
    /// This manifest entry points to the safetensors model.
    /// Companion config and tokenizer files must be downloaded separately
    /// (see `whisper_config()` and `whisper_tokenizer()`).
    pub fn whisper_base_en_safetensors() -> ModelInfo {
        ModelInfo {
            name: "whisper-base-en-safetensors".to_string(),
            filename: "model.safetensors".to_string(),
            url: "https://huggingface.co/openai/whisper-base.en/resolve/main/model.safetensors"
                .to_string(),
            sha256: "d4dd5542fd6a1d35639e21384238f3bfe6c557c849d392b5905d33ee29e71db5".to_string(),
            size_bytes: 290_000_000, // ~290MB
            model_type: ModelType::WhisperStt,
        }
    }

    /// Returns the Whisper config.json for candle.
    pub fn whisper_config() -> ModelInfo {
        ModelInfo {
            name: "whisper-config".to_string(),
            filename: "config.json".to_string(),
            url: "https://huggingface.co/openai/whisper-base.en/resolve/main/config.json"
                .to_string(),
            sha256: "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
            size_bytes: 2_000, // ~2KB
            model_type: ModelType::WhisperStt,
        }
    }

    /// Returns the Whisper tokenizer.json for candle.
    pub fn whisper_tokenizer() -> ModelInfo {
        ModelInfo {
            name: "whisper-tokenizer".to_string(),
            filename: "tokenizer.json".to_string(),
            url: "https://huggingface.co/openai/whisper-base.en/resolve/main/tokenizer.json"
                .to_string(),
            sha256: "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
            size_bytes: 2_000_000, // ~2MB
            model_type: ModelType::WhisperStt,
        }
    }

    /// Returns the Piper voice model for TTS (~60MB).
    pub fn piper_voice() -> ModelInfo {
        ModelInfo {
            name: "piper-en-us-lessac-medium".to_string(),
            filename: "en_US-lessac-medium.onnx".to_string(),
            url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/medium/en_US-lessac-medium.onnx".to_string(),
            sha256: "5efe09e69902187827af646e1a6e9d269dee769f9877d17b16b1b46eeaaf019f".to_string(),
            size_bytes: 63_200_000, // ~63.2MB
            model_type: ModelType::PiperTts,
        }
    }

    /// Returns the OpenWakeWord model for wakeword detection (~5MB).
    pub fn open_wakeword() -> ModelInfo {
        ModelInfo {
            name: "openwakeword-hey-sena".to_string(),
            filename: "hey_sena.tflite".to_string(),
            url: "https://huggingface.co/davidscripka/openwakeword/resolve/main/hey_sena.tflite"
                .to_string(),
            sha256: "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
            size_bytes: 5_000_000, // ~5MB
            model_type: ModelType::OpenWakeWord,
        }
    }

    /// Returns all known models.
    pub fn all_models() -> Vec<ModelInfo> {
        vec![
            Self::whisper_base_en_safetensors(),
            Self::whisper_config(),
            Self::whisper_tokenizer(),
            Self::piper_voice(),
            Self::open_wakeword(),
        ]
    }
}

/// Model cache operations (path resolution and existence checking only).
pub struct ModelCache;

impl ModelCache {
    /// Checks if a model file exists on disk.
    ///
    /// NOTE: This does NOT verify checksums. Checksum verification is the runtime
    /// DownloadManager's responsibility.
    pub async fn is_cached(model_dir: &Path, model: &ModelInfo) -> bool {
        let path = Self::cached_path(model_dir, model);
        path.exists()
    }

    /// Returns the expected path for a cached model.
    pub fn cached_path(model_dir: &Path, model: &ModelInfo) -> PathBuf {
        model_dir.join(&model.filename)
    }

    /// Lists all cached models in the directory.
    pub async fn list_cached(model_dir: &Path) -> Vec<ModelInfo> {
        let mut cached = Vec::new();

        for model in ModelManifest::all_models() {
            if Self::is_cached(model_dir, &model).await {
                cached.push(model);
            }
        }

        cached
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn model_manifest_contains_all_models() {
        let models = ModelManifest::all_models();
        assert_eq!(models.len(), 5);

        let whisper = &models[0];
        assert_eq!(whisper.model_type, ModelType::WhisperStt);
        assert_eq!(whisper.filename, "model.safetensors");

        let whisper_cfg = &models[1];
        assert_eq!(whisper_cfg.model_type, ModelType::WhisperStt);
        assert_eq!(whisper_cfg.filename, "config.json");

        let whisper_tok = &models[2];
        assert_eq!(whisper_tok.model_type, ModelType::WhisperStt);
        assert_eq!(whisper_tok.filename, "tokenizer.json");

        let piper = &models[3];
        assert_eq!(piper.model_type, ModelType::PiperTts);
        assert!(piper.filename.ends_with(".onnx"));

        let wakeword = &models[4];
        assert_eq!(wakeword.model_type, ModelType::OpenWakeWord);
        assert!(wakeword.filename.ends_with(".tflite"));
    }

    #[test]
    fn cached_path_returns_correct_path() {
        let model = ModelManifest::whisper_base_en_safetensors();
        let model_dir = Path::new("/tmp/models");
        let path = ModelCache::cached_path(model_dir, &model);

        assert_eq!(path, model_dir.join(&model.filename));
    }

    #[tokio::test]
    async fn is_cached_returns_false_for_nonexistent_file() {
        let temp_dir = tempdir().expect("tempdir creation");
        let model = ModelManifest::whisper_base_en_safetensors();

        let cached = ModelCache::is_cached(temp_dir.path(), &model).await;
        assert!(!cached);
    }

    #[tokio::test]
    async fn list_cached_returns_empty_for_new_directory() {
        let temp_dir = tempdir().expect("tempdir creation");
        let cached = ModelCache::list_cached(temp_dir.path()).await;
        assert_eq!(cached.len(), 0);
    }
}
