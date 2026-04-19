//! Speech subsystem — STT and TTS actors with pluggable backends.
//!
//! This crate provides:
//! - `SttBackend` and `TtsBackend` traits for pluggable speech processing
//! - `SttActor` and `TtsActor` for runtime integration
//! - Stub implementations for testing and development

pub mod audio_input;
pub mod backend;
pub mod error;
pub mod models;
pub mod stt_actor;
pub mod tts_actor;
pub mod types;

pub use audio_input::{AudioChunk, AudioInputConfig, AudioInputStream};
pub use backend::{AudioDevice, SttBackend, TtsBackend};
pub use error::{SpeechActorError, SttError, TtsError};
pub use models::{ModelCache, ModelInfo, ModelManifest, ModelType};
pub use stt_actor::{SttActor, StubSttBackend};
pub use tts_actor::{SpeakRequest, StubTtsBackend, TtsActor};
pub use types::{AudioStream, PendingSentence, SttBackendKind, SttEvent, TranscriptionResult};
