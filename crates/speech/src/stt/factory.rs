//! Factory function for constructing STT backends from configuration.

use std::path::Path;

use crate::SpeechError;
use crate::SttBackendKind;
use super::backend_trait::SttBackend;
use super::mock_backend::MockSttBackend;
use super::whisper_backend::WhisperSttBackend;
use super::sherpa_backend::SherpaSttBackend;
use super::parakeet_backend::ParakeetSttBackend;

/// Construct the concrete `SttBackend` implementation for the given `kind`.
///
/// Heavy model loading is dispatched via `tokio::task::spawn_blocking` so that
/// the async executor is not blocked during initialization.
///
/// # Errors
/// Returns `SpeechError::SttInitFailed` if model files are missing or fail to load.
pub async fn build_stt_backend(
    kind: SttBackendKind,
    model_dir: Option<&Path>,
    whisper_model_path: Option<&str>,
    stt_energy_threshold: f32,
) -> Result<Box<dyn SttBackend>, SpeechError> {
    match kind {
        SttBackendKind::Mock => Ok(Box::new(MockSttBackend)),

        SttBackendKind::Whisper => {
            let model_dir = model_dir
                .ok_or_else(|| {
                    SpeechError::SttInitFailed("model_dir required for Whisper".to_string())
                })?
                .to_path_buf();
            let whisper_model_path = whisper_model_path.map(str::to_string);

            let backend = tokio::task::spawn_blocking(move || {
                WhisperSttBackend::new(
                    &model_dir,
                    whisper_model_path.as_deref(),
                    stt_energy_threshold,
                )
            })
            .await
            .map_err(|e| {
                SpeechError::SttInitFailed(format!("spawn_blocking panicked: {}", e))
            })??;

            Ok(Box::new(backend))
        }

        SttBackendKind::Sherpa => {
            let model_dir = model_dir
                .ok_or_else(|| {
                    SpeechError::SttInitFailed("model_dir required for Sherpa".to_string())
                })?
                .to_path_buf();

            let backend = tokio::task::spawn_blocking(move || {
                SherpaSttBackend::new(&model_dir)
            })
            .await
            .map_err(|e| {
                SpeechError::SttInitFailed(format!("spawn_blocking panicked: {}", e))
            })??;

            Ok(Box::new(backend))
        }

        SttBackendKind::Parakeet => {
            let model_dir = model_dir
                .ok_or_else(|| {
                    SpeechError::SttInitFailed("model_dir required for Parakeet".to_string())
                })?
                .to_path_buf();

            let backend = tokio::task::spawn_blocking(move || {
                ParakeetSttBackend::new(&model_dir)
            })
            .await
            .map_err(|e| {
                SpeechError::SttInitFailed(format!("spawn_blocking panicked: {}", e))
            })??;

            Ok(Box::new(backend))
        }
    }
}
