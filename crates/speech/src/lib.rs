//! Speech subsystem — local STT and TTS.
//!
//! This crate provides speech-to-text (STT) and text-to-speech (TTS)
//! capabilities using local models only (no cloud APIs).
//!
//! # Architecture
//! - STT and TTS are separate actors (isolation principle)
//! - All processing is local (no network calls)
//! - Backend selection: Whisper via candle (STT), Piper or platform APIs (TTS)
//! - Mock backends available for testing

pub mod audio_input;
pub mod audio_output;
mod candle_whisper;
pub mod download;
pub mod error;
pub mod onboarding;
mod parakeet_stt;
mod sherpa_stt;
mod silence_detector;
pub mod stt;
pub mod stt_actor;
pub mod telemetry;
pub mod tts_actor;
pub mod wakeword;

pub use audio_input::list_input_devices;
pub use error::SpeechError;
pub use parakeet_stt::ParakeetStt;
pub use sherpa_stt::SherpaZipformerStt;
pub use stt::SttBackend;
pub use stt_actor::SttActor;
pub use telemetry::log_stt_telemetry;
pub use tts_actor::TtsActor;
pub use wakeword::WakewordActor;

use serde::{Deserialize, Serialize};

/// Audio buffer for PCM samples.
#[derive(Debug, Clone)]
pub struct AudioBuffer {
    /// PCM samples (f32 normalized to [-1.0, 1.0]).
    pub samples: Vec<f32>,
    /// Sample rate (e.g., 16000 Hz for Whisper).
    pub sample_rate: u32,
    /// Number of channels (1 = mono, 2 = stereo).
    pub channels: u16,
}

/// STT backend kind — used for config and variant selection at construction time.
///
/// This enum is configuration-only. After `build_stt_backend()` constructs the
/// concrete `Box<dyn SttBackend>`, the actor never matches on this enum again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SttBackendKind {
    /// Whisper via candle (STT).
    Whisper,
    /// Sherpa-onnx Zipformer streaming STT (ONNX, <600MB VRAM).
    Sherpa,
    /// NVIDIA Parakeet streaming STT (ONNX format, 1.2-2GB VRAM recommended).
    Parakeet,
    /// Mock backend for testing.
    Mock,
}

/// TTS backend selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtsBackend {
    /// Piper local TTS (preferred).
    Piper,
    /// Platform-native TTS (AVSpeechSynthesizer/SAPI/espeak).
    SystemPlatform,
    /// Mock backend for testing.
    Mock,
}
