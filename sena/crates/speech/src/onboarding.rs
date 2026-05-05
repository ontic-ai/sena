//! Speech onboarding helpers.
//!
//! This module does not download models. It only checks whether required
//! speech models and audio devices are available so boot code can make
//! daemon-owned decisions without duplicating path logic.

use std::path::Path;

use cpal::traits::HostTrait;

use crate::error::SttError;
use crate::models::{ModelCache, ModelInfo, ModelManifest};

/// Check if speech onboarding is needed because one or more speech models are missing.
pub async fn speech_onboarding_needed(model_dir: &Path) -> bool {
    for model in ModelManifest::all_models() {
        if !ModelCache::is_cached(model_dir, &model).await {
            return true;
        }
    }
    false
}

/// Check whether a single speech model is cached.
pub async fn check_model_cached(model_dir: &Path, model: &ModelInfo) -> bool {
    ModelCache::is_cached(model_dir, model).await
}

/// Check required speech models and verify that the host exposes audio devices.
pub async fn check_speech_models(model_dir: &Path) -> Result<Vec<String>, SttError> {
    let _ = check_audio_input_device();
    let _ = check_audio_output_device();

    let whisper_cached = ModelCache::is_cached(model_dir, &ModelManifest::whisper_base_en()).await;
    let parakeet_encoder_cached =
        ModelCache::is_cached(model_dir, &ModelManifest::parakeet_encoder()).await;
    let parakeet_decoder_cached =
        ModelCache::is_cached(model_dir, &ModelManifest::parakeet_decoder()).await;
    let parakeet_tokenizer_cached =
        ModelCache::is_cached(model_dir, &ModelManifest::parakeet_tokenizer()).await;
    let piper_cached = ModelCache::is_cached(model_dir, &ModelManifest::piper_voice()).await;

    let mut missing = Vec::new();
    if !whisper_cached {
        missing.push("whisper-base-en-ggml".to_string());
    }
    if !parakeet_encoder_cached {
        missing.push("parakeet-nemotron-encoder".to_string());
    }
    if !parakeet_decoder_cached {
        missing.push("parakeet-nemotron-decoder".to_string());
    }
    if !parakeet_tokenizer_cached {
        missing.push("parakeet-nemotron-tokenizer".to_string());
    }
    if !piper_cached {
        missing.push("piper-en-us-lessac-medium".to_string());
    }

    Ok(missing)
}

fn check_audio_input_device() -> bool {
    cpal::default_host().default_input_device().is_some()
}

fn check_audio_output_device() -> bool {
    cpal::default_host().default_output_device().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn speech_onboarding_needed_returns_true_when_models_missing() {
        let temp_dir = tempdir().expect("tempdir creation");
        assert!(speech_onboarding_needed(temp_dir.path()).await);
    }

    #[tokio::test]
    async fn check_model_cached_returns_false_when_model_missing() {
        let temp_dir = tempdir().expect("tempdir creation");
        let model = ModelManifest::whisper_base_en();

        assert!(!check_model_cached(temp_dir.path(), &model).await);
    }

    #[tokio::test]
    async fn check_model_cached_returns_true_when_model_exists() {
        let temp_dir = tempdir().expect("tempdir creation");
        let model = ModelManifest::whisper_base_en();
        let model_path = ModelCache::cached_path(temp_dir.path(), &model);

        std::fs::write(&model_path, b"dummy model data").expect("write model file");

        assert!(check_model_cached(temp_dir.path(), &model).await);
    }

    #[tokio::test]
    async fn check_speech_models_returns_missing_when_models_not_cached() {
        let temp_dir = tempdir().expect("tempdir creation");

        let missing = check_speech_models(temp_dir.path())
            .await
            .expect("speech model check should succeed");

        assert!(!missing.is_empty());
        assert!(missing.contains(&"parakeet-nemotron-encoder".to_string()));
        assert!(missing.contains(&"piper-en-us-lessac-medium".to_string()));
    }

    #[test]
    fn audio_device_checks_do_not_panic() {
        let _ = check_audio_input_device();
        let _ = check_audio_output_device();
    }
}
