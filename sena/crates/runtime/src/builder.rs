//! Actor builder functions — construct actor instances with their backends.

use crate::error::RuntimeError;
use inference::InferenceError;
use platform::PlatformError;
use soul::SoulError;
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

    async fn shutdown(&mut self) -> Result<(), InferenceError> {
        Ok(())
    }
}

/// Build the platform actor with a stub backend.
pub fn build_platform_actor() -> Result<platform::PlatformActor, RuntimeError> {
    tracing::debug!("building platform actor with stub backend");
    let backend = Box::new(StubPlatformBackend);
    let actor = platform::PlatformActor::new(backend);
    Ok(actor)
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

/// Build the inference actor with a stub backend.
pub fn build_inference_actor() -> Result<inference::InferenceActor, RuntimeError> {
    tracing::debug!("building inference actor with stub backend");
    let backend = Box::new(StubInferenceBackend);
    let actor = inference::InferenceActor::new(backend);
    Ok(actor)
}

/// Build the CTP actor with stub backends.
///
/// Returns (actor, signal_tx) where signal_tx can be used to send signals to CTP.
pub fn build_ctp_actor() -> Result<
    (
        ctp::CtpActor,
        tokio::sync::mpsc::UnboundedSender<ctp::CtpSignal>,
    ),
    RuntimeError,
> {
    tracing::debug!("building CTP actor with stub backends");
    let platform_backend: std::sync::Arc<dyn platform::PlatformBackend> =
        std::sync::Arc::new(StubPlatformBackend);
    let soul_store: std::sync::Arc<dyn soul::SoulStore> = std::sync::Arc::new(StubSoulStore);

    let (actor, signal_tx) = ctp::CtpActor::new(platform_backend, soul_store);
    Ok((actor, signal_tx))
}

/// Build the STT actor with a stub backend.
pub fn build_stt_actor() -> Result<speech::SttActor, RuntimeError> {
    tracing::debug!("building STT actor with stub backend");
    let backend = Box::new(speech::StubSttBackend::new(1600));
    // Stub: create bus and channel but don't use them
    let bus = std::sync::Arc::new(bus::EventBus::new());
    let (_audio_tx, audio_rx) = tokio::sync::mpsc::unbounded_channel();
    let actor = speech::SttActor::new(backend, bus, audio_rx);
    Ok(actor)
}

/// Build the TTS actor with a stub backend.
pub fn build_tts_actor() -> Result<speech::TtsActor, RuntimeError> {
    tracing::debug!("building TTS actor with stub backend");
    let backend = Box::new(speech::StubTtsBackend::new(16000));
    // Stub: create bus and channel but don't use them
    let bus = std::sync::Arc::new(bus::EventBus::new());
    let (_speak_tx, speak_rx) = tokio::sync::mpsc::unbounded_channel();
    let actor = speech::TtsActor::new(backend, bus, speak_rx);
    Ok(actor)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let result = build_inference_actor();
        assert!(result.is_ok());
    }

    #[test]
    fn ctp_actor_builds() {
        let result = build_ctp_actor();
        assert!(result.is_ok());
    }

    #[test]
    fn stt_actor_builds() {
        let result = build_stt_actor();
        assert!(result.is_ok());
    }

    #[test]
    fn tts_actor_builds() {
        let result = build_tts_actor();
        assert!(result.is_ok());
    }
}
