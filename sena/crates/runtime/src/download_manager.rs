//! Download manager: ensures speech model assets are present and verified.
//!
//! This module is the source of truth for model integrity. It maintains a
//! registry of required assets with their URLs and SHA-256 checksums, and
//! provides an idempotent `ensure_models_present` function that downloads
//! missing or corrupted files.
//!
//! ## Asset Registry
//!
//! The registry includes:
//! - Parakeet/Nemotron INT8 STT models (encoder, decoder_joint, tokenizer)
//! - Piper TTS model and config (ONNX + JSON)
//!
//! ## Integrity Verification
//!
//! All assets are SHA-256 verified except the Piper JSON config, which logs
//! a warning and computes its checksum on first download for human review.

use futures::StreamExt;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tracing::{error, info, warn};

/// Download and verification errors.
#[derive(Debug, Error)]
pub enum DownloadError {
    #[error("HTTP request failed for {asset}: {source}")]
    RequestFailed {
        asset: String,
        source: reqwest::Error,
    },

    #[error("I/O error for {asset}: {source}")]
    IoError {
        asset: String,
        source: std::io::Error,
    },

    #[error("Checksum mismatch for {asset}: expected {expected}, got {computed}")]
    ChecksumMismatch {
        asset: String,
        expected: String,
        computed: String,
    },

    #[error("Failed to rename temp file for {asset}: {source}")]
    RenameFailed {
        asset: String,
        source: std::io::Error,
    },
}

/// Asset metadata: name, URL, and expected SHA-256 checksum.
struct Asset {
    name: &'static str,
    url: &'static str,
    checksum: Option<&'static str>, // None for assets pending checksum verification
}

/// Registry of all required speech model assets.
const ASSET_REGISTRY: &[Asset] = &[
    // Parakeet/Nemotron INT8 STT models
    Asset {
        name: "encoder.onnx",
        url: "https://huggingface.co/lokkju/nemotron-speech-streaming-en-0.6b-int8/resolve/main/encoder.onnx",
        checksum: Some("d24be4aff18dd9d2aa3433cb89c5a457df5015abf79e06a63dde76b1cd6386bb"),
    },
    Asset {
        name: "decoder_joint.onnx",
        url: "https://huggingface.co/lokkju/nemotron-speech-streaming-en-0.6b-int8/resolve/main/decoder_joint.onnx",
        checksum: Some("c86d527e4ae27251a741609eaddd4429ba5c32050e2f532cea1052d9e21f4f09"),
    },
    Asset {
        name: "tokenizer.model",
        url: "https://huggingface.co/lokkju/nemotron-speech-streaming-en-0.6b-int8/resolve/main/tokenizer.model",
        checksum: Some("07d4e5a63840a53ab2d4d106d2874768143fb3fbdd47938b3910d2da05bfb0a9"),
    },
    // Piper TTS model
    Asset {
        name: "en_US-lessac-high.onnx",
        url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/high/en_US-lessac-high.onnx",
        checksum: Some("4cabf7c3a638017137f34a1516522032d4fe3f38228a843cc9b764ddcbcd9e09"),
    },
    // Piper TTS config (checksum pending human review)
    Asset {
        name: "en_US-lessac-high.onnx.json",
        url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/high/en_US-lessac-high.onnx.json",
        checksum: None, // Checksum computed on first download
    },
];

/// Ensure all required model assets are present and verified.
///
/// For each asset in the registry:
/// 1. Check if the file exists at `models_dir/asset_name`
/// 2. If it exists, verify its SHA-256 checksum (skip for JSON config)
/// 3. If missing or checksum fails, download to `.tmp`, verify, and rename
/// 4. Delete `.tmp` and return error on checksum mismatch
///
/// The Piper JSON config is a special case: its checksum is computed on first
/// download and written to `docs/_scratch/piper_json_checksum.txt` for human
/// review. Subsequent runs skip checksum verification for this file and log
/// a warning.
///
/// Returns `Ok(())` only when all assets are present and verified.
pub async fn ensure_models_present(models_dir: &Path) -> Result<(), DownloadError> {
    // Create models directory if it doesn't exist
    tokio::fs::create_dir_all(models_dir)
        .await
        .map_err(|e| DownloadError::IoError {
            asset: "models_dir".to_string(),
            source: e,
        })?;

    for asset in ASSET_REGISTRY {
        let asset_path = models_dir.join(asset.name);
        let needs_download = if asset_path.exists() {
            // File exists — verify checksum if available
            match asset.checksum {
                Some(expected) => {
                    let computed = compute_checksum(&asset_path).await?;
                    if computed != expected {
                        warn!(
                            asset = asset.name,
                            expected, computed, "Checksum mismatch — re-downloading"
                        );
                        true
                    } else {
                        info!(asset = asset.name, "Verified");
                        false
                    }
                }
                None => {
                    // JSON config: skip verification, log warning
                    warn!(
                        asset = asset.name,
                        "Checksum verification skipped (pending human review)"
                    );
                    false
                }
            }
        } else {
            true
        };

        if needs_download {
            download_and_verify(asset, &asset_path).await?;
        }
    }

    Ok(())
}

/// Download an asset to a temporary file, verify checksum, and atomically rename.
async fn download_and_verify(asset: &Asset, target_path: &Path) -> Result<(), DownloadError> {
    let tmp_path = target_path.with_extension("tmp");

    info!(asset = asset.name, url = asset.url, "Downloading");

    // Download to temp file with streaming writes
    let response = reqwest::get(asset.url)
        .await
        .map_err(|e| DownloadError::RequestFailed {
            asset: asset.name.to_string(),
            source: e,
        })?
        .error_for_status()
        .map_err(|e| {
            error!(
                asset = asset.name,
                status = %e.status().unwrap_or(reqwest::StatusCode::INTERNAL_SERVER_ERROR),
                "HTTP request failed"
            );
            DownloadError::RequestFailed {
                asset: asset.name.to_string(),
                source: e,
            }
        })?;

    // Extract content length for progress tracking
    let content_length = response.content_length();
    if let Some(total) = content_length {
        info!(asset = asset.name, bytes = total, "Download size");
    }

    // Stream response body to temp file
    let mut stream = response.bytes_stream();
    let mut file =
        tokio::fs::File::create(&tmp_path)
            .await
            .map_err(|e| DownloadError::IoError {
                asset: asset.name.to_string(),
                source: e,
            })?;

    let mut total_bytes = 0;
    let mut last_logged_percent = 0u64;
    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result.map_err(|e| {
            error!(asset = asset.name, error = %e, "Failed to read response chunk");
            DownloadError::RequestFailed {
                asset: asset.name.to_string(),
                source: e,
            }
        })?;
        file.write_all(&chunk)
            .await
            .map_err(|e| DownloadError::IoError {
                asset: asset.name.to_string(),
                source: e,
            })?;
        total_bytes += chunk.len();

        // Log progress at 25% milestones if content length is known
        if let Some(total) = content_length {
            let percent = (total_bytes as u64 * 100) / total;
            if percent >= last_logged_percent + 25 && percent < 100 {
                info!(
                    asset = asset.name,
                    percent,
                    bytes = total_bytes,
                    "Download progress"
                );
                last_logged_percent = percent;
            }
        }
    }

    file.flush().await.map_err(|e| DownloadError::IoError {
        asset: asset.name.to_string(),
        source: e,
    })?;

    info!(asset = asset.name, bytes = total_bytes, "Download complete");

    // Verify checksum if available
    if let Some(expected) = asset.checksum {
        let computed = compute_checksum(&tmp_path).await?;
        if computed != expected {
            error!(
                asset = asset.name,
                expected, computed, "Checksum mismatch after download"
            );
            // Clean up temp file
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(DownloadError::ChecksumMismatch {
                asset: asset.name.to_string(),
                expected: expected.to_string(),
                computed,
            });
        }
        info!(asset = asset.name, checksum = computed, "Verified");
    } else {
        // Special case: compute and record checksum for JSON config
        let computed = compute_checksum(&tmp_path).await?;
        info!(
            asset = asset.name,
            checksum = computed,
            "Checksum computed (pending review)"
        );

        // Write to ROOT repo scratch directory (relative to crate root)
        // This crate is at sena/sena/crates/runtime, traverse up to outer repo root
        // CARGO_MANIFEST_DIR = .../sena/sena/crates/runtime
        // Need to go up 3 levels: ../../../ to reach outer repo root
        let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let checksum_file = crate_root
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .map(|repo_root| repo_root.join("docs/_scratch/piper_json_checksum.txt"))
            .unwrap_or_else(|| PathBuf::from("../../../docs/_scratch/piper_json_checksum.txt"));
        let content = format!(
            "Piper TTS JSON config checksum (computed on first download):\n\
             Asset: {}\n\
             URL: {}\n\
             SHA-256: {}\n\
             \n\
             This checksum should be reviewed and hardcoded into the asset registry.\n",
            asset.name, asset.url, computed
        );

        // Ensure scratch directory exists
        if let Some(parent) = checksum_file.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }

        if let Err(e) = tokio::fs::write(&checksum_file, content).await {
            warn!(
                path = %checksum_file.display(),
                error = %e,
                "Failed to write checksum file (non-fatal)"
            );
        } else {
            info!(
                path = %checksum_file.display(),
                "Checksum written for human review"
            );
        }
    }

    // Atomically rename temp to final path
    tokio::fs::rename(&tmp_path, target_path)
        .await
        .map_err(|e| {
            error!(
                asset = asset.name,
                error = %e,
                "Failed to rename temp file"
            );
            DownloadError::RenameFailed {
                asset: asset.name.to_string(),
                source: e,
            }
        })?;

    info!(asset = asset.name, "Installed");
    Ok(())
}

/// Compute SHA-256 checksum of a file.
async fn compute_checksum(path: &Path) -> Result<String, DownloadError> {
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|e| DownloadError::IoError {
            asset: path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string(),
            source: e,
        })?;

    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let result = hasher.finalize();
    Ok(format!("{:x}", result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn compute_checksum_returns_correct_hash() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        tokio::fs::write(&file_path, b"hello world").await.unwrap();

        let checksum = compute_checksum(&file_path).await.unwrap();
        // SHA-256 of "hello world"
        assert_eq!(
            checksum,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[tokio::test]
    async fn ensure_models_present_creates_directory() {
        let dir = tempdir().unwrap();
        let models_dir = dir.path().join("models");

        // Directory doesn't exist yet
        assert!(!models_dir.exists());

        // This will fail because we can't download in tests, but it should
        // create the directory first
        let _ = ensure_models_present(&models_dir).await;

        // Directory should now exist
        assert!(models_dir.exists());
    }

    #[test]
    fn asset_registry_is_complete() {
        // Verify registry has the expected number of assets
        assert_eq!(ASSET_REGISTRY.len(), 5);

        // Verify Nemotron assets
        assert!(
            ASSET_REGISTRY
                .iter()
                .any(|a| a.name == "encoder.onnx" && a.checksum.is_some())
        );
        assert!(
            ASSET_REGISTRY
                .iter()
                .any(|a| a.name == "decoder_joint.onnx" && a.checksum.is_some())
        );
        assert!(
            ASSET_REGISTRY
                .iter()
                .any(|a| a.name == "tokenizer.model" && a.checksum.is_some())
        );

        // Verify Piper assets
        assert!(
            ASSET_REGISTRY
                .iter()
                .any(|a| a.name == "en_US-lessac-high.onnx" && a.checksum.is_some())
        );
        assert!(
            ASSET_REGISTRY
                .iter()
                .any(|a| a.name == "en_US-lessac-high.onnx.json" && a.checksum.is_none())
        );
    }

    #[test]
    fn checksum_mismatch_error_includes_details() {
        let err = DownloadError::ChecksumMismatch {
            asset: "test.onnx".to_string(),
            expected: "abc123".to_string(),
            computed: "def456".to_string(),
        };

        let msg = err.to_string();
        assert!(msg.contains("test.onnx"));
        assert!(msg.contains("abc123"));
        assert!(msg.contains("def456"));
    }
}
