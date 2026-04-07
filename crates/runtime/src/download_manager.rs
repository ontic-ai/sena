//! General Sena model download manager.
//!
//! Handles downloading Sena-managed models (embedding models, etc.) from HuggingFace.
//! This is part of the network exception in Sena's local-first architecture.
//! Downloads happen automatically when a required model is missing at boot.

use bus::{DownloadEvent, Event, EventBus};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use tokio::io::AsyncWriteExt;

/// Download error types.
#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("download failed: {0}")]
    DownloadFailed(String),
    #[error("checksum verification failed: {0}")]
    ChecksumFailed(String),
    #[error("IO error: {0}")]
    Io(String),
}

/// Model metadata for a downloadable Sena model.
#[derive(Debug, Clone)]
pub struct ModelInfo {
    /// Human-readable model name.
    pub name: String,
    /// Filename on disk.
    pub filename: String,
    /// Download URL (HuggingFace or compatible).
    pub url: String,
    /// Expected SHA-256 checksum (lowercase hex).
    pub sha256: String,
    /// Approximate file size in bytes (for progress reporting).
    pub size_bytes: u64,
}

/// Known Sena-managed model manifests.
pub struct ModelManifest;

impl ModelManifest {
    /// Returns the nomic-embed-text-v1.5 Q4_K_M embedding model info (~87MB).
    ///
    /// This is Sena's default embedding model for the memory subsystem.
    /// Q4_K_M provides a good accuracy/size trade-off for CPU inference.
    pub fn nomic_embed_v1_5() -> ModelInfo {
        ModelInfo {
            name: "nomic-embed-text-v1.5".to_string(),
            filename: "nomic-embed-text-v1.5.Q4_K_M.gguf".to_string(),
            url: "https://huggingface.co/nomic-ai/nomic-embed-text-v1.5-GGUF/resolve/main/nomic-embed-text-v1.5.Q4_K_M.gguf".to_string(),
            sha256: "d4e388894e09cf3816e8b0896d81d265b55e7a9fff9ab03fe8bf4ef5e11295ac".to_string(),
            size_bytes: 91_000_000, // ~87MB (approximate)
        }
    }
}

/// Model cache operations.
pub struct ModelCache;

impl ModelCache {
    /// Returns true if the model file exists and its SHA-256 checksum matches.
    pub async fn is_cached(model_dir: &Path, model: &ModelInfo) -> bool {
        let path = Self::cached_path(model_dir, model);
        if !path.exists() {
            return false;
        }
        // If checksum is set, verify it
        if model.sha256.chars().all(|c| c == '0') {
            return true; // placeholder checksum — skip verification
        }
        Self::verify_checksum(&path, &model.sha256)
            .await
            .unwrap_or(false)
    }

    /// Returns the expected filesystem path for a cached model.
    pub fn cached_path(model_dir: &Path, model: &ModelInfo) -> PathBuf {
        model_dir.join(&model.filename)
    }

    /// Verifies SHA-256 checksum of a file.
    async fn verify_checksum(path: &Path, expected: &str) -> Result<bool, DownloadError> {
        let bytes = fs::read(path)
            .await
            .map_err(|e| DownloadError::Io(e.to_string()))?;
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let actual = hex::encode(hasher.finalize());
        Ok(actual.eq_ignore_ascii_case(expected))
    }
}

/// HTTP download client for Sena models.
pub struct DownloadClient {
    client: reqwest::Client,
}

impl DownloadClient {
    /// Creates a new download client with a 10-minute timeout.
    pub fn new() -> Result<Self, DownloadError> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(600))
            .build()
            .map_err(|e| DownloadError::DownloadFailed(e.to_string()))?;
        Ok(Self { client })
    }

    /// Downloads a model with progress events emitted on the bus.
    ///
    /// Uses a temp-file-then-rename pattern for atomic writes.
    /// Verifies SHA-256 checksum after download completes.
    pub async fn download_model(
        &self,
        bus: &Arc<EventBus>,
        model_dir: &Path,
        model: &ModelInfo,
        request_id: u64,
    ) -> Result<PathBuf, DownloadError> {
        fs::create_dir_all(model_dir)
            .await
            .map_err(|e| DownloadError::Io(format!("create dir: {}", e)))?;

        let path = ModelCache::cached_path(model_dir, model);
        let temp_path = path.with_extension("tmp");

        tracing::info!(
            "download_manager: downloading {} from {} → {}",
            model.name,
            model.url,
            path.display()
        );

        // Clean up any partial download
        if temp_path.exists() {
            let _ = fs::remove_file(&temp_path).await;
        }

        // Emit download started
        let _ = bus
            .broadcast(Event::Download(DownloadEvent::Started {
                model_name: model.name.clone(),
                total_bytes: model.size_bytes,
                request_id,
            }))
            .await;

        let response = self
            .client
            .get(&model.url)
            .send()
            .await
            .map_err(|e| DownloadError::DownloadFailed(format!("request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let err = format!("HTTP {}", status);
            let _ = bus
                .broadcast(Event::Download(DownloadEvent::Failed {
                    model_name: model.name.clone(),
                    reason: err.clone(),
                    request_id,
                }))
                .await;
            return Err(DownloadError::DownloadFailed(err));
        }

        let mut file = fs::File::create(&temp_path)
            .await
            .map_err(|e| DownloadError::Io(format!("create temp file: {}", e)))?;

        let mut stream = response.bytes_stream();
        let mut bytes_downloaded: u64 = 0;
        let mut last_reported_pct: u64 = 0;

        while let Some(chunk) = stream.next().await {
            let chunk =
                chunk.map_err(|e| DownloadError::DownloadFailed(format!("stream error: {}", e)))?;
            file.write_all(&chunk)
                .await
                .map_err(|e| DownloadError::Io(format!("write error: {}", e)))?;
            bytes_downloaded += chunk.len() as u64;

            // Emit progress every ~5%
            let pct = if model.size_bytes > 0 {
                bytes_downloaded * 100 / model.size_bytes
            } else {
                0
            };
            if pct >= last_reported_pct + 5 {
                last_reported_pct = pct;
                let _ = bus
                    .broadcast(Event::Download(DownloadEvent::Progress {
                        model_name: model.name.clone(),
                        bytes_downloaded,
                        total_bytes: model.size_bytes,
                        request_id,
                    }))
                    .await;
            }
        }

        file.flush()
            .await
            .map_err(|e| DownloadError::Io(format!("flush error: {}", e)))?;
        drop(file);

        // Checksum verification
        let ok = ModelCache::verify_checksum(&temp_path, &model.sha256)
            .await
            .unwrap_or(false);
        if !ok {
            let _ = fs::remove_file(&temp_path).await;
            let err = "SHA-256 checksum mismatch after download".to_string();
            let _ = bus
                .broadcast(Event::Download(DownloadEvent::Failed {
                    model_name: model.name.clone(),
                    reason: err.clone(),
                    request_id,
                }))
                .await;
            return Err(DownloadError::ChecksumFailed(err));
        }

        // Atomic rename
        fs::rename(&temp_path, &path)
            .await
            .map_err(|e| DownloadError::Io(format!("rename failed: {}", e)))?;

        tracing::info!(
            "download_manager: {} downloaded and verified at {}",
            model.name,
            path.display()
        );

        let _ = bus
            .broadcast(Event::Download(DownloadEvent::Completed {
                model_name: model.name.clone(),
                cached_path: path.to_string_lossy().into_owned(),
                request_id,
            }))
            .await;

        Ok(path)
    }
}
