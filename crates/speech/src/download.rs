//! Speech model download pipeline.
//!
//! This module handles downloading speech models from HuggingFace.
//! This is the ONLY network exception in Sena's local-first architecture.
//! Downloads are user-consented and happen during onboarding or explicit enable.

use crate::error::SpeechError;
use bus::{Event, EventBus, SpeechEvent};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use tokio::io::AsyncWriteExt;

// TODO: pin real SHA-256 checksums from HuggingFace before release.
// ggml-base.en.bin: verify against https://huggingface.co/ggerganov/whisper.cpp
/// Placeholder checksum for models without verified hashes.
const CHECKSUM_UNKNOWN: &str = "0000000000000000000000000000000000000000000000000000000000000000";

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
    /// Returns the Whisper base (English-only) model for STT (~141MB).
    ///
    /// `ggml-base.en` is the recommended model for general use:
    /// - ~141MB on disk, ~300MB RAM
    /// - English-only, ~16x realtime on average CPU
    /// - Excellent accuracy for clear speech; acceptable for light accents
    /// - Smallest model that reliably handles real-world speech input
    pub fn whisper_base_en_gguf() -> ModelInfo {
        ModelInfo {
            name: "whisper-base-en".to_string(),
            filename: "ggml-base.en.bin".to_string(),
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin"
                .to_string(),
            sha256: "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002".to_string(),
            size_bytes: 147_964_211, // 141MB actual
            model_type: ModelType::WhisperStt,
        }
    }

    /// Returns the Piper voice model for TTS (~60MB).
    pub fn piper_voice() -> ModelInfo {
        ModelInfo {
            name: "piper-en-us-lessac-medium".to_string(),
            filename: "en_US-lessac-medium.onnx".to_string(),
            // Placeholder URL — will be updated to actual HF URL
            url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/medium/en_US-lessac-medium.onnx".to_string(),
            // Placeholder checksum — will be updated to actual SHA-256
            sha256: "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
            size_bytes: 60_000_000, // ~60MB
            model_type: ModelType::PiperTts,
        }
    }

    /// Returns the OpenWakeWord model for wakeword detection (~5MB).
    pub fn open_wakeword() -> ModelInfo {
        ModelInfo {
            name: "openwakeword-hey-sena".to_string(),
            filename: "hey_sena.tflite".to_string(),
            // Placeholder URL — will be updated to actual HF URL
            url: "https://huggingface.co/davidscripka/openwakeword/resolve/main/hey_sena.tflite"
                .to_string(),
            // Placeholder checksum — will be updated to actual SHA-256
            sha256: "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
            size_bytes: 5_000_000, // ~5MB
            model_type: ModelType::OpenWakeWord,
        }
    }

    /// Returns all known models.
    pub fn all_models() -> Vec<ModelInfo> {
        vec![
            Self::whisper_base_en_gguf(),
            Self::piper_voice(),
            Self::open_wakeword(),
        ]
    }
}

/// Model cache operations.
pub struct ModelCache;

impl ModelCache {
    /// Checks if a model is cached and has valid checksum.
    pub async fn is_cached(model_dir: &Path, model: &ModelInfo) -> bool {
        let path = Self::cached_path(model_dir, model);
        if !path.exists() {
            return false;
        }

        // If checksum is unknown, only check file existence
        if model.sha256 == CHECKSUM_UNKNOWN {
            return true;
        }

        // Verify SHA-256 checksum
        Self::verify_checksum(&path, &model.sha256)
            .await
            .unwrap_or_default()
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

    /// Verifies SHA-256 checksum of a file.
    async fn verify_checksum(path: &Path, expected_sha256: &str) -> Result<bool, SpeechError> {
        // Skip verification if checksum is unknown
        if expected_sha256 == CHECKSUM_UNKNOWN {
            tracing::info!("skipping checksum verification (CHECKSUM_UNKNOWN)");
            return Ok(true);
        }

        tracing::info!("verifying checksum for {}", path.display());
        let bytes = fs::read(path)
            .await
            .map_err(|e| SpeechError::ChecksumVerificationFailed(e.to_string()))?;

        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let result = hasher.finalize();
        let actual_sha256 = hex::encode(result);

        tracing::debug!(
            "checksum verify: expected={} (len={}), actual={} (len={})",
            expected_sha256,
            expected_sha256.len(),
            actual_sha256,
            actual_sha256.len()
        );

        Ok(actual_sha256.eq_ignore_ascii_case(expected_sha256))
    }
}

/// HTTP download client for speech models.
pub struct DownloadClient {
    client: reqwest::Client,
}

impl DownloadClient {
    /// Creates a new download client.
    pub fn new() -> Result<Self, SpeechError> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(600)) // 10 min for large models
            .build()
            .map_err(|e| SpeechError::DownloadFailed(e.to_string()))?;

        Ok(Self { client })
    }

    /// Downloads a model from HuggingFace with progress reporting.
    ///
    /// # Arguments
    /// - `bus` — event bus for progress reporting
    /// - `model_dir` — directory to save the model
    /// - `model` — model metadata
    /// - `request_id` — unique request ID for event correlation
    ///
    /// # Returns
    /// Path to the downloaded model file.
    pub async fn download_model(
        &self,
        bus: &Arc<EventBus>,
        model_dir: &Path,
        model: &ModelInfo,
        request_id: u64,
    ) -> Result<PathBuf, SpeechError> {
        // Ensure model directory exists
        fs::create_dir_all(model_dir)
            .await
            .map_err(|e| SpeechError::DownloadFailed(format!("create dir: {}", e)))?;

        let path = ModelCache::cached_path(model_dir, model);
        let temp_path = path.with_extension("tmp");

        tracing::info!(
            "downloading {} from {} → {} (size: {} bytes)",
            model.name,
            model.url,
            path.display(),
            model.size_bytes
        );

        // Clean up any partial downloads
        if temp_path.exists() {
            let _ = fs::remove_file(&temp_path).await;
        }

        // Emit download started event
        let _ = bus
            .broadcast(Event::Speech(SpeechEvent::ModelDownloadStarted {
                model_name: model.name.clone(),
                total_bytes: model.size_bytes,
                request_id,
            }))
            .await;

        // Start download
        let response =
            self.client.get(&model.url).send().await.map_err(|e| {
                SpeechError::DownloadFailed(format!("network request failed: {}", e))
            })?;

        if !response.status().is_success() {
            return Err(SpeechError::DownloadFailed(format!(
                "HTTP {}: {}",
                response.status(),
                model.url
            )));
        }

        // Stream download to temp file
        let mut file = fs::File::create(&temp_path)
            .await
            .map_err(|e| SpeechError::DownloadFailed(format!("create file: {}", e)))?;

        let mut stream = response.bytes_stream();
        let mut bytes_downloaded: u64 = 0;
        let mut last_progress_report: u64 = 0;
        const PROGRESS_INTERVAL: u64 = 1_048_576; // Report every 1MB

        while let Some(chunk) = stream.next().await {
            let chunk = chunk
                .map_err(|e| SpeechError::DownloadFailed(format!("stream read failed: {}", e)))?;

            file.write_all(&chunk)
                .await
                .map_err(|e| SpeechError::DownloadFailed(format!("write failed: {}", e)))?;

            bytes_downloaded += chunk.len() as u64;

            // Report progress at intervals
            if bytes_downloaded - last_progress_report >= PROGRESS_INTERVAL
                || bytes_downloaded == model.size_bytes
            {
                let _ = bus
                    .broadcast(Event::Speech(SpeechEvent::ModelDownloadProgress {
                        model_name: model.name.clone(),
                        bytes_downloaded,
                        total_bytes: model.size_bytes,
                        request_id,
                    }))
                    .await;
                last_progress_report = bytes_downloaded;
            }
        }

        file.flush()
            .await
            .map_err(|e| SpeechError::DownloadFailed(format!("flush failed: {}", e)))?;
        drop(file);

        // Verify checksum
        let valid_checksum = ModelCache::verify_checksum(&temp_path, &model.sha256)
            .await
            .map_err(|e| SpeechError::DownloadFailed(format!("checksum verification: {}", e)))?;

        if !valid_checksum {
            // Compute actual hash BEFORE deleting the file
            let actual_bytes = fs::read(&temp_path)
                .await
                .map(|b| {
                    let mut hasher = Sha256::new();
                    hasher.update(&b);
                    hex::encode(hasher.finalize())
                })
                .unwrap_or_else(|_| "unknown".to_string());

            tracing::error!(
                "checksum mismatch for {}: expected={}, actual={} (temp_file_size={})",
                model.name,
                model.sha256,
                actual_bytes,
                fs::metadata(&temp_path).await.map(|m| m.len()).unwrap_or(0)
            );

            // Clean up corrupted file
            let _ = fs::remove_file(&temp_path).await;

            return Err(SpeechError::ChecksumMismatch {
                expected: model.sha256.clone(),
                actual: actual_bytes,
            });
        }

        // Move to final location
        fs::rename(&temp_path, &path)
            .await
            .map_err(|e| SpeechError::DownloadFailed(format!("rename failed: {}", e)))?;

        tracing::info!("model downloaded and verified: {}", path.display());

        // Emit download completed event
        let _ = bus
            .broadcast(Event::Speech(SpeechEvent::ModelDownloadCompleted {
                model_name: model.name.clone(),
                cached_path: path.to_string_lossy().to_string(),
                request_id,
            }))
            .await;

        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn model_manifest_contains_all_models() {
        let models = ModelManifest::all_models();
        assert_eq!(models.len(), 3);

        let whisper = &models[0];
        assert_eq!(whisper.model_type, ModelType::WhisperStt);
        assert_eq!(whisper.filename, "ggml-base.en.bin");

        let piper = &models[1];
        assert_eq!(piper.model_type, ModelType::PiperTts);
        assert!(piper.filename.ends_with(".onnx"));

        let wakeword = &models[2];
        assert_eq!(wakeword.model_type, ModelType::OpenWakeWord);
        assert!(wakeword.filename.ends_with(".tflite"));
    }

    #[test]
    fn cached_path_returns_correct_path() {
        let model = ModelManifest::whisper_base_en_gguf();
        let model_dir = Path::new("/tmp/models");
        let path = ModelCache::cached_path(model_dir, &model);

        assert_eq!(path, model_dir.join(&model.filename));
    }

    #[tokio::test]
    async fn is_cached_returns_false_for_nonexistent_file() {
        let temp_dir = tempdir().unwrap();
        let model = ModelManifest::whisper_base_en_gguf();

        let cached = ModelCache::is_cached(temp_dir.path(), &model).await;
        assert!(!cached);
    }

    #[tokio::test]
    async fn verify_checksum_matches_known_data() {
        let temp_dir = tempdir().unwrap();
        let test_file = temp_dir.path().join("test.bin");

        // Known test data
        let test_data = b"Hello, Sena!";
        tokio::fs::write(&test_file, test_data).await.unwrap();

        // Expected SHA-256: computed externally
        let mut hasher = Sha256::new();
        hasher.update(test_data);
        let expected = hex::encode(hasher.finalize());

        let valid = ModelCache::verify_checksum(&test_file, &expected)
            .await
            .unwrap();
        assert!(valid);
    }

    #[tokio::test]
    async fn verify_checksum_fails_on_mismatch() {
        let temp_dir = tempdir().unwrap();
        let test_file = temp_dir.path().join("test.bin");
        tokio::fs::write(&test_file, b"Different data")
            .await
            .unwrap();

        let wrong_checksum = "1111111111111111111111111111111111111111111111111111111111111111";
        let valid = ModelCache::verify_checksum(&test_file, wrong_checksum)
            .await
            .unwrap();
        assert!(!valid);
    }

    #[tokio::test]
    async fn list_cached_returns_empty_for_new_directory() {
        let temp_dir = tempdir().unwrap();
        let cached = ModelCache::list_cached(temp_dir.path()).await;
        assert_eq!(cached.len(), 0);
    }

    #[test]
    fn download_client_creation_succeeds() {
        let client = DownloadClient::new();
        assert!(client.is_ok());
    }
}
