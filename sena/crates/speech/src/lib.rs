//! Speech subsystem — STT and TTS actors with pluggable backends.
//!
//! This crate provides:
//! - `SttBackend` and `TtsBackend` traits for pluggable speech processing
//! - `SttActor` and `TtsActor` for runtime integration
//! - Stub implementations for testing and development
//! - Parakeet Nemotron STT backend
//! - Piper ONNX TTS backend

pub mod backend;
pub mod error;
pub mod parakeet_backend;
pub mod piper_backend;
pub mod stt_actor;
pub mod tts_actor;
pub mod types;

pub use backend::{AudioDevice, SttBackend, TtsBackend};
pub use error::{SpeechActorError, SttError, TtsError};
pub use parakeet_backend::ParakeetSttBackend;
pub use piper_backend::PiperTtsBackend;
pub use stt_actor::{AudioChunk, SttActor, StubSttBackend};
pub use tts_actor::{SpeakRequest, StubTtsBackend, TtsActor};
pub use types::{AudioStream, PendingSentence, SttBackendKind, SttEvent, TranscriptionResult};
