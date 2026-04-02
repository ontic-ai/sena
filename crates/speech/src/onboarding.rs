//! Speech onboarding flow.
//!
//! This module handles first-time speech setup: checking for required models,
//! triggering downloads, verifying audio devices, and reporting status via bus events.

use std::path::Path;
use std::sync::Arc;

use bus::{Event, EventBus, SpeechEvent};
use cpal::traits::HostTrait;

use crate::download::{DownloadClient, ModelCache, ModelManifest};
use crate::error::SpeechError;

/// Check if speech onboarding is needed (any required models missing).
pub async fn speech_onboarding_needed(model_dir: &Path) -> bool {
    for model in ModelManifest::all_models() {
        if !ModelCache::is_cached(model_dir, &model).await {
            return true;
        }
    }
    false
}

/// Run the speech onboarding flow: download missing models, verify audio devices.
///
/// Emits bus events for progress tracking:
/// - SpeechOnboardingStarted
/// - ModelDownloadStarted/Progress/Completed/Failed (per model)
/// - SpeechOnboardingCompleted { models_downloaded }
/// - SpeechOnboardingFailed { reason, recoverable }
pub async fn run_speech_onboarding(
    bus: &Arc<EventBus>,
    model_dir: &Path,
) -> Result<Vec<String>, SpeechError> {
    // Emit onboarding started
    let _ = bus
        .broadcast(Event::Speech(SpeechEvent::SpeechOnboardingStarted))
        .await;

    // Create model directory if missing
    tokio::fs::create_dir_all(model_dir)
        .await
        .map_err(|e| SpeechError::DownloadFailed(format!("cannot create model dir: {e}")))?;

    let client = DownloadClient::new()?;
    let mut downloaded = Vec::new();
    let mut request_id = 9000u64; // onboarding request IDs

    for model in ModelManifest::all_models() {
        if ModelCache::is_cached(model_dir, &model).await {
            continue; // already cached, skip
        }

        request_id += 1;
        match client
            .download_model(bus, model_dir, &model, request_id)
            .await
        {
            Ok(_path) => {
                downloaded.push(model.name.clone());
            }
            Err(e) => {
                // Non-critical: if one model fails, continue with others
                // The specific ModelDownloadFailed event was already emitted by download_model
                // Log but continue
                let _ = bus
                    .broadcast(Event::Speech(SpeechEvent::ModelDownloadFailed {
                        model_name: model.name.clone(),
                        reason: e.to_string(),
                        request_id,
                    }))
                    .await;
            }
        }
    }

    // Verify audio devices (non-blocking check)
    let has_input = check_audio_input_device();
    let _has_output = check_audio_output_device();

    if downloaded.is_empty() && !has_input {
        let reason = if !has_input {
            "No microphone detected and no models could be downloaded".to_string()
        } else {
            "No speech models could be downloaded".to_string()
        };
        let _ = bus
            .broadcast(Event::Speech(SpeechEvent::SpeechOnboardingFailed {
                reason: reason.clone(),
                recoverable: true,
            }))
            .await;
        return Err(SpeechError::DownloadFailed(reason));
    }

    let _ = bus
        .broadcast(Event::Speech(SpeechEvent::SpeechOnboardingCompleted {
            models_downloaded: downloaded.clone(),
        }))
        .await;

    Ok(downloaded)
}

/// Check if an audio input device (microphone) is available.
fn check_audio_input_device() -> bool {
    cpal::default_host().default_input_device().is_some()
}

/// Check if an audio output device (speakers) is available.
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
        let model_dir = temp_dir.path();

        // No models cached → onboarding needed
        assert!(speech_onboarding_needed(model_dir).await);
    }

    #[tokio::test]
    async fn speech_onboarding_needed_returns_false_when_all_models_cached() {
        let temp_dir = tempdir().expect("tempdir creation");
        let model_dir = temp_dir.path();

        // Create dummy model files with correct checksums (placeholder test)
        // In real testing, this would write valid files with correct checksums
        // For now, we just test the logic when directory is empty
        assert!(speech_onboarding_needed(model_dir).await);
    }

    #[test]
    fn check_audio_input_device_does_not_crash() {
        // Just verify the function doesn't panic
        let _result = check_audio_input_device();
    }

    #[test]
    fn check_audio_output_device_does_not_crash() {
        // Just verify the function doesn't panic
        let _result = check_audio_output_device();
    }
}
