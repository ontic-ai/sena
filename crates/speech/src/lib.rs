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
mod silence_detector;
pub mod stt_actor;
pub mod tts_actor;
pub mod wakeword;

pub use audio_input::list_input_devices;
pub use error::SpeechError;
pub use stt_actor::SttActor;
pub use tts_actor::TtsActor;
pub use wakeword::WakewordActor;

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

/// STT backend selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SttBackend {
    /// Whisper via candle (STT).
    Whisper,
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
