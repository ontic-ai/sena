//! Actor builder functions — construct actor instances with their backends.

use crate::error::RuntimeError;
use infer::{BackendType as InferBackendType, ModelRegistry as InferModelRegistry};
use platform::PlatformError;
use soul::SoulError;
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

/// Build the memory actor with a stub backend.
pub fn build_memory_actor() -> Result<memory::MemoryActor, RuntimeError> {
    tracing::debug!("building memory actor with stub backend");
    let backend = Box::new(memory::StubBackend);
    let actor = memory::MemoryActor::new(backend);
    Ok(actor)
}

/// Build the inference actor from discovered model registry.
///
/// - With `llama` feature enabled and at least one model available:
///   constructs infer backend via `infer::auto_backend`.
/// - If no model is available, or `llama` feature is disabled:
///   falls back to `inference::MockBackend`.
pub fn build_inference_actor(
    registry: &InferModelRegistry,
) -> Result<inference::InferenceActor, RuntimeError> {
    #[cfg(feature = "llama")]
    infer::suppress_llama_logs();

    #[cfg(feature = "llama")]
    {
        if registry.is_empty() {
            tracing::warn!("no models found — inference actor using MockBackend");
            let backend = Box::new(inference::MockBackend::default_loaded());
            return Ok(inference::InferenceActor::new(backend));
        }

        let backend_type = InferBackendType::auto_detect();
        let mut candidates: Vec<_> = registry.models().iter().collect();
        candidates.sort_by_key(|model| {
            let is_embedding_model = model.name.to_ascii_lowercase().contains("embed");
            (is_embedding_model, model.size_bytes)
        });

        for model in candidates {
            tracing::info!(
                model = %model.name,
                path = ?model.path,
                size_bytes = model.size_bytes,
                ?backend_type,
                "inference actor: attempting infer backend"
            );

            match infer::auto_backend(&model.path, backend_type) {
                Ok(infer_backend) => {
                    let adapter = inference::LlamaAdapter::from_infer_backend(infer_backend);
                    return Ok(inference::InferenceActor::new(Box::new(adapter)));
                }
                Err(e) => {
                    tracing::warn!(
                        model = %model.name,
                        path = ?model.path,
                        error = %e,
                        "inference actor: model load failed, trying next candidate"
                    );
                }
            }
        }

        tracing::warn!("all discovered models failed to load — inference actor using MockBackend");
        let backend = Box::new(inference::MockBackend::default_loaded());
        Ok(inference::InferenceActor::new(backend))
    }

    #[cfg(not(feature = "llama"))]
    {
        let _ = registry;
        tracing::warn!("llama feature disabled — inference actor using MockBackend");
        let backend = Box::new(inference::MockBackend::default_loaded());
        Ok(inference::InferenceActor::new(backend))
    }
}

/// Build the CTP actor.
///
/// The production runtime path ingests CTP signals from the bus; direct
/// signal injection sender is test-only and is therefore not returned here.
pub fn build_ctp_actor() -> Result<ctp::CtpActor, RuntimeError> {
    tracing::debug!("building CTP actor");
    let (actor, _signal_tx) = ctp::CtpActor::new();
    Ok(actor)
}

/// Build the STT actor with Parakeet backend or stub fallback.
///
/// Attempts to construct ParakeetSttBackend from models_dir. Falls back to
/// StubSttBackend with tracing::warn! if construction fails.
pub fn build_stt_actor(models_dir: &Path) -> Result<speech::SttActor, RuntimeError> {
    let encoder_path = models_dir.join("encoder.onnx");
    let decoder_path = models_dir.join("decoder_joint.onnx");
    let tokenizer_path = models_dir.join("tokenizer.model");

    match speech::ParakeetSttBackend::new(
        encoder_path.clone(),
        decoder_path.clone(),
        tokenizer_path.clone(),
    ) {
        Ok(backend) => {
            tracing::info!(
                encoder = ?encoder_path,
                decoder = ?decoder_path,
                tokenizer = ?tokenizer_path,
                "STT actor: using ParakeetSttBackend"
            );
            let actor = speech::SttActor::new(Box::new(backend));
            Ok(actor)
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                encoder = ?encoder_path,
                decoder = ?decoder_path,
                tokenizer = ?tokenizer_path,
                "STT actor: ParakeetSttBackend construction failed, falling back to StubSttBackend"
            );
            let backend = Box::new(speech::StubSttBackend::new(1600));
            let actor = speech::SttActor::new(backend);
            Ok(actor)
        }
    }
}

/// Build the TTS actor with Piper backend or stub fallback.
///
/// Attempts to construct PiperTtsBackend from models_dir. Falls back to
/// StubTtsBackend with tracing::warn! if construction fails.
pub fn build_tts_actor(models_dir: &Path) -> Result<speech::TtsActor, RuntimeError> {
    let model_path = models_dir.join("en_US-lessac-high.onnx");
    let config_path = models_dir.join("en_US-lessac-high.onnx.json");

    match speech::PiperTtsBackend::new(model_path.clone(), config_path.clone()) {
        Ok(backend) => {
            tracing::info!(
                model = ?model_path,
                config = ?config_path,
                "TTS actor: using PiperTtsBackend"
            );
            let actor = speech::TtsActor::new(Box::new(backend));
            Ok(actor)
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                model = ?model_path,
                config = ?config_path,
                "TTS actor: PiperTtsBackend construction failed, falling back to StubTtsBackend"
            );
            let backend = Box::new(speech::StubTtsBackend::new(16000));
            let actor = speech::TtsActor::new(backend);
            Ok(actor)
        }
    }
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
    use infer::{ModelInfo as InferModelInfo, Quantization as InferQuantization};
    use std::path::PathBuf;

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
        let result = build_inference_actor(&infer::ModelRegistry::new());
        assert!(result.is_ok());
    }

    #[test]
    fn inference_actor_falls_back_to_mock_when_model_load_fails() {
        let registry = infer::ModelRegistry::from_models(vec![InferModelInfo {
            name: "missing-model".to_string(),
            path: PathBuf::from("C:/does-not-exist/missing.gguf"),
            size_bytes: 1,
            quantization: InferQuantization::Unknown("unknown".to_string()),
        }]);

        let result = build_inference_actor(&registry);
        assert!(result.is_ok());
    }

    #[test]
    fn ctp_actor_builds() {
        let result = build_ctp_actor();
        assert!(result.is_ok());
    }

    #[test]
    fn stt_actor_builds() {
        let models_dir = std::path::Path::new(".");
        let result = build_stt_actor(models_dir);
        assert!(result.is_ok());
    }

    #[test]
    fn tts_actor_builds() {
        let models_dir = std::path::Path::new(".");
        let result = build_tts_actor(models_dir);
        assert!(result.is_ok());
    }

    #[test]
    fn prompt_actor_builds() {
        let result = build_prompt_actor();
        assert!(result.is_ok());
    }
}
