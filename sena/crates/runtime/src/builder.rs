//! Actor builder functions — construct actor instances with their backends.

use crate::error::RuntimeError;
use inference::InferenceError;
use platform::PlatformError;
use soul::SoulError;
use speech::{ModelCache, ModelManifest};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

/// Stub platform backend implementation.
struct StubPlatformBackend;

impl platform::PlatformBackend for StubPlatformBackend {
    fn active_window(&self) -> Result<platform::PlatformSignal, PlatformError> {
        Ok(platform::PlatformSignal::Window(platform::WindowContext {
            app_name: "stub".to_string(),
            window_title: None,
            bundle_id: None,
            timestamp: Instant::now(),
        }))
    }

    fn clipboard_content(&self) -> Result<platform::PlatformSignal, PlatformError> {
        Ok(platform::PlatformSignal::Clipboard(
            platform::ClipboardDigest {
                digest: None,
                char_count: 0,
                timestamp: Instant::now(),
            },
        ))
    }

    fn keystroke_cadence(&self) -> Result<platform::PlatformSignal, PlatformError> {
        Ok(platform::PlatformSignal::Keystroke(
            platform::KeystrokeCadence {
                events_per_minute: 0.0,
                burst_detected: false,
                idle_duration: Duration::from_secs(0),
                timestamp: Instant::now(),
            },
        ))
    }

    fn screen_frame(&self) -> Result<platform::PlatformSignal, PlatformError> {
        Ok(platform::PlatformSignal::ScreenFrame(
            platform::ScreenFrame {
                width: 1,
                height: 1,
                rgb_data: vec![0, 0, 0],
                timestamp: Instant::now(),
            },
        ))
    }
}

/// Stub soul store implementation.
struct StubSoulStore;

impl soul::SoulStore for StubSoulStore {
    fn write_event(
        &mut self,
        _description: String,
        _app_context: Option<String>,
        _timestamp: SystemTime,
    ) -> Result<u64, SoulError> {
        Ok(1)
    }

    fn read_summary(
        &self,
        _max_events: usize,
        _max_chars: Option<usize>,
    ) -> Result<soul::SoulSummary, SoulError> {
        Ok(soul::SoulSummary {
            content: String::new(),
            event_count: 0,
        })
    }

    fn read_event(&self, _row_id: u64) -> Result<Option<soul::SoulEventRecord>, SoulError> {
        Ok(None)
    }

    fn write_identity_signal(&mut self, _key: &str, _value: &str) -> Result<(), SoulError> {
        Ok(())
    }

    fn read_identity_signal(&self, _key: &str) -> Result<Option<String>, SoulError> {
        Ok(None)
    }

    fn read_all_identity_signals(&self) -> Result<Vec<soul::IdentitySignal>, SoulError> {
        Ok(Vec::new())
    }

    fn increment_identity_counter(&mut self, _key: &str, _delta: u64) -> Result<(), SoulError> {
        Ok(())
    }

    fn write_temporal_pattern(&mut self, _pattern: soul::TemporalPattern) -> Result<(), SoulError> {
        Ok(())
    }

    fn read_temporal_patterns(&self) -> Result<Vec<soul::TemporalPattern>, SoulError> {
        Ok(Vec::new())
    }

    fn initialize(&mut self) -> Result<(), SoulError> {
        Ok(())
    }

    fn close(&mut self) -> Result<(), SoulError> {
        Ok(())
    }
}

/// Stub inference backend implementation.
struct StubInferenceBackend;

#[async_trait::async_trait]
impl inference::InferenceBackend for StubInferenceBackend {
    fn backend_type(&self) -> inference::BackendType {
        inference::BackendType::Mock
    }

    fn is_loaded(&self) -> bool {
        false
    }

    async fn infer(
        &self,
        _prompt: String,
        _params: inference::InferenceParams,
    ) -> Result<inference::InferenceStream, InferenceError> {
        Err(InferenceError::ModelNotLoaded)
    }

    fn complete(
        &self,
        _prompt: &str,
        _params: &inference::InferenceParams,
    ) -> Result<String, InferenceError> {
        Err(InferenceError::ModelNotLoaded)
    }

    // embed() and extract() use default trait implementations

    async fn shutdown(&mut self) -> Result<(), InferenceError> {
        Ok(())
    }
}

/// Build the platform actor with the native OS backend.
///
/// Falls back to stub backend if native backend construction fails.
pub fn build_platform_actor() -> Result<platform::PlatformActor, RuntimeError> {
    match platform::PlatformActor::native() {
        Ok(actor) => {
            tracing::info!("platform actor: using NativeBackend");
            Ok(actor)
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "platform actor: NativeBackend failed, falling back to StubPlatformBackend"
            );
            let backend = Box::new(StubPlatformBackend);
            Ok(platform::PlatformActor::new(backend))
        }
    }
}

/// Build the soul actor with a stub store.
pub fn build_soul_actor() -> Result<soul::SoulActor, RuntimeError> {
    tracing::debug!("building soul actor with stub store");
    let store = Box::new(StubSoulStore);
    let actor = soul::SoulActor::new(store);
    Ok(actor)
}

/// Build the memory actor with the in-memory Echo0 backend.
pub fn build_memory_actor() -> Result<memory::MemoryActor, RuntimeError> {
    tracing::debug!("building memory actor with Echo0 backend");
    let backend = Box::new(memory::Echo0Backend::new());
    let actor = memory::MemoryActor::new(backend);
    Ok(actor)
}

/// Build the inference actor with a real backend if available, stub otherwise.
///
/// Attempts to discover and load a GGUF model from the default models directory.
/// Falls back to stub backend if no model is available (with a warning).
pub fn build_inference_actor(
    inference_max_tokens: usize,
) -> Result<inference::InferenceActor, RuntimeError> {
    use crate::llama_backend::try_build_default_backend;

    match try_build_default_backend() {
        Some(backend) => {
            tracing::info!("inference actor: using real LlamaBackend");
            Ok(inference::InferenceActor::new(backend)
                .with_inference_max_tokens(inference_max_tokens))
        }
        None => {
            tracing::warn!(
                "inference actor: no model available, falling back to StubInferenceBackend"
            );
            let backend = Box::new(StubInferenceBackend);
            Ok(inference::InferenceActor::new(backend)
                .with_inference_max_tokens(inference_max_tokens))
        }
    }
}

/// Build the CTP actor.
///
/// Returns (actor, signal_tx) where signal_tx can be used to send signals to CTP.
pub fn build_ctp_actor() -> Result<
    (
        ctp::CtpActor,
        tokio::sync::mpsc::UnboundedSender<ctp::CtpSignal>,
    ),
    RuntimeError,
> {
    tracing::debug!("building CTP actor");
    let (actor, signal_tx) = ctp::CtpActor::new();
    Ok((actor, signal_tx))
}

/// Build the STT actor with real or stub backend.
///
/// Attempts to construct a Parakeet backend if all required assets are present.
/// Falls back to stub backend if assets are missing or initialization fails.
///
/// # Arguments
/// * `models_dir` - Path to the speech models directory
///
/// Returns an SttActor with the best available backend.
pub fn build_stt_actor(models_dir: &Path) -> Result<speech::SttActor, RuntimeError> {
    // Check if Parakeet assets are available
    let encoder_path = ModelCache::cached_path(models_dir, &ModelManifest::parakeet_encoder());
    let decoder_path = ModelCache::cached_path(models_dir, &ModelManifest::parakeet_decoder());
    let tokenizer_path = ModelCache::cached_path(models_dir, &ModelManifest::parakeet_tokenizer());

    if encoder_path.exists() && decoder_path.exists() && tokenizer_path.exists() {
        tracing::info!("Parakeet STT assets found — attempting to construct ParakeetSttBackend");

        match speech::ParakeetSttBackend::new(encoder_path, decoder_path, tokenizer_path) {
            Ok(backend) => {
                tracing::info!("STT actor: using ParakeetSttBackend");
                let actor = speech::SttActor::new(Box::new(backend));
                return Ok(actor);
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "ParakeetSttBackend initialization failed — falling back to stub"
                );
            }
        }
    } else {
        tracing::debug!(
            "Parakeet assets not available (encoder: {}, decoder: {}, tokenizer: {}) — using stub backend",
            encoder_path.exists(),
            decoder_path.exists(),
            tokenizer_path.exists()
        );
    }

    // Fall back to stub backend
    tracing::info!("STT actor: using StubSttBackend");
    let backend = Box::new(speech::StubSttBackend::new(1600));
    let actor = speech::SttActor::new(backend);
    Ok(actor)
}

/// Build the TTS actor with real or stub backend.
///
/// Attempts to construct a Piper backend if all required assets are present.
/// Falls back to stub backend if assets are missing or initialization fails.
///
/// # Arguments
/// * `models_dir` - Path to the speech models directory
///
/// Returns a TtsActor with the best available backend.
pub fn build_tts_actor(models_dir: &Path) -> Result<speech::TtsActor, RuntimeError> {
    // Check if Piper assets are available
    let model_path = ModelCache::cached_path(models_dir, &ModelManifest::piper_voice());
    let config_path = ModelCache::cached_path(models_dir, &ModelManifest::piper_config());

    if model_path.exists() && config_path.exists() {
        tracing::info!("Piper TTS assets found — attempting to construct PiperTtsBackend");

        match speech::PiperTtsBackend::new(model_path, config_path) {
            Ok(backend) => {
                tracing::info!("TTS actor: using PiperTtsBackend");
                let actor = speech::TtsActor::new(Box::new(backend));
                return Ok(actor);
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "PiperTtsBackend initialization failed — falling back to stub"
                );
            }
        }
    } else {
        tracing::debug!(
            "Piper assets not available (model: {}, config: {}) — using stub backend",
            model_path.exists(),
            config_path.exists()
        );
    }

    // Fall back to stub backend
    tracing::info!("TTS actor: using StubTtsBackend");
    let backend = Box::new(speech::StubTtsBackend::new(16000));
    let actor = speech::TtsActor::new(backend);
    Ok(actor)
}

/// Build the Prompt actor with a stub composer.
pub fn build_prompt_actor() -> Result<prompt::PromptActor, RuntimeError> {
    tracing::debug!("building Prompt actor with stub composer");
    let composer = Box::new(prompt::StubComposer::default_segments());
    let actor = prompt::PromptActor::new(composer);
    Ok(actor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn platform_actor_builds() {
        let result = build_platform_actor();
        assert!(result.is_ok());
    }

    #[test]
    fn soul_actor_builds() {
        let result = build_soul_actor();
        assert!(result.is_ok());
    }

    #[test]
    fn memory_actor_builds() {
        let result = build_memory_actor();
        assert!(result.is_ok());
    }

    #[test]
    fn inference_actor_builds() {
        let result = build_inference_actor(512);
        assert!(result.is_ok());
    }

    #[test]
    fn ctp_actor_builds() {
        let result = build_ctp_actor();
        assert!(result.is_ok());
    }

    #[test]
    fn stt_actor_builds_with_stub_backend_when_assets_missing() {
        let models_dir = tempdir().expect("failed to create tempdir");
        let result = build_stt_actor(models_dir.path());
        assert!(result.is_ok());
    }

    #[test]
    fn tts_actor_builds_with_stub_backend_when_assets_missing() {
        let models_dir = tempdir().expect("failed to create tempdir");
        let result = build_tts_actor(models_dir.path());
        assert!(result.is_ok());
    }

    #[test]
    fn prompt_actor_builds() {
        let result = build_prompt_actor();
        assert!(result.is_ok());
    }
}
