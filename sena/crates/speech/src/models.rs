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
    /// Parakeet encoder ONNX model for STT.
    ParakeetEncoder,
    /// Parakeet decoder ONNX model for STT.
    ParakeetDecoder,
    /// Parakeet tokenizer model for STT.
    ParakeetTokenizer,
    /// Piper voice ONNX model for TTS.
    PiperTts,
    /// Piper config JSON for TTS.
    PiperConfig,
    /// OpenWakeWord model for wakeword detection.
    OpenWakeWord,
    /// Nomic embedding model for memory vector store.
    NomicEmbed,
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
    /// Returns the Whisper base (English-only) GGML model for STT.
    ///
    /// Single GGML file used by whisper-rs. Downloaded by runtime DownloadManager.
    pub fn whisper_base_en() -> ModelInfo {
        ModelInfo {
            name: "whisper-base-en-ggml".to_string(),
            filename: "ggml-base.en.bin".to_string(),
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin"
                .to_string(),
            // TODO: Pin real SHA-256 checksum from HuggingFace
            sha256: "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
            size_bytes: 148_164_587, // ~148MB
            model_type: ModelType::WhisperStt,
        }
    }

    /// Returns the Piper voice model for TTS.
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

    /// Returns the Piper config JSON for TTS.
    pub fn piper_config() -> ModelInfo {
        ModelInfo {
            name: "piper-en-us-lessac-medium-config".to_string(),
            filename: "en_US-lessac-medium.onnx.json".to_string(),
            url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/medium/en_US-lessac-medium.onnx.json".to_string(),
            sha256: "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
            size_bytes: 5_000, // ~5KB
            model_type: ModelType::PiperConfig,
        }
    }

    /// Returns the Parakeet encoder ONNX model for STT.
    pub fn parakeet_encoder() -> ModelInfo {
        ModelInfo {
            name: "parakeet-nemotron-encoder".to_string(),
            filename: "encoder.onnx".to_string(),
            url: "https://huggingface.co/lokkju/nemotron-speech-streaming-en-0.6b-int8/resolve/main/encoder.onnx"
                .to_string(),
            sha256: "d24be4aff18dd9d2aa3433cb89c5a457df5015abf79e06a63dde76b1cd6386bb".to_string(),
            size_bytes: 450_000_000, // ~450MB
            model_type: ModelType::ParakeetEncoder,
        }
    }

    /// Returns the Parakeet decoder ONNX model for STT.
    pub fn parakeet_decoder() -> ModelInfo {
        ModelInfo {
            name: "parakeet-nemotron-decoder".to_string(),
            filename: "decoder_joint.onnx".to_string(),
            url: "https://huggingface.co/lokkju/nemotron-speech-streaming-en-0.6b-int8/resolve/main/decoder_joint.onnx".to_string(),
            sha256: "c86d527e4ae27251a741609eaddd4429ba5c32050e2f532cea1052d9e21f4f09".to_string(),
            size_bytes: 50_000_000, // ~50MB
            model_type: ModelType::ParakeetDecoder,
        }
    }

    /// Returns the Parakeet tokenizer model for STT.
    pub fn parakeet_tokenizer() -> ModelInfo {
        ModelInfo {
            name: "parakeet-nemotron-tokenizer".to_string(),
            filename: "tokenizer.model".to_string(),
            url:
                "https://huggingface.co/lokkju/nemotron-speech-streaming-en-0.6b-int8/resolve/main/tokenizer.model"
                    .to_string(),
            sha256: "07d4e5a63840a53ab2d4d106d2874768143fb3fbdd47938b3910d2da05bfb0a9".to_string(),
            size_bytes: 2_500_000, // ~2.5MB
            model_type: ModelType::ParakeetTokenizer,
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

    /// Returns the Nomic embed-text v1.5 GGUF model for vector embeddings.
    ///
    /// Required for memory subsystem operation. Boot will fail if this model
    /// is missing and cannot be downloaded.
    pub fn nomic_embed_text() -> ModelInfo {
        ModelInfo {
            name: "nomic-embed-text-v1.5-Q8_0".to_string(),
            filename: "nomic-embed-text-v1.5.Q8_0.gguf".to_string(),
            url: "https://huggingface.co/nomic-ai/nomic-embed-text-v1.5-GGUF/resolve/main/nomic-embed-text-v1.5.Q8_0.gguf".to_string(),
            sha256: "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
            size_bytes: 274_000_000, // ~274MB
            model_type: ModelType::NomicEmbed,
        }
    }

    /// Returns all speech models (does not include required embedding model).
    pub fn all_models() -> Vec<ModelInfo> {
        vec![
            Self::whisper_base_en(),
            Self::parakeet_encoder(),
            Self::parakeet_decoder(),
            Self::parakeet_tokenizer(),
            Self::piper_voice(),
            Self::piper_config(),
            Self::open_wakeword(),
        ]
    }

    /// Returns the required embedding model.
    ///
    /// This model is a boot dependency and must be present for Sena to run.
    pub fn required_embed_model() -> ModelInfo {
        Self::nomic_embed_text()
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
        assert_eq!(models.len(), 7);

        // Whisper
        let whisper = &models[0];
        assert_eq!(whisper.model_type, ModelType::WhisperStt);
        assert_eq!(whisper.filename, "ggml-base.en.bin");

        // Parakeet (encoder, decoder, tokenizer)
        let parakeet_encoder = &models[1];
        assert_eq!(parakeet_encoder.model_type, ModelType::ParakeetEncoder);
        assert_eq!(parakeet_encoder.filename, "encoder.onnx");

        let parakeet_decoder = &models[2];
        assert_eq!(parakeet_decoder.model_type, ModelType::ParakeetDecoder);
        assert_eq!(parakeet_decoder.filename, "decoder_joint.onnx");

        let parakeet_tokenizer = &models[3];
        assert_eq!(parakeet_tokenizer.model_type, ModelType::ParakeetTokenizer);
        assert_eq!(parakeet_tokenizer.filename, "tokenizer.model");

        // Piper (onnx + config)
        let piper = &models[4];
        assert_eq!(piper.model_type, ModelType::PiperTts);
        assert!(piper.filename.ends_with(".onnx"));

        let piper_config = &models[5];
        assert_eq!(piper_config.model_type, ModelType::PiperConfig);
        assert!(piper_config.filename.ends_with(".onnx.json"));

        // OpenWakeWord
        let wakeword = &models[6];
        assert_eq!(wakeword.model_type, ModelType::OpenWakeWord);
        assert!(wakeword.filename.ends_with(".tflite"));
    }

    #[test]
    fn cached_path_returns_correct_path() {
        let model = ModelManifest::whisper_base_en();
        let model_dir = Path::new("/tmp/models");
        let path = ModelCache::cached_path(model_dir, &model);

        assert_eq!(path, model_dir.join(&model.filename));
    }

    #[tokio::test]
    async fn is_cached_returns_false_for_nonexistent_file() {
        let temp_dir = tempdir().expect("tempdir creation");
        let model = ModelManifest::whisper_base_en();

        let cached = ModelCache::is_cached(temp_dir.path(), &model).await;
        assert!(!cached);
    }

    #[tokio::test]
    async fn is_cached_returns_true_for_existing_file() {
        let temp_dir = tempdir().expect("tempdir creation");
        let model = ModelManifest::whisper_base_en();

        // Create the model file
        let model_path = ModelCache::cached_path(temp_dir.path(), &model);
        std::fs::write(&model_path, b"dummy model data").expect("write model file");

        let cached = ModelCache::is_cached(temp_dir.path(), &model).await;
        assert!(cached);
    }

    #[tokio::test]
    async fn list_cached_returns_empty_for_new_directory() {
        let temp_dir = tempdir().expect("tempdir creation");
        let cached = ModelCache::list_cached(temp_dir.path()).await;
        assert_eq!(cached.len(), 0);
    }

    #[tokio::test]
    async fn list_cached_returns_models_that_exist() {
        let temp_dir = tempdir().expect("tempdir creation");

        // Create one model file
        let whisper = ModelManifest::whisper_base_en();
        let whisper_path = ModelCache::cached_path(temp_dir.path(), &whisper);
        std::fs::write(&whisper_path, b"dummy whisper data").expect("write whisper file");

        let cached = ModelCache::list_cached(temp_dir.path()).await;
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].filename, whisper.filename);
    }

    #[test]
    fn required_embed_model_is_nomic_embed() {
        let embed_model = ModelManifest::required_embed_model();
        assert_eq!(embed_model.model_type, ModelType::NomicEmbed);
        assert_eq!(embed_model.filename, "nomic-embed-text-v1.5.Q8_0.gguf");
        assert!(embed_model.size_bytes > 0);
    }
}
