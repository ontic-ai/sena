//! Actor builder functions — construct actor instances with their backends.

use crate::error::RuntimeError;
#[cfg(test)]
use inference::MockBackend;
use platform::PlatformError;
use speech::{AudioInputConfig, ModelCache, ModelManifest};
use std::path::Path;
use std::time::{Duration, Instant};

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

/// Build the soul actor with a persistent redb store.
pub fn build_soul_actor(data_dir: &Path) -> Result<soul::SoulActor, RuntimeError> {
    let soul_db_path = data_dir.join("soul.redb");
    let soul_store = soul::RedbSoulStore::open(&soul_db_path)
        .map_err(|error| RuntimeError::SoulStore(error.to_string()))?;

    tracing::info!(path = %soul_db_path.display(), "soul actor: using RedbSoulStore");
    Ok(soul::SoulActor::new(Box::new(soul_store)))
}

/// Build the memory actor with the in-memory Echo0 backend.
pub fn build_memory_actor(
    data_dir: &Path,
    embed_tx: tokio::sync::mpsc::Sender<inference::EmbedRequest>,
) -> Result<memory::MemoryActor, RuntimeError> {
    let memory_db_path = data_dir.join("memory.redb");
    tracing::debug!(path = %memory_db_path.display(), "building memory actor with persistent backend");
    let backend = Box::new(
        memory::Echo0Backend::open(&memory_db_path, memory::SenaEmbedder::new(embed_tx))
            .map_err(|error| RuntimeError::MemoryStore(error.to_string()))?,
    );
    let actor = memory::MemoryActor::with_backup_config(
        backend,
        memory::BackupConfig::with_data_dir(data_dir.to_path_buf()),
    );
    Ok(actor)
}

/// Build the inference actor with a real backend.
///
/// Normal runtime boot is strict: a usable GGUF model must load successfully.
/// When `embed_model_path` is `Some`, a dedicated embedding backend is also
/// loaded and injected into the actor.
pub fn build_inference_actor(
    inference_max_tokens: usize,
    embed_rx: tokio::sync::mpsc::Receiver<inference::EmbedRequest>,
    embed_model_path: Option<std::path::PathBuf>,
) -> Result<inference::InferenceActor, RuntimeError> {
    let backend = load_inference_backend()?;
    tracing::info!("inference actor: using loaded LlamaBackend");

    let actor = inference::InferenceActor::with_embed_requests(backend, 100, embed_rx)
        .with_inference_max_tokens(inference_max_tokens);

    if let Some(path) = embed_model_path {
        match load_embed_backend(&path) {
            Ok(embed_backend) => {
                tracing::info!(path = %path.display(), "inference actor: embed backend loaded");
                Ok(actor.with_embed_backend(embed_backend))
            }
            Err(e) => {
                tracing::warn!(error = %e, "inference actor: embed backend failed to load, embeddings will fail");
                Ok(actor)
            }
        }
    } else {
        Ok(actor)
    }
}

#[cfg(not(test))]
fn load_embed_backend(
    path: &std::path::Path,
) -> Result<Box<dyn inference::InferenceBackend>, RuntimeError> {
    inference::build_loaded_embed_backend(path)
        .map_err(|e| RuntimeError::ModelLoadFailed(e.to_string()))
}

#[cfg(test)]
fn load_embed_backend(
    _path: &std::path::Path,
) -> Result<Box<dyn inference::InferenceBackend>, RuntimeError> {
    tracing::info!("inference actor: using MockBackend for embed in tests");
    Ok(Box::new(inference::MockBackend::default_loaded()))
}

#[cfg(not(test))]
fn load_inference_backend() -> Result<Box<dyn inference::InferenceBackend>, RuntimeError> {
    crate::llama_backend::build_default_backend()
}

#[cfg(test)]
fn load_inference_backend() -> Result<Box<dyn inference::InferenceBackend>, RuntimeError> {
    tracing::info!("inference actor: using MockBackend in tests");
    Ok(Box::new(MockBackend::default_loaded()))
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
/// * `config` - Runtime configuration containing STT device and VAD settings
///
/// Returns an SttActor with the best available backend.
pub fn build_stt_actor(
    models_dir: &Path,
    config: &crate::config::SenaConfig,
) -> Result<speech::SttActor, RuntimeError> {
    let stt_audio_config = stt_audio_config(config);
    let (stt_energy_threshold, stt_silence_duration_secs) = stt_vad_settings(config);

    // Check if Parakeet assets are available
    let encoder_path = ModelCache::cached_path(models_dir, &ModelManifest::parakeet_encoder());
    let decoder_path = ModelCache::cached_path(models_dir, &ModelManifest::parakeet_decoder());
    let tokenizer_path = ModelCache::cached_path(models_dir, &ModelManifest::parakeet_tokenizer());

    if encoder_path.exists() && decoder_path.exists() && tokenizer_path.exists() {
        tracing::info!("Parakeet STT assets found — attempting to construct ParakeetSttBackend");

        match speech::ParakeetSttBackend::new(encoder_path, decoder_path, tokenizer_path) {
            Ok(backend) => {
                tracing::info!("STT actor: using ParakeetSttBackend");
                let actor = speech::SttActor::new(Box::new(backend))
                    .with_audio_config(stt_audio_config.clone())
                    .with_vad_config(stt_energy_threshold, stt_silence_duration_secs);
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
    let actor = speech::SttActor::new(backend)
        .with_audio_config(stt_audio_config)
        .with_vad_config(stt_energy_threshold, stt_silence_duration_secs);
    Ok(actor)
}

fn stt_audio_config(config: &crate::config::SenaConfig) -> AudioInputConfig {
    AudioInputConfig {
        sample_rate: config.stt_sample_rate_hz,
        buffer_duration_secs: config.stt_buffer_duration_secs,
        input_device: config.microphone_device.clone(),
    }
}

fn stt_vad_settings(config: &crate::config::SenaConfig) -> (f32, f32) {
    (
        config.stt_energy_threshold,
        config.stt_silence_duration_secs,
    )
}

/// Build the TTS actor with a real Piper backend.
///
/// Piper assets are required for runtime speech output. Missing assets or
/// backend initialization failures return an error instead of silently
/// degrading to silent stub playback.
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
                return Err(RuntimeError::ModelLoadFailed(format!(
                    "PiperTtsBackend initialization failed: {}",
                    e
                )));
            }
        }
    }

    Err(RuntimeError::RequiredModelMissing {
        model_name: "piper tts assets".to_string(),
        reason: format!(
            "required Piper assets missing (model: {}, config: {})",
            model_path.exists(),
            config_path.exists()
        ),
    })
}

/// Build the wakeword actor when a wakeword model is available.
pub fn build_wakeword_actor(
    models_dir: &Path,
    sensitivity: f32,
) -> Result<Option<speech::WakewordActor>, RuntimeError> {
    let model_path = ModelCache::cached_path(models_dir, &ModelManifest::open_wakeword());

    if !model_path.exists() {
        tracing::debug!(
            "Wakeword model asset not available at {} — wakeword actor will not spawn",
            model_path.display()
        );
        return Ok(None);
    }

    let actor = speech::WakewordActor::new(speech::WakewordConfig {
        sensitivity,
        model_path: Some(model_path),
        model_dir: Some(models_dir.to_path_buf()),
        debounce_secs: 3.0,
    });
    Ok(Some(actor))
}

/// Build the Prompt actor.
pub fn build_prompt_actor() -> Result<prompt::PromptActor, RuntimeError> {
    tracing::debug!("building Prompt actor");
    let actor = prompt::PromptActor::new();
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
        let data_dir = tempdir().expect("failed to create tempdir");
        let result = build_soul_actor(data_dir.path());
        assert!(result.is_ok());
    }

    #[test]
    fn memory_actor_builds() {
        let (embed_tx, _embed_rx) = tokio::sync::mpsc::channel(1);
        let data_dir = tempdir().expect("failed to create tempdir");
        let result = build_memory_actor(data_dir.path(), embed_tx);
        assert!(result.is_ok());
    }

    #[test]
    fn inference_actor_builds() {
        let (_embed_tx, embed_rx) = tokio::sync::mpsc::channel(1);
        let result = build_inference_actor(512, embed_rx, None);
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
        let config = crate::config::SenaConfig::default();
        let result = build_stt_actor(models_dir.path(), &config);
        assert!(result.is_ok());
    }

    #[test]
    fn stt_audio_config_uses_runtime_settings() {
        let config = crate::config::SenaConfig {
            microphone_device: Some("USB Mic".to_string()),
            stt_sample_rate_hz: 22_050,
            stt_buffer_duration_secs: 0.2,
            stt_energy_threshold: 0.03,
            stt_silence_duration_secs: 0.8,
            ..Default::default()
        };

        let audio_config = stt_audio_config(&config);
        let (energy_threshold, silence_duration_secs) = stt_vad_settings(&config);

        assert_eq!(audio_config.sample_rate, 22_050);
        assert_eq!(audio_config.buffer_duration_secs, 0.2);
        assert_eq!(audio_config.input_device.as_deref(), Some("USB Mic"));
        assert_eq!(energy_threshold, 0.03);
        assert_eq!(silence_duration_secs, 0.8);
    }

    #[test]
    fn tts_actor_fails_when_assets_missing() {
        let models_dir = tempdir().expect("failed to create tempdir");
        let result = build_tts_actor(models_dir.path());
        assert!(matches!(result, Err(RuntimeError::RequiredModelMissing { .. })));
    }

    #[test]
    fn wakeword_actor_not_built_when_asset_missing() {
        let models_dir = tempdir().expect("failed to create tempdir");
        let result =
            build_wakeword_actor(models_dir.path(), 0.5).expect("wakeword builder should succeed");
        assert!(result.is_none());
    }

    #[test]
    fn wakeword_actor_built_when_asset_exists() {
        let models_dir = tempdir().expect("failed to create tempdir");
        let model = ModelManifest::open_wakeword();
        let model_path = ModelCache::cached_path(models_dir.path(), &model);

        std::fs::write(&model_path, b"stub wakeword model").expect("write wakeword asset");

        let result =
            build_wakeword_actor(models_dir.path(), 0.6).expect("wakeword builder should succeed");
        assert!(result.is_some());
    }

    #[test]
    fn prompt_actor_builds() {
        let result = build_prompt_actor();
        assert!(result.is_ok());
    }
}
