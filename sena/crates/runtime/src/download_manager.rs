//! Model download manager.
//!
//! Handles downloading Sena-managed models (embedding models, speech models) from HuggingFace.
//! This is part of the network exception in Sena's local-first architecture.
//! Downloads happen on-demand when a required model is missing.
//!
//! ## Design
//!
//! - Uses speech crate's ModelManifest for speech model metadata (no duplication).
//! - Emits DownloadEvent lifecycle events on the bus.
//! - Temp-file-then-rename for atomic writes.
//! - SHA-256 checksum verification after download.
//! - No hardcoded model metadata in runtime — delegates to speech crate.

use bus::{DownloadEvent, Event, EventBus};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use speech::ModelInfo;
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

/// Model cache operations.
pub struct ModelCache;

impl ModelCache {
    /// Returns the expected filesystem path for a cached model.
    pub fn cached_path(model_dir: &Path, model: &ModelInfo) -> PathBuf {
        model_dir.join(&model.filename)
    }

    /// Verifies SHA-256 checksum of a file.
    ///
    /// Skips verification if the checksum is all zeros (placeholder).
    async fn verify_checksum(path: &Path, expected: &str) -> Result<bool, DownloadError> {
        // Skip verification for placeholder checksums
        if expected.chars().all(|c| c == '0') {
            return Ok(true);
        }

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
            let pct = bytes_downloaded
                .checked_mul(100)
                .and_then(|value| value.checked_div(model.size_bytes))
                .unwrap_or(0);
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
        let ok = ModelCache::verify_checksum(&temp_path, &model.sha256).await?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use speech::ModelManifest;
    use tempfile::tempdir;

    #[test]
    fn cached_path_construction() {
        let dir = tempdir().expect("create tempdir");
        let model = ModelManifest::whisper_base_en();
        let path = ModelCache::cached_path(dir.path(), &model);
        assert_eq!(
            path.file_name()
                .expect("cached path should have a filename"),
            "ggml-base.en.bin"
        );
    }

    #[tokio::test]
    async fn verify_checksum_skips_placeholder() {
        let dir = tempdir().expect("create tempdir");
        let test_file = dir.path().join("test.bin");
        tokio::fs::write(&test_file, b"dummy content")
            .await
            .expect("write test file");

        // Placeholder checksum (all zeros) should skip verification
        let ok = ModelCache::verify_checksum(
            &test_file,
            "0000000000000000000000000000000000000000000000000000000000000000",
        )
        .await
        .expect("checksum verification");
        assert!(ok);
    }

    #[tokio::test]
    async fn verify_checksum_fails_on_mismatch() {
        let dir = tempdir().expect("create tempdir");
        let test_file = dir.path().join("test.bin");
        tokio::fs::write(&test_file, b"dummy content")
            .await
            .expect("write test file");

        // Wrong checksum should fail
        let ok = ModelCache::verify_checksum(
            &test_file,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .await
        .expect("checksum verification");
        assert!(!ok);
    }

    #[test]
    fn download_client_creation() {
        let client = DownloadClient::new();
        assert!(client.is_ok());
    }
}
