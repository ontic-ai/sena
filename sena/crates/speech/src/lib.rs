//! Speech subsystem — STT and TTS actors with pluggable backends.
//!
//! This crate provides:
//! - `SttBackend` and `TtsBackend` traits for pluggable speech processing
//! - `SttActor` and `TtsActor` for runtime integration
//! - Stub implementations for testing and development

pub mod audio_input;
pub mod audio_output;
pub mod backend;
pub mod error;
pub mod models;
pub mod onboarding;
pub mod parakeet_backend;
pub mod piper_backend;
mod silence_detector;
pub mod stt_actor;
pub mod tts_actor;
pub mod types;
pub mod wakeword;

pub use audio_input::{AudioChunk, AudioInputConfig, AudioInputStream};
pub use audio_output::{AudioBuffer, AudioOutputConfig, AudioOutputStream};
pub use backend::{AudioDevice, SttBackend, TtsBackend};
pub use error::{SpeechActorError, SttError, TtsError};
pub use models::{ModelCache, ModelInfo, ModelManifest, ModelType};
pub use onboarding::{check_model_cached, check_speech_models, speech_onboarding_needed};
pub use parakeet_backend::ParakeetSttBackend;
pub use piper_backend::PiperTtsBackend;
pub use stt_actor::{SttActor, StubSttBackend};
pub use tts_actor::{SpeakRequest, StubTtsBackend, TtsActor};
pub use types::{AudioStream, PendingSentence, SttBackendKind, SttEvent, TranscriptionResult};
pub use wakeword::{WakewordActor, WakewordConfig};
